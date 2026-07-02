use crate::{
    ensure_rustls_provider,
    gitops::ops::{
        CacheAcquisitionMode, GrepOptions, GrepResult, cache_github_repo, grep_repo_with_options,
        head_with_ignore, list_dir_with_ignore, read_file, read_file_with_ignore, tail_with_ignore,
        tree_text_with_ignore,
    },
    search::{DEFAULT_GITHUB_REPO_LIMIT, GitHubSearchClient, MAX_GITHUB_REPOS},
    sed,
};
use rmcp::schemars::JsonSchema;
use rmcp::{
    ErrorData as McpError, Json, RoleServer, ServerHandler, ServiceExt,
    handler::server::{
        router::{prompt::PromptRouter, tool::ToolRouter},
        wrapper::Parameters,
    },
    model::{
        GetPromptResult, Implementation, ListResourcesResult, PaginatedRequestParams,
        PromptMessage, ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents,
        Role, ServerCapabilities, ServerInfo,
    },
    prompt, prompt_handler, prompt_router,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

const SKILL_MD: &str = include_str!("skill/SKILL.md");
const DEFAULT_TEXT_BYTES: usize = 32 * 1024;
const MAX_TEXT_BYTES: usize = 256 * 1024;
const DEFAULT_TREE_ENTRIES: usize = 5_000;
const MAX_TREE_ENTRIES: usize = 50_000;
const MAX_CONTEXT_LINES: usize = 200;

#[derive(Clone)]
pub struct WitMcpServer {
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
}

impl WitMcpServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }
}

impl Default for WitMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn serve_stdio() -> anyhow::Result<()> {
    ensure_rustls_provider();
    let service = WitMcpServer::new()
        .serve(stdio())
        .await
        .inspect_err(|err| {
            tracing::error!(?err, "wit MCP server failed");
        })?;
    service.waiting().await?;
    Ok(())
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Optional repository-name filter. GitHub treats this as a literal name search.
    pub pattern: Option<String>,
    /// Optional GitHub language qualifier, such as "Rust".
    pub lang: Option<String>,
    /// Additional raw GitHub repository-search terms and qualifiers.
    pub query: Option<String>,
    /// Maximum repositories to return. Defaults to 10 and must be between 1 and 1000.
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RepoArgs {
    /// GitHub repository in owner/repo form.
    pub repo: String,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct TreeArgs {
    /// GitHub repository in owner/repo form.
    pub repo: String,
    /// Optional subdirectory path.
    pub path: Option<String>,
    /// Include line counts and approximate token estimates.
    #[serde(default)]
    pub long: bool,
    /// Force refresh the default-branch cache before reading.
    #[serde(default)]
    pub refresh_cache: bool,
    /// Exclude files, directories, or glob patterns.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Maximum file entries to render. Defaults to 5000.
    pub max_entries: Option<usize>,
    /// Maximum returned text bytes. Defaults to 32768.
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct LsArgs {
    /// GitHub repository in owner/repo form.
    pub repo: String,
    /// Optional directory path.
    pub path: Option<String>,
    /// Include line counts and approximate token estimates.
    #[serde(default)]
    pub long: bool,
    /// Force refresh the default-branch cache before reading.
    #[serde(default)]
    pub refresh_cache: bool,
    /// Exclude files, directories, or glob patterns.
    #[serde(default)]
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct CatArgs {
    /// GitHub repository in owner/repo form.
    pub repo: String,
    /// Path to the file within the repository.
    pub path: String,
    /// Number all output lines.
    #[serde(default)]
    pub number: bool,
    /// Number non-blank output lines only. Overrides number.
    #[serde(default)]
    pub number_nonblank: bool,
    /// Suppress repeated empty output lines.
    #[serde(default)]
    pub squeeze_blank: bool,
    /// Display $ at end of each line.
    #[serde(default)]
    pub show_ends: bool,
    /// Display TAB characters as ^I.
    #[serde(default)]
    pub show_tabs: bool,
    /// Equivalent to show_ends plus show_tabs.
    #[serde(default)]
    pub show_all: bool,
    /// Force refresh the default-branch cache before reading.
    #[serde(default)]
    pub refresh_cache: bool,
    /// Exclude files, directories, or glob patterns.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Maximum returned text bytes. Defaults to 32768.
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct RgArgs {
    /// GitHub repository in owner/repo form.
    pub repo: String,
    /// Rust regex pattern to search for.
    pub pattern: String,
    #[serde(default)]
    pub ignore_case: bool,
    #[serde(default)]
    pub smart_case: bool,
    #[serde(default)]
    pub word_regexp: bool,
    #[serde(default)]
    pub invert_match: bool,
    /// Maximum matches to show. Omit for unlimited; use 0 to return no matches.
    pub max_count: Option<usize>,
    /// Lines of context before and after matches.
    #[serde(default)]
    pub context: usize,
    /// Lines of context before matches.
    #[serde(default)]
    pub before_context: usize,
    /// Lines of context after matches.
    #[serde(default)]
    pub after_context: usize,
    /// Glob pattern to filter files, such as "*.rs" or "src/**".
    pub glob: Option<String>,
    /// Only show file names with matches.
    #[serde(default)]
    pub files_with_matches: bool,
    /// Only show count of matches per file.
    #[serde(default)]
    pub count: bool,
    /// Include line counts and approximate token estimates with files_with_matches.
    #[serde(default)]
    pub long: bool,
    /// Force refresh the default-branch cache before reading.
    #[serde(default)]
    pub refresh_cache: bool,
    /// Exclude files, directories, or glob patterns.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Maximum returned text bytes. Defaults to 32768.
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct SedArgs {
    /// GitHub repository in owner/repo form.
    pub repo: String,
    /// Path to the file within the repository.
    pub path: String,
    /// Inline sed script.
    pub script: Option<String>,
    /// Additional sed script expressions.
    #[serde(default)]
    pub expressions: Vec<String>,
    /// Local server-side sed script files are rejected by MCP; pass scripts inline instead.
    #[serde(default)]
    pub script_files: Vec<String>,
    /// Suppress automatic printing of pattern space.
    #[serde(default)]
    pub quiet: bool,
    /// Force refresh the default-branch cache before reading.
    #[serde(default)]
    pub refresh_cache: bool,
    /// Exclude files, directories, or glob patterns.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Maximum returned text bytes. Defaults to 32768.
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct HeadArgs {
    /// GitHub repository in owner/repo form.
    pub repo: String,
    /// Path to the file within the repository.
    pub path: String,
    /// Number of lines to show. Defaults to 10.
    pub lines: Option<usize>,
    /// Number all output lines.
    #[serde(default)]
    pub number: bool,
    /// Force refresh the default-branch cache before reading.
    #[serde(default)]
    pub refresh_cache: bool,
    /// Exclude files, directories, or glob patterns.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Maximum returned text bytes. Defaults to 32768.
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct TailArgs {
    /// GitHub repository in owner/repo form.
    pub repo: String,
    /// Path to the file within the repository.
    pub path: String,
    /// Number of lines to show from the end when from_line is not set. Defaults to 10.
    pub lines: Option<usize>,
    /// Start from line N, like tail -n +N.
    pub from_line: Option<usize>,
    /// Number all output lines.
    #[serde(default)]
    pub number: bool,
    /// Force refresh the default-branch cache before reading.
    #[serde(default)]
    pub refresh_cache: bool,
    /// Exclude files, directories, or glob patterns.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Maximum returned text bytes. Defaults to 32768.
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SkillInstallArgs {
    /// Directory in which to create the wit-skill folder.
    pub path: PathBuf,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchResponse {
    pub repositories: Vec<SearchRepository>,
    pub total_count: u64,
    pub incomplete_results: bool,
    pub metric: &'static str,
    pub limit: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchRepository {
    pub name: String,
    pub full_name: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub stars: u32,
    pub html_url: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CacheResponse {
    pub repo: String,
    pub cache_path: String,
    pub refreshed: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TextResponse {
    pub command: &'static str,
    pub repo: Option<String>,
    pub path: Option<String>,
    pub text: String,
    pub truncated: bool,
    pub original_bytes: usize,
    pub returned_bytes: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TreeResponse {
    pub repo: String,
    pub path: Option<String>,
    pub long: bool,
    pub text: String,
    pub entries: usize,
    pub truncated: bool,
    pub truncated_by_entries: bool,
    pub original_bytes: usize,
    pub returned_bytes: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LsResponse {
    pub repo: String,
    pub path: Option<String>,
    pub long: bool,
    pub entries: Vec<LsEntry>,
    pub text: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LsEntry {
    pub name: String,
    pub kind: &'static str,
    pub size_bytes: Option<u64>,
    pub lines: Option<usize>,
    pub approx_tokens: Option<usize>,
    pub is_binary: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GrepResponse {
    pub repo: String,
    pub pattern: String,
    pub mode: &'static str,
    pub text: String,
    pub truncated: bool,
    pub original_bytes: usize,
    pub returned_bytes: usize,
    pub effective_max_count: Option<usize>,
    pub matches: Vec<GrepMatchResponse>,
    pub files: Vec<GrepFileResponse>,
    pub counts: Vec<GrepCountResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GrepMatchResponse {
    pub path: String,
    pub line_number: u64,
    pub content: String,
    pub is_context: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GrepFileResponse {
    pub path: String,
    pub lines: Option<usize>,
    pub approx_tokens: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GrepCountResponse {
    pub path: String,
    pub count: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SedResponse {
    pub repo: String,
    pub path: String,
    pub text: String,
    pub exit_code: i32,
    pub truncated: bool,
    pub original_bytes: usize,
    pub returned_bytes: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SkillResponse {
    pub text: String,
    pub bytes: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SkillInstallResponse {
    pub path: String,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ExploreRepoPromptArgs {
    /// GitHub repository in owner/repo form.
    pub repo: String,
    /// The user's exploration goal.
    pub objective: Option<String>,
    /// Optional paths that are likely relevant.
    #[serde(default)]
    pub focus_paths: Vec<String>,
    /// Optional ignore patterns for generated, vendored, or noisy files.
    #[serde(default)]
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct DiscoverReposPromptArgs {
    /// What kind of repository the user is looking for.
    pub topic: String,
    /// Optional GitHub language qualifier.
    pub lang: Option<String>,
    /// Optional raw GitHub qualifiers such as stars:>1000 archived:false.
    pub query: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ReadPrecisePromptArgs {
    /// GitHub repository in owner/repo form.
    pub repo: String,
    /// Path to inspect.
    pub path: String,
    /// What the user needs to learn from the file.
    pub objective: Option<String>,
    /// Optional known line range like "120-180".
    pub line_range: Option<String>,
}

#[tool_router(router = tool_router)]
impl WitMcpServer {
    #[tool(
        name = "wit_search",
        description = "Find GitHub repositories using GitHub repository search qualifiers"
    )]
    pub async fn wit_search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<Json<SearchResponse>, String> {
        let limit = validate_search_limit(args.limit)?;
        let results = GitHubSearchClient::new()
            .search_repositories(
                args.pattern.as_deref(),
                args.lang.as_deref(),
                args.query.as_deref(),
                limit,
            )
            .await
            .map_err(github_error)?;

        let repositories = results
            .repositories
            .into_iter()
            .map(|repo| SearchRepository {
                name: repo.name,
                full_name: repo.full_name,
                description: repo.description,
                language: repo.language,
                stars: repo.stars,
                html_url: repo.html_url,
            })
            .collect();

        Ok(Json(SearchResponse {
            repositories,
            total_count: results.total_count,
            incomplete_results: results.incomplete_results,
            metric: "stars",
            limit,
        }))
    }

    #[tool(
        name = "wit_cache_refresh",
        description = "Force-refresh a GitHub repository in the local wit cache"
    )]
    pub async fn wit_cache_refresh(
        &self,
        Parameters(args): Parameters<RepoArgs>,
    ) -> Result<Json<CacheResponse>, String> {
        let repo = cache_github_repo(&args.repo, CacheAcquisitionMode::ForceInvalidate)
            .await
            .map_err(anyhow_error)?;
        Ok(Json(CacheResponse {
            repo: args.repo,
            cache_path: repo.path().display().to_string(),
            refreshed: true,
        }))
    }

    #[tool(
        name = "wit_tree",
        description = "Show a recursive file tree for a GitHub repository or subtree"
    )]
    pub async fn wit_tree(
        &self,
        Parameters(args): Parameters<TreeArgs>,
    ) -> Result<Json<TreeResponse>, String> {
        let max_entries = validate_limit(
            args.max_entries,
            DEFAULT_TREE_ENTRIES,
            MAX_TREE_ENTRIES,
            "max_entries",
        )?;
        let max_bytes = validate_text_limit(args.max_bytes)?;
        let repo = cache_github_repo(&args.repo, cache_mode(args.refresh_cache))
            .await
            .map_err(anyhow_error)?;
        let tree = tree_text_with_ignore(
            &repo,
            args.path.as_deref(),
            args.long,
            &args.ignore,
            Some(max_entries),
        )
        .map_err(anyhow_error)?;
        let bounded = bound_text(tree.text, max_bytes);
        Ok(Json(TreeResponse {
            repo: args.repo,
            path: args.path,
            long: args.long,
            text: bounded.text,
            entries: tree.entries,
            truncated: bounded.truncated || tree.truncated,
            truncated_by_entries: tree.truncated,
            original_bytes: bounded.original_bytes,
            returned_bytes: bounded.returned_bytes,
        }))
    }

    #[tool(
        name = "wit_ls",
        description = "List one directory level in a GitHub repository"
    )]
    pub async fn wit_ls(
        &self,
        Parameters(args): Parameters<LsArgs>,
    ) -> Result<Json<LsResponse>, String> {
        let repo = cache_github_repo(&args.repo, cache_mode(args.refresh_cache))
            .await
            .map_err(anyhow_error)?;
        let entries = list_dir_with_ignore(&repo, args.path.as_deref(), args.long, &args.ignore)
            .map_err(anyhow_error)?;
        let entries = entries
            .into_iter()
            .map(|entry| LsEntry {
                name: entry.name,
                kind: if entry.is_dir { "directory" } else { "file" },
                size_bytes: entry.size_bytes,
                lines: entry.lines,
                approx_tokens: entry.lines.map(|lines| lines * 5),
                is_binary: entry.is_binary,
            })
            .collect::<Vec<_>>();
        let text = format_ls_text(&entries, args.long);
        Ok(Json(LsResponse {
            repo: args.repo,
            path: args.path,
            long: args.long,
            entries,
            text,
        }))
    }

    #[tool(
        name = "wit_cat",
        description = "Read a file from a GitHub repository with POSIX cat-style display flags"
    )]
    pub async fn wit_cat(
        &self,
        Parameters(args): Parameters<CatArgs>,
    ) -> Result<Json<TextResponse>, String> {
        let max_bytes = validate_text_limit(args.max_bytes)?;
        let repo = cache_github_repo(&args.repo, cache_mode(args.refresh_cache))
            .await
            .map_err(anyhow_error)?;
        let content =
            read_file_with_ignore(&repo, &args.path, &args.ignore).map_err(anyhow_error)?;
        let text = format_cat(&content, &args);
        Ok(Json(text_response(
            "wit_cat",
            Some(args.repo),
            Some(args.path),
            text,
            max_bytes,
        )))
    }

    #[tool(
        name = "wit_rg",
        description = "Search repository file contents with ripgrep-style regex options"
    )]
    pub async fn wit_rg(
        &self,
        Parameters(args): Parameters<RgArgs>,
    ) -> Result<Json<GrepResponse>, String> {
        validate_context(args.context, "context")?;
        validate_context(args.before_context, "before_context")?;
        validate_context(args.after_context, "after_context")?;
        let max_bytes = validate_text_limit(args.max_bytes)?;
        let effective_max_count = args.max_count;

        let repo = cache_github_repo(&args.repo, cache_mode(args.refresh_cache))
            .await
            .map_err(anyhow_error)?;
        let mut opts = GrepOptions::new()
            .ignore_case(args.ignore_case)
            .smart_case(args.smart_case)
            .word_regexp(args.word_regexp)
            .invert_match(args.invert_match)
            .before_context(if args.context > 0 {
                args.context
            } else {
                args.before_context
            })
            .after_context(if args.context > 0 {
                args.context
            } else {
                args.after_context
            })
            .glob(args.glob.clone())
            .ignore(args.ignore.clone())
            .files_with_matches(args.files_with_matches)
            .count(args.count);
        if let Some(max_count) = effective_max_count {
            opts = opts.max_count(max_count);
        }

        let result = grep_repo_with_options(&repo, &args.pattern, &opts).map_err(anyhow_error)?;
        let mut matches = Vec::new();
        let mut files = Vec::new();
        let mut counts = Vec::new();
        let (mode, text) = match result {
            GrepResult::Matches(raw_matches) => {
                matches = raw_matches
                    .into_iter()
                    .map(|item| GrepMatchResponse {
                        path: item.path,
                        line_number: item.line_number,
                        content: item.content,
                        is_context: item.is_context,
                    })
                    .collect();
                ("matches", format_grep_matches(&matches))
            }
            GrepResult::Files(raw_files) => {
                files = raw_files
                    .into_iter()
                    .map(|path| {
                        if args.long {
                            let lines = read_file(&repo, &path)
                                .ok()
                                .map(|text| text.lines().count());
                            GrepFileResponse {
                                path,
                                lines,
                                approx_tokens: lines.map(|lines| lines * 5),
                            }
                        } else {
                            GrepFileResponse {
                                path,
                                lines: None,
                                approx_tokens: None,
                            }
                        }
                    })
                    .collect();
                ("files", format_grep_files(&files))
            }
            GrepResult::Counts(raw_counts) => {
                counts = raw_counts
                    .into_iter()
                    .map(|(path, count)| GrepCountResponse { path, count })
                    .collect();
                ("counts", format_grep_counts(&counts))
            }
        };
        let bounded = bound_text(text, max_bytes);
        Ok(Json(GrepResponse {
            repo: args.repo,
            pattern: args.pattern,
            mode,
            text: bounded.text,
            truncated: bounded.truncated,
            original_bytes: bounded.original_bytes,
            returned_bytes: bounded.returned_bytes,
            effective_max_count,
            matches,
            files,
            counts,
        }))
    }

    #[tool(
        name = "wit_sed",
        description = "Extract or transform one repository file using POSIX-style sed scripts"
    )]
    pub async fn wit_sed(
        &self,
        Parameters(args): Parameters<SedArgs>,
    ) -> Result<Json<SedResponse>, String> {
        let max_bytes = validate_text_limit(args.max_bytes)?;
        let scripts = collect_sed_scripts(&args)?;
        let repo = cache_github_repo(&args.repo, cache_mode(args.refresh_cache))
            .await
            .map_err(anyhow_error)?;
        let content =
            read_file_with_ignore(&repo, &args.path, &args.ignore).map_err(anyhow_error)?;
        let program = sed::parse_script(&scripts).map_err(anyhow_error)?;
        let output = sed::run(
            &program,
            &content,
            &sed::SedOptions {
                quiet: args.quiet,
                allow_file_io: false,
            },
        )
        .map_err(anyhow_error)?;
        let bounded = bound_text(output.output, max_bytes);
        Ok(Json(SedResponse {
            repo: args.repo,
            path: args.path,
            text: bounded.text,
            exit_code: output.exit_code,
            truncated: bounded.truncated,
            original_bytes: bounded.original_bytes,
            returned_bytes: bounded.returned_bytes,
        }))
    }

    #[tool(
        name = "wit_head",
        description = "Read the first N lines of a repository file"
    )]
    pub async fn wit_head(
        &self,
        Parameters(args): Parameters<HeadArgs>,
    ) -> Result<Json<TextResponse>, String> {
        let lines = args.lines.unwrap_or(10);
        let max_bytes = validate_text_limit(args.max_bytes)?;
        let repo = cache_github_repo(&args.repo, cache_mode(args.refresh_cache))
            .await
            .map_err(anyhow_error)?;
        let text = head_with_ignore(&repo, &args.path, lines, args.number, &args.ignore)
            .map_err(anyhow_error)?;
        Ok(Json(text_response(
            "wit_head",
            Some(args.repo),
            Some(args.path),
            text,
            max_bytes,
        )))
    }

    #[tool(
        name = "wit_tail",
        description = "Read the last N lines of a repository file, or from line N onward"
    )]
    pub async fn wit_tail(
        &self,
        Parameters(args): Parameters<TailArgs>,
    ) -> Result<Json<TextResponse>, String> {
        let lines = args.lines.unwrap_or(10);
        let max_bytes = validate_text_limit(args.max_bytes)?;
        let repo = cache_github_repo(&args.repo, cache_mode(args.refresh_cache))
            .await
            .map_err(anyhow_error)?;
        let text = tail_with_ignore(
            &repo,
            &args.path,
            lines,
            args.from_line,
            args.number,
            &args.ignore,
        )
        .map_err(anyhow_error)?;
        Ok(Json(text_response(
            "wit_tail",
            Some(args.repo),
            Some(args.path),
            text,
            max_bytes,
        )))
    }

    #[tool(
        name = "wit_skill_load",
        description = "Return the bundled wit agent skill markdown"
    )]
    pub async fn wit_skill_load(&self) -> Json<SkillResponse> {
        Json(SkillResponse {
            text: SKILL_MD.to_string(),
            bytes: SKILL_MD.len(),
        })
    }

    #[tool(
        name = "wit_skill_install",
        description = "Install the bundled wit agent skill as wit-skill/SKILL.md under a local directory"
    )]
    pub async fn wit_skill_install(
        &self,
        Parameters(args): Parameters<SkillInstallArgs>,
    ) -> Result<Json<SkillInstallResponse>, String> {
        let skill_dir = args.path.join("wit-skill");
        std::fs::create_dir_all(&skill_dir).map_err(|err| {
            format!(
                "failed to create skill directory '{}': {err}",
                skill_dir.display()
            )
        })?;
        let skill_path = skill_dir.join("SKILL.md");
        std::fs::write(&skill_path, SKILL_MD)
            .map_err(|err| format!("failed to write '{}': {err}", skill_path.display()))?;
        Ok(Json(SkillInstallResponse {
            path: skill_path.display().to_string(),
        }))
    }
}

#[prompt_router(router = "prompt_router")]
impl WitMcpServer {
    #[prompt(
        name = "wit_explore_repo",
        description = "Plan a coherent GitHub repository exploration using wit MCP tools"
    )]
    async fn wit_explore_repo(
        &self,
        Parameters(args): Parameters<ExploreRepoPromptArgs>,
    ) -> GetPromptResult {
        let objective = args
            .objective
            .unwrap_or_else(|| "understand the repository and answer the user's question".into());
        let focus = if args.focus_paths.is_empty() {
            "Start at the repository root.".to_string()
        } else {
            format!("Prioritize these paths: {}.", args.focus_paths.join(", "))
        };
        let ignore = if args.ignore.is_empty() {
            "Use ignore patterns only when generated, vendored, or binary-heavy paths are noisy."
                .to_string()
        } else {
            format!("Apply these ignore patterns: {}.", args.ignore.join(", "))
        };

        GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Use the wit MCP server to explore GitHub repo `{}`. Objective: {}. {} {} Recommended workflow: call `wit_tree` or `wit_ls` to orient, use `wit_rg` to locate relevant symbols or text, then read narrowly with `wit_head`, `wit_tail`, `wit_sed`, or `wit_cat`. Prefer bounded, cited snippets over dumping whole large files. Report which paths and lines support your answer.",
                args.repo, objective, focus, ignore
            ),
        )])
        .with_description(format!("Explore `{}` with wit", args.repo))
    }

    #[prompt(
        name = "wit_discover_repos",
        description = "Guide repository discovery with GitHub search qualifiers and follow-up exploration"
    )]
    async fn wit_discover_repos(
        &self,
        Parameters(args): Parameters<DiscoverReposPromptArgs>,
    ) -> GetPromptResult {
        let lang = args
            .lang
            .map(|lang| format!(" Language qualifier: `{lang}`."))
            .unwrap_or_default();
        let query = args
            .query
            .map(|query| format!(" Additional GitHub qualifiers: `{query}`."))
            .unwrap_or_default();

        GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Use `wit_search` to discover GitHub repositories for: {}.{}{} Keep the search limit modest first, compare stars/language/descriptions, then explore the strongest candidate with `wit_tree`, `wit_ls`, and `wit_rg` before reading files. Mention search qualifiers used and why each selected repo is relevant.",
                args.topic, lang, query
            ),
        )])
        .with_description(format!("Discover repositories for {}", args.topic))
    }

    #[prompt(
        name = "wit_read_precise",
        description = "Guide precise reading of one repository file without overloading context"
    )]
    async fn wit_read_precise(
        &self,
        Parameters(args): Parameters<ReadPrecisePromptArgs>,
    ) -> GetPromptResult {
        let objective = args
            .objective
            .unwrap_or_else(|| "extract only the relevant evidence".into());
        let range = args
            .line_range
            .map(|range| {
                format!(
                    "Known relevant line range: `{range}`. Call `wit_sed` with `quiet: true` and `script: \"{range}p\"` for that range."
                )
            })
            .unwrap_or_else(|| {
                "If line numbers are unknown, use `wit_rg` first, then `wit_head`, `wit_tail`, or `wit_sed`.".into()
            });

        GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Use the wit MCP server to read `{}` in `{}`. Objective: {}. {} Avoid full-file reads unless the file is small. Cite exact paths and line numbers when answering.",
                args.path, args.repo, objective, range
            ),
        )])
        .with_description(format!("Read `{}` precisely", args.path))
    }
}

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for WitMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("wit-mcp", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "Explore GitHub repositories through wit over MCP stdio. Start with wit_search for discovery, wit_tree or wit_ls for orientation, wit_rg for locating code, and wit_head/wit_tail/wit_sed/wit_cat for bounded reads. Use prompts and resources for workflow guidance.",
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![
                Resource::new("wit://skill/SKILL.md", "wit-skill")
                    .with_description("Bundled wit agent skill with workflow guidance")
                    .with_mime_type("text/markdown"),
                Resource::new("wit://guide/workflow", "wit-workflow")
                    .with_description("Concise workflow for exploring GitHub repos with wit MCP")
                    .with_mime_type("text/markdown"),
                Resource::new("wit://guide/tools", "wit-tools")
                    .with_description("MCP tool map for the wit CLI command surface")
                    .with_mime_type("text/markdown"),
            ],
            next_cursor: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = request.uri.as_str();
        let text = match uri {
            "wit://skill/SKILL.md" => SKILL_MD,
            "wit://guide/workflow" => WIT_WORKFLOW_GUIDE,
            "wit://guide/tools" => WIT_TOOLS_GUIDE,
            _ => {
                return Err(McpError::resource_not_found(
                    "resource_not_found",
                    Some(json!({ "uri": request.uri })),
                ));
            }
        };
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, request.uri).with_mime_type("text/markdown"),
        ]))
    }
}

const WIT_WORKFLOW_GUIDE: &str = r#"# wit MCP workflow

1. Use `wit_search` when the repository is unknown. Keep `limit` small, then refine with GitHub qualifiers in `query`.
2. Use `wit_tree` or `wit_ls` to orient before reading. Add `long` to estimate file size.
3. Use `wit_rg` to locate symbols, text, filenames, or likely implementation areas. Use `glob` and `ignore` to reduce noise.
4. Use `wit_head`, `wit_tail`, or `wit_sed` for precise reads. Use `wit_cat` for small-to-medium files.
5. `wit_sed` runs in MCP-safe mode: local sed file I/O commands are disabled and script files are rejected. Use inline `script` or `expressions`.
6. Use `refresh_cache` only when fresh default-branch content matters. Normal reads use wit's stale-while-revalidate cache.
7. Fetch `wit://skill/SKILL.md` when an agent needs the full reusable skill guidance.
"#;

const WIT_TOOLS_GUIDE: &str = r#"# wit MCP tools

- `wit_search`: GitHub repository discovery.
- `wit_cache_refresh`: force-refresh a repo cache.
- `wit_tree`: recursive tree for a repo or subtree.
- `wit_ls`: one-level directory listing.
- `wit_cat`: file read with cat-style display flags.
- `wit_rg`: ripgrep-style content search.
- `wit_sed`: POSIX-style sed extraction or transformation on one repository file. MCP disables sed local file I/O and local script files.
- `wit_head`: first N lines of a file.
- `wit_tail`: last N lines or from line N onward.
- `wit_skill_load`: return the bundled wit skill.
- `wit_skill_install`: install or overwrite the bundled wit skill under a local directory.

All repo-reading tools accept `repo`, optional `refresh_cache`, and optional `ignore` patterns. Public branch selection and TTL/max-age controls are intentionally not exposed.
"#;

fn cache_mode(refresh_cache: bool) -> CacheAcquisitionMode {
    if refresh_cache {
        CacheAcquisitionMode::ForceInvalidate
    } else {
        CacheAcquisitionMode::ServeStaleAndRevalidate
    }
}

fn validate_search_limit(limit: Option<usize>) -> Result<usize, String> {
    let limit = limit.unwrap_or(DEFAULT_GITHUB_REPO_LIMIT);
    if !(1..=MAX_GITHUB_REPOS).contains(&limit) {
        return Err(format!("limit must be between 1 and {MAX_GITHUB_REPOS}"));
    }
    Ok(limit)
}

fn validate_limit(
    value: Option<usize>,
    default: usize,
    max: usize,
    name: &str,
) -> Result<usize, String> {
    let value = value.unwrap_or(default);
    if value > max {
        return Err(format!("{name} must be <= {max}"));
    }
    Ok(value)
}

fn validate_text_limit(value: Option<usize>) -> Result<usize, String> {
    validate_limit(value, DEFAULT_TEXT_BYTES, MAX_TEXT_BYTES, "max_bytes")
}

fn validate_context(value: usize, name: &str) -> Result<(), String> {
    if value > MAX_CONTEXT_LINES {
        return Err(format!("{name} must be <= {MAX_CONTEXT_LINES}"));
    }
    Ok(())
}

fn anyhow_error(err: anyhow::Error) -> String {
    err.to_string()
}

fn github_error(err: anyhow::Error) -> String {
    let lower = err.to_string().to_lowercase();
    if lower.contains("401")
        || lower.contains("unauthorized")
        || lower.contains("403")
        || lower.contains("forbidden")
    {
        return format!(
            "{err}. GitHub rejected the request; set GITHUB_TOKEN for authenticated access and higher rate limits."
        );
    }
    if lower.contains("429") || lower.contains("rate limit") {
        return format!("{err}. GitHub API rate limit exceeded; retry later or set GITHUB_TOKEN.");
    }
    err.to_string()
}

struct BoundedText {
    text: String,
    truncated: bool,
    original_bytes: usize,
    returned_bytes: usize,
}

fn bound_text(text: String, max_bytes: usize) -> BoundedText {
    let original_bytes = text.len();
    if original_bytes <= max_bytes {
        return BoundedText {
            returned_bytes: original_bytes,
            text,
            truncated: false,
            original_bytes,
        };
    }

    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = text[..end].to_string();
    BoundedText {
        returned_bytes: truncated.len(),
        text: truncated,
        truncated: true,
        original_bytes,
    }
}

fn text_response(
    command: &'static str,
    repo: Option<String>,
    path: Option<String>,
    text: String,
    max_bytes: usize,
) -> TextResponse {
    let bounded = bound_text(text, max_bytes);
    TextResponse {
        command,
        repo,
        path,
        text: bounded.text,
        truncated: bounded.truncated,
        original_bytes: bounded.original_bytes,
        returned_bytes: bounded.returned_bytes,
    }
}

fn format_ls_text(entries: &[LsEntry], long: bool) -> String {
    if entries.is_empty() {
        return "Directory is empty or does not exist.".to_string();
    }
    entries
        .iter()
        .map(|entry| {
            if entry.kind == "directory" {
                format!("{}/", entry.name)
            } else if long {
                if entry.is_binary {
                    format!("[bin] {}", entry.name)
                } else if let Some(lines) = entry.lines {
                    format!(
                        "{:>6} ln  {:<30} (~{} tok)",
                        lines,
                        entry.name,
                        entry.approx_tokens.unwrap_or(lines * 5)
                    )
                } else {
                    entry.name.clone()
                }
            } else {
                entry.name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_cat(content: &str, args: &CatArgs) -> String {
    let show_ends = args.show_ends || args.show_all;
    let show_tabs = args.show_tabs || args.show_all;
    let number_lines = args.number && !args.number_nonblank;
    let mut line_num = 0usize;
    let mut prev_blank = false;
    let mut output_lines = Vec::new();

    for line in content.lines() {
        let is_blank = line.is_empty();
        if args.squeeze_blank && is_blank && prev_blank {
            continue;
        }
        prev_blank = is_blank;

        let mut output = line.to_string();
        if show_tabs {
            output = output.replace('\t', "^I");
        }
        if show_ends {
            output.push('$');
        }
        if number_lines || (args.number_nonblank && !is_blank) {
            line_num += 1;
            output_lines.push(format!("{:>6}  {output}", line_num));
        } else if args.number_nonblank && is_blank {
            output_lines.push(format!("{:>6}  {output}", ""));
        } else {
            output_lines.push(output);
        }
    }

    output_lines.join("\n")
}

fn format_grep_matches(matches: &[GrepMatchResponse]) -> String {
    matches
        .iter()
        .map(|item| {
            if item.line_number == 0 && item.content == "--" {
                "--".to_string()
            } else if item.is_context {
                format!("{}-{}-{}", item.path, item.line_number, item.content)
            } else {
                format!("{}:{}:{}", item.path, item.line_number, item.content)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_grep_files(files: &[GrepFileResponse]) -> String {
    files
        .iter()
        .map(|item| {
            if let Some(lines) = item.lines {
                format!(
                    "{:>6} ln  {:<40} (~{} tok)",
                    lines,
                    item.path,
                    item.approx_tokens.unwrap_or(lines * 5)
                )
            } else {
                item.path.clone()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_grep_counts(counts: &[GrepCountResponse]) -> String {
    counts
        .iter()
        .map(|item| format!("{}:{}", item.path, item.count))
        .collect::<Vec<_>>()
        .join("\n")
}

fn collect_sed_scripts(args: &SedArgs) -> Result<Vec<String>, String> {
    if !args.script_files.is_empty() {
        return Err(
            "script_files are not available over MCP; pass script or expressions inline"
                .to_string(),
        );
    }
    let mut scripts = args.expressions.clone();
    if let Some(script) = &args.script {
        scripts.push(script.clone());
    }
    if scripts.is_empty() {
        return Err("missing sed script; provide script or expressions".to_string());
    }
    Ok(scripts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_truncation_keeps_utf8_boundary() {
        let bounded = bound_text("aébc".to_string(), 2);
        assert_eq!(bounded.text, "a");
        assert!(bounded.truncated);
        assert_eq!(bounded.original_bytes, 5);
        assert_eq!(bounded.returned_bytes, 1);
    }

    #[test]
    fn cat_format_matches_number_nonblank_override() {
        let args = CatArgs {
            number: true,
            number_nonblank: true,
            ..CatArgs::default()
        };
        assert_eq!(
            format_cat("a\n\nb\n", &args),
            "     1  a\n        \n     2  b"
        );
    }

    #[test]
    fn rg_default_matches_cli_unlimited_and_zero_is_preserved() {
        let unbounded = RgArgs {
            repo: "owner/repo".to_string(),
            pattern: "needle".to_string(),
            ..RgArgs::default()
        };
        assert_eq!(unbounded.max_count, None);

        let zero = RgArgs {
            max_count: Some(0),
            ..unbounded
        };
        assert_eq!(zero.max_count, Some(0));
    }

    #[test]
    fn mcp_sed_rejects_local_script_files() {
        let args = SedArgs {
            repo: "owner/repo".to_string(),
            path: "README.md".to_string(),
            script_files: vec!["/tmp/script.sed".to_string()],
            ..SedArgs::default()
        };
        assert!(collect_sed_scripts(&args).is_err());
    }

    #[test]
    fn mcp_sed_safe_mode_rejects_local_file_io_commands() {
        let program = sed::parse_script(&["r /etc/passwd".to_string()]).unwrap();
        let err = sed::run(
            &program,
            "line\n",
            &sed::SedOptions {
                quiet: true,
                allow_file_io: false,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("sed local file I/O commands are disabled"));

        let program = sed::parse_script(&["w /tmp/wit-mcp-out".to_string()]).unwrap();
        let err = sed::run(
            &program,
            "line\n",
            &sed::SedOptions {
                quiet: true,
                allow_file_io: false,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("sed local file I/O commands are disabled"));
    }

    #[test]
    fn server_exposes_complete_tool_prompt_resource_surface() {
        let server = WitMcpServer::new();
        let tools = server.tool_router.list_all();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert_eq!(
            names,
            vec![
                "wit_cache_refresh",
                "wit_cat",
                "wit_head",
                "wit_ls",
                "wit_rg",
                "wit_search",
                "wit_sed",
                "wit_skill_install",
                "wit_skill_load",
                "wit_tail",
                "wit_tree",
            ]
        );

        let prompts = server.prompt_router.list_all();
        let prompt_names: Vec<&str> = prompts.iter().map(|prompt| prompt.name.as_ref()).collect();
        assert_eq!(
            prompt_names,
            vec!["wit_discover_repos", "wit_explore_repo", "wit_read_precise"]
        );
    }
}
