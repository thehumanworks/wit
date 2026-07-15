use crate::{
    ensure_rustls_provider,
    gitops::ops::{CacheAcquisitionMode, CacheBranchSelection, cache_github_repo},
    search::{GitHubSearchClient, MAX_GITHUB_REPOS},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;
use rmcp::schemars::JsonSchema;
use rmcp::{
    ErrorData as McpError, Json, RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        Implementation, ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
        ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};
use tempfile::TempDir;

const SKILL_MD: &str = include_str!("skill/SKILL.md");
const DEFAULT_BUDGET_BYTES: usize = 64 * 1024;
const MIN_BUDGET_BYTES: usize = 1024;
const MAX_BUDGET_BYTES: usize = 256 * 1024;
const DEFAULT_PAGE_ITEMS: usize = 100;
const MAX_PAGE_ITEMS: usize = 1000;
const DEFAULT_LIST_DEPTH: usize = 2;
const MAX_LIST_DEPTH: usize = 32;
const DEFAULT_CONTEXT_LINES: usize = 4;
const MAX_CONTEXT_LINES: usize = 100;
const DEFAULT_CONTEXT_RESULTS: usize = 20;
const MAX_CONTEXT_RESULTS: usize = 100;
const MAX_CONTEXT_CANDIDATES: usize = 5000;

#[derive(Clone)]
pub struct WitMcpServer {
    tool_router: ToolRouter<Self>,
    snapshots: Arc<Mutex<HashMap<String, SnapshotRecord>>>,
}

impl WitMcpServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            snapshots: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn snapshot(&self, snapshot_id: &str) -> Result<SnapshotHandle, String> {
        let snapshots = self
            .snapshots
            .lock()
            .map_err(|_| "snapshot registry lock was poisoned".to_string())?;
        snapshots
            .get(snapshot_id)
            .map(SnapshotRecord::handle)
            .ok_or_else(|| {
                format!(
                    "unknown or expired snapshot_id '{snapshot_id}'; call wit_open in this server session"
                )
            })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for WitMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("wit-mcp", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "wit MCP v2 is snapshot-first and structured-first. Call wit_open once, reuse snapshot_id for immutable reads, use wit_list for structure, wit_search_code for exact matches, wit_read for explicit line ranges, and wit_context for deterministic multi-file evidence. Collection tools are byte-bounded and return next_cursor when has_more is true. Fetch wit://skill/SKILL.md for the full workflow.",
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
                    .with_description("Bundled snapshot-first wit agent skill")
                    .with_mime_type("text/markdown"),
                Resource::new("wit://guide/workflow", "wit-workflow-v2")
                    .with_description("Concise agent-native MCP v2 workflow")
                    .with_mime_type("text/markdown"),
                Resource::new("wit://guide/tools", "wit-tools-v2")
                    .with_description("MCP v2 semantic tool and pagination contracts")
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
        let text = match request.uri.as_str() {
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

const WIT_WORKFLOW_GUIDE: &str = r#"# wit MCP v2 workflow

1. If `owner/repo` is unknown, call `wit_find_repositories` with narrow GitHub qualifiers.
2. Call `wit_refs` when branch or tag discovery matters, then call `wit_open`. Reuse the returned immutable `snapshot_id` throughout the task.
3. Call `wit_list` to orient with an explicit depth, or `wit_search_code` when symbols or text are known.
4. Call `wit_read` with explicit one-based line ranges for precise evidence.
5. Call `wit_context` when one deterministic operation should rank and merge evidence across files. It does not invoke a model or embeddings.
6. When `has_more` is true, pass `next_cursor` back with otherwise unchanged arguments. Changed arguments or snapshots invalidate a cursor.
7. Responses are structured by default and bounded to 64 KiB. Set `include_rendered_text` only for compatibility with text-oriented consumers.
8. MCP v1 remains available with `wit mcp --compat-v1` or `wit-mcp --compat-v1` during the 0.1 release line.
"#;

const WIT_TOOLS_GUIDE: &str = r#"# wit MCP v2 tools

- `wit_find_repositories`: discover owner/repo when unknown.
- `wit_refs`: discover default branch, branches, and tags.
- `wit_open`: pin a default branch, named branch, tag, or full commit SHA into an immutable server-lifetime snapshot.
- `wit_list`: bounded structure listing with explicit depth.
- `wit_search_code`: bounded multi-query regex search with atomic context groups and provenance.
- `wit_read`: explicit one-based inclusive line-range read with provenance.
- `wit_context`: deterministic ranked and merged multi-file evidence.

Collection responses use `items`, `returned_items`, `has_more`, `next_cursor`, and whole-structured-response `budget` metadata. Cursors are opaque and bound to the tool, snapshot, and normalized query. Default responses are at most 64 KiB; the fixed MCP framing outside structured content is not included and is constrained to less than 1 KiB.

The legacy Unix-shaped MCP v1 tools are deprecated as the recommended agent surface but remain supported in explicit compatibility mode throughout the 0.1 release line. Human CLI commands are unchanged.
"#;

#[derive(Debug)]
struct ResolvedRef {
    kind: ResolvedRefKind,
    resolved_ref: String,
    fetch_ref: String,
    commit_sha: Option<String>,
}

#[derive(Debug)]
enum ResolvedRefKind {
    DefaultBranch(String),
    Branch(String),
    Tag,
    Commit,
}

impl ResolvedRefKind {
    fn label(&self) -> &'static str {
        match self {
            Self::DefaultBranch(_) | Self::Branch(_) => "branch",
            Self::Tag => "tag",
            Self::Commit => "commit",
        }
    }
}

#[derive(Debug)]
struct RemoteRefs {
    default_branch: String,
    branches: BTreeMap<String, String>,
    tags: BTreeMap<String, String>,
}

#[derive(Debug)]
struct TreeEntry {
    kind: String,
    oid: String,
    size: Option<u64>,
    path: String,
}

fn validate_repo(repo: &str) -> Result<(), String> {
    let mut parts = repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if owner.is_empty()
        || name.is_empty()
        || parts.next().is_some()
        || !owner
            .chars()
            .chain(name.chars())
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err("repo must be in owner/repo form using GitHub-safe characters".to_string());
    }
    Ok(())
}

fn github_remote_url(repo: &str) -> String {
    format!("https://github.com/{repo}")
}

fn snapshot_id(repo: &str, commit_sha: &str) -> String {
    format!("{repo}@{commit_sha}")
}

fn resolve_requested_ref(remote: &str, requested: &str) -> Result<ResolvedRef, String> {
    let requested = requested.trim();
    if requested.is_empty() || requested == "HEAD" {
        let refs = query_remote_refs(remote)?;
        let sha = refs
            .branches
            .get(&refs.default_branch)
            .cloned()
            .ok_or_else(|| "remote default branch was not listed".to_string())?;
        return Ok(ResolvedRef {
            kind: ResolvedRefKind::DefaultBranch(refs.default_branch.clone()),
            resolved_ref: format!("refs/heads/{}", refs.default_branch),
            fetch_ref: "HEAD".to_string(),
            commit_sha: Some(sha),
        });
    }
    if is_full_sha(requested) {
        return Ok(ResolvedRef {
            kind: ResolvedRefKind::Commit,
            resolved_ref: requested.to_ascii_lowercase(),
            fetch_ref: requested.to_ascii_lowercase(),
            commit_sha: Some(requested.to_ascii_lowercase()),
        });
    }

    let refs = query_remote_refs(remote)?;
    let branch_name = requested.strip_prefix("refs/heads/").unwrap_or(requested);
    if let Some(sha) = refs.branches.get(branch_name) {
        return Ok(ResolvedRef {
            kind: ResolvedRefKind::Branch(branch_name.to_string()),
            resolved_ref: format!("refs/heads/{branch_name}"),
            fetch_ref: format!("refs/heads/{branch_name}"),
            commit_sha: Some(sha.clone()),
        });
    }
    let tag_name = requested.strip_prefix("refs/tags/").unwrap_or(requested);
    if let Some(sha) = refs.tags.get(tag_name) {
        return Ok(ResolvedRef {
            kind: ResolvedRefKind::Tag,
            resolved_ref: format!("refs/tags/{tag_name}"),
            fetch_ref: format!("refs/tags/{tag_name}"),
            commit_sha: Some(sha.clone()),
        });
    }
    Err(format!(
        "ref '{requested}' was not found as a branch or tag; full 40-character commit SHAs are also accepted"
    ))
}

fn is_full_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn query_remote_refs(remote: &str) -> Result<RemoteRefs, String> {
    let output = Command::new("git")
        .args([
            "ls-remote",
            "--symref",
            remote,
            "HEAD",
            "refs/heads/*",
            "refs/tags/*",
        ])
        .output()
        .map_err(|err| format!("failed to run git ls-remote: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-remote failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| "git ls-remote returned non-UTF-8 output".to_string())?;
    let mut default_branch = None;
    let mut branches = BTreeMap::new();
    let mut raw_tags = BTreeMap::new();
    let mut peeled_tags = BTreeMap::new();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("ref: refs/heads/") {
            if let Some((branch, target)) = rest.split_once('\t')
                && target == "HEAD"
            {
                default_branch = Some(branch.to_string());
            }
            continue;
        }
        let Some((sha, reference)) = line.split_once('\t') else {
            continue;
        };
        if let Some(branch) = reference.strip_prefix("refs/heads/") {
            branches.insert(branch.to_string(), sha.to_string());
        } else if let Some(tag) = reference.strip_prefix("refs/tags/") {
            if let Some(tag) = tag.strip_suffix("^{}") {
                peeled_tags.insert(tag.to_string(), sha.to_string());
            } else {
                raw_tags.insert(tag.to_string(), sha.to_string());
            }
        }
    }
    for (tag, sha) in peeled_tags {
        raw_tags.insert(tag, sha);
    }
    let default_branch = default_branch
        .ok_or_else(|| "remote HEAD did not resolve to refs/heads/<branch>".to_string())?;
    Ok(RemoteRefs {
        default_branch,
        branches,
        tags: raw_tags,
    })
}

fn list_remote_refs(repo: &str, remote: &str) -> Result<Vec<RefItem>, String> {
    let refs = query_remote_refs(remote)?;
    let mut items = Vec::with_capacity(refs.branches.len() + refs.tags.len());
    for (name, commit_sha) in refs.branches {
        items.push(RefItem {
            repo: repo.to_string(),
            resolved_ref: format!("refs/heads/{name}"),
            is_default: name == refs.default_branch,
            name,
            kind: "branch".to_string(),
            commit_sha,
        });
    }
    for (name, commit_sha) in refs.tags {
        items.push(RefItem {
            repo: repo.to_string(),
            resolved_ref: format!("refs/tags/{name}"),
            is_default: false,
            name,
            kind: "tag".to_string(),
            commit_sha,
        });
    }
    Ok(items)
}

fn clone_snapshot(repo: &str, source: &Path, expected_sha: &str) -> Result<SnapshotRecord, String> {
    let temp_dir = tempfile::Builder::new()
        .prefix("wit-snapshot-")
        .tempdir()
        .map_err(|err| format!("failed to create snapshot directory: {err}"))?;
    let repo_path = temp_dir.path().join("repo.git");
    let output = Command::new("git")
        .args(["clone", "--bare", "--no-hardlinks"])
        .arg(source)
        .arg(&repo_path)
        .output()
        .map_err(|err| format!("failed to clone immutable snapshot: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to clone immutable snapshot: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let actual = git_stdout(
        &repo_path,
        &["rev-parse", "HEAD^{commit}"],
        "verify cloned snapshot",
    )?;
    if actual != expected_sha {
        return Err(format!(
            "snapshot changed while being pinned: expected {expected_sha}, cloned {actual}; retry wit_open"
        ));
    }
    Ok(SnapshotRecord {
        _temp_dir: temp_dir,
        repo_path,
        repo: repo.to_string(),
        commit_sha: actual,
    })
}

fn fetch_snapshot(
    repo: &str,
    remote: &str,
    fetch_ref: &str,
    resolved_ref: &str,
) -> Result<(SnapshotRecord, String), String> {
    let temp_dir = tempfile::Builder::new()
        .prefix("wit-snapshot-")
        .tempdir()
        .map_err(|err| format!("failed to create snapshot directory: {err}"))?;
    let repo_path = temp_dir.path().join("repo.git");
    run_git(
        None,
        &["init", "--bare", repo_path.to_string_lossy().as_ref()],
        "initialize snapshot",
    )?;
    run_git(
        Some(&repo_path),
        &["fetch", "--depth", "1", remote, fetch_ref],
        "fetch snapshot ref",
    )?;
    let commit_sha = git_stdout(
        &repo_path,
        &["rev-parse", "FETCH_HEAD^{commit}"],
        "resolve fetched snapshot commit",
    )?;
    if is_full_sha(resolved_ref) && commit_sha != resolved_ref {
        return Err(format!(
            "remote resolved commit {commit_sha}, not requested commit {resolved_ref}"
        ));
    }
    run_git(
        Some(&repo_path),
        &["update-ref", "refs/heads/snapshot", &commit_sha],
        "pin snapshot ref",
    )?;
    run_git(
        Some(&repo_path),
        &["symbolic-ref", "HEAD", "refs/heads/snapshot"],
        "set snapshot HEAD",
    )?;
    Ok((
        SnapshotRecord {
            _temp_dir: temp_dir,
            repo_path,
            repo: repo.to_string(),
            commit_sha: commit_sha.clone(),
        },
        commit_sha,
    ))
}

#[derive(Deserialize)]
struct CacheMetadataView {
    last_checked_at: Option<u64>,
    last_updated_at: Option<u64>,
    last_error: Option<String>,
}

fn cache_provenance(repo_path: &Path, freshness: Freshness) -> CacheProvenance {
    let metadata = repo_path
        .parent()
        .and_then(|parent| std::fs::read(parent.join("metadata.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<CacheMetadataView>(&bytes).ok());
    let state = match freshness {
        Freshness::RequireFresh => "explicitly_refreshed",
        Freshness::AllowStale
            if metadata
                .as_ref()
                .and_then(|value| value.last_error.as_ref())
                .is_some() =>
        {
            "stale_with_error"
        }
        Freshness::AllowStale => "stale_served_revalidating",
    };
    CacheProvenance {
        state: state.to_string(),
        last_checked_at: metadata.as_ref().and_then(|value| value.last_checked_at),
        last_updated_at: metadata.as_ref().and_then(|value| value.last_updated_at),
        last_error: metadata.and_then(|value| value.last_error),
    }
}

fn run_git(repo_path: Option<&Path>, args: &[&str], action: &str) -> Result<(), String> {
    let mut command = Command::new("git");
    if let Some(repo_path) = repo_path {
        command.arg("-C").arg(repo_path);
    }
    let output = command
        .args(args)
        .output()
        .map_err(|err| format!("failed to {action}: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to {action}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn git_stdout(repo_path: &Path, args: &[&str], action: &str) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .output()
        .map_err(|err| format!("failed to {action}: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to {action}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|_| format!("failed to {action}: git returned non-UTF-8 output"))
}

fn normalize_repo_path(path: &str) -> Result<String, String> {
    let normalized = path.trim_matches('/');
    if normalized
        .split('/')
        .any(|part| part == ".." || part == ".")
        || path.starts_with('/')
        || path.contains('\0')
    {
        return Err(
            "path must be repository-relative and cannot contain . or .. components".to_string(),
        );
    }
    Ok(normalized.to_string())
}

fn walk_tree(
    snapshot: &SnapshotHandle,
    mut visit: impl FnMut(&TreeEntry) -> Result<bool, String>,
) -> Result<(), String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&snapshot.repo_path)
        .args(["ls-tree", "-r", "-t", "-z", "-l", "HEAD"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to list snapshot tree: {err}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture git ls-tree output".to_string())?;
    let mut reader = BufReader::new(stdout);
    let mut record = Vec::new();
    let mut active = true;
    loop {
        record.clear();
        let read = reader
            .read_until(0, &mut record)
            .map_err(|err| format!("failed to read snapshot tree: {err}"))?;
        if read == 0 {
            break;
        }
        if record.last() == Some(&0) {
            record.pop();
        }
        if !active {
            continue;
        }
        let text = std::str::from_utf8(&record)
            .map_err(|_| "snapshot tree contains a non-UTF-8 path".to_string())?;
        let (metadata, path) = text
            .split_once('\t')
            .ok_or_else(|| format!("unexpected git ls-tree record: {text}"))?;
        let fields = metadata.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 4 {
            return Err(format!("unexpected git ls-tree metadata: {metadata}"));
        }
        let entry = TreeEntry {
            kind: fields[1].to_string(),
            oid: fields[2].to_string(),
            size: fields[3].parse().ok(),
            path: path.to_string(),
        };
        active = visit(&entry)?;
    }
    let output = child
        .wait_with_output()
        .map_err(|err| format!("failed to finish snapshot tree listing: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to list snapshot tree: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn relative_depth(path: &str, base_path: &str) -> Option<usize> {
    let relative = if base_path.is_empty() {
        path
    } else if path == base_path {
        return Some(0);
    } else {
        path.strip_prefix(&format!("{base_path}/"))?
    };
    Some(relative.split('/').count())
}

fn list_snapshot_window(
    snapshot: &SnapshotHandle,
    base_path: &str,
    depth: usize,
    include_metadata: bool,
    offset: usize,
    limit: usize,
) -> Result<Vec<ListItem>, String> {
    let mut seen = 0usize;
    let mut items = Vec::with_capacity(limit.min(MAX_PAGE_ITEMS + 1));
    walk_tree(snapshot, |entry| {
        let Some(entry_depth) = relative_depth(&entry.path, base_path) else {
            return Ok(true);
        };
        if entry_depth == 0 || entry_depth > depth {
            return Ok(true);
        }
        if seen < offset {
            seen += 1;
            return Ok(true);
        }
        if items.len() >= limit {
            return Ok(false);
        }
        let lines = if include_metadata && entry.kind == "blob" {
            blob_text(snapshot, &entry.oid, 4 * 1024 * 1024)
                .ok()
                .map(|text| text.lines().count())
        } else {
            None
        };
        items.push(ListItem {
            snapshot_id: snapshot.snapshot_id.clone(),
            repo: snapshot.repo.clone(),
            commit_sha: snapshot.commit_sha.clone(),
            path: entry.path.clone(),
            kind: if entry.kind == "tree" {
                "directory".to_string()
            } else {
                "file".to_string()
            },
            blob_sha: (entry.kind == "blob").then(|| entry.oid.clone()),
            size_bytes: include_metadata.then_some(entry.size).flatten(),
            lines,
        });
        seen += 1;
        Ok(true)
    })?;
    Ok(items)
}

fn compile_queries(queries: &[String]) -> Result<Vec<(String, Regex)>, String> {
    if queries.is_empty() {
        return Err("queries must contain at least one regular expression".to_string());
    }
    if queries.len() > 20 {
        return Err("queries must contain at most 20 regular expressions".to_string());
    }
    queries
        .iter()
        .map(|query| {
            if query.is_empty() {
                return Err("queries cannot contain an empty expression".to_string());
            }
            Regex::new(query)
                .map(|regex| (query.clone(), regex))
                .map_err(|err| format!("invalid query regex '{query}': {err}"))
        })
        .collect()
}

fn compile_globs(globs: &[String]) -> Result<Option<GlobSet>, String> {
    if globs.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for glob in globs {
        builder.add(Glob::new(glob).map_err(|err| format!("invalid glob '{glob}': {err}"))?);
    }
    builder
        .build()
        .map(Some)
        .map_err(|err| format!("failed to compile globs: {err}"))
}

fn search_snapshot_window(
    snapshot: &SnapshotHandle,
    queries: &[(String, Regex)],
    globs: &Option<GlobSet>,
    context_lines: usize,
    offset: usize,
    limit: usize,
) -> Result<Vec<SearchItem>, String> {
    let mut seen = 0usize;
    let mut items = Vec::with_capacity(limit.min(MAX_CONTEXT_CANDIDATES));
    walk_tree(snapshot, |entry| {
        if entry.kind != "blob" || globs.as_ref().is_some_and(|set| !set.is_match(&entry.path)) {
            return Ok(true);
        }
        let Some(size) = entry.size else {
            return Ok(true);
        };
        if size > 4 * 1024 * 1024 {
            return Ok(true);
        }
        let Ok(text) = blob_text(snapshot, &entry.oid, 4 * 1024 * 1024) else {
            return Ok(true);
        };
        let lines = text.lines().collect::<Vec<_>>();
        for (line_index, line) in lines.iter().enumerate() {
            for (query, regex) in queries {
                if !regex.is_match(line) {
                    continue;
                }
                if seen < offset {
                    seen += 1;
                    continue;
                }
                if items.len() >= limit {
                    return Ok(false);
                }
                let match_line = line_index + 1;
                let start_line = match_line.saturating_sub(context_lines).max(1);
                let end_line = (match_line + context_lines).min(lines.len());
                let source_lines = (start_line..=end_line)
                    .map(|line_number| SourceLine {
                        line_number,
                        text: lines[line_number - 1].to_string(),
                    })
                    .collect();
                items.push(SearchItem {
                    snapshot_id: snapshot.snapshot_id.clone(),
                    repo: snapshot.repo.clone(),
                    commit_sha: snapshot.commit_sha.clone(),
                    path: entry.path.clone(),
                    blob_sha: entry.oid.clone(),
                    query: query.clone(),
                    match_line,
                    start_line,
                    end_line,
                    lines: source_lines,
                });
                seen += 1;
            }
        }
        Ok(true)
    })?;
    Ok(items)
}

fn read_snapshot_window(
    snapshot: &SnapshotHandle,
    path: &str,
    requested_start: usize,
    requested_end: Option<usize>,
    offset: usize,
    limit: usize,
) -> Result<Vec<ReadLineItem>, String> {
    let oid = git_stdout(
        &snapshot.repo_path,
        &["rev-parse", &format!("HEAD:{path}")],
        "resolve file blob",
    )?;
    let text = blob_text(snapshot, &oid, 16 * 1024 * 1024)?;
    let lines = text.lines().collect::<Vec<_>>();
    let end = requested_end.unwrap_or(lines.len()).min(lines.len());
    if requested_start > lines.len().saturating_add(1) {
        return Err(format!(
            "start_line {requested_start} exceeds file length {}",
            lines.len()
        ));
    }
    let page_start = requested_start.saturating_add(offset);
    if page_start > end {
        return Ok(Vec::new());
    }
    Ok((page_start..=end)
        .take(limit)
        .map(|line_number| ReadLineItem {
            snapshot_id: snapshot.snapshot_id.clone(),
            repo: snapshot.repo.clone(),
            commit_sha: snapshot.commit_sha.clone(),
            path: path.to_string(),
            blob_sha: oid.clone(),
            start_line: line_number,
            end_line: line_number,
            text: lines[line_number - 1].to_string(),
        })
        .collect())
}

fn blob_text(snapshot: &SnapshotHandle, oid: &str, max_bytes: usize) -> Result<String, String> {
    let size = git_stdout(
        &snapshot.repo_path,
        &["cat-file", "-s", oid],
        "read blob size",
    )?
    .parse::<usize>()
    .map_err(|_| "git cat-file returned an invalid blob size".to_string())?;
    if size > max_bytes {
        return Err(format!(
            "blob {oid} is {size} bytes, above this operation's {max_bytes}-byte safety limit"
        ));
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(&snapshot.repo_path)
        .args(["cat-file", "blob", oid])
        .output()
        .map_err(|err| format!("failed to read blob {oid}: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to read blob {oid}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if output.stdout.contains(&0) {
        return Err(format!("blob {oid} is binary"));
    }
    String::from_utf8(output.stdout).map_err(|_| format!("blob {oid} is not valid UTF-8"))
}

fn rank_context(mut matches: Vec<SearchItem>, queries: &[String]) -> Vec<ContextItem> {
    matches.sort_by(|left, right| {
        (
            &left.path,
            left.start_line,
            left.end_line,
            left.match_line,
            &left.query,
        )
            .cmp(&(
                &right.path,
                right.start_line,
                right.end_line,
                right.match_line,
                &right.query,
            ))
    });
    let mut merged: Vec<ContextItem> = Vec::new();
    for item in matches {
        if let Some(previous) = merged.last_mut()
            && previous.path == item.path
            && previous.blob_sha == item.blob_sha
            && item.start_line <= previous.end_line.saturating_add(1)
        {
            previous.end_line = previous.end_line.max(item.end_line);
            previous.score += 1;
            if !previous.queries.contains(&item.query) {
                previous.queries.push(item.query);
            }
            let mut lines = previous
                .lines
                .iter()
                .cloned()
                .map(|line| (line.line_number, line))
                .collect::<BTreeMap<_, _>>();
            for line in item.lines {
                lines.entry(line.line_number).or_insert(line);
            }
            previous.lines = lines.into_values().collect();
            continue;
        }
        merged.push(ContextItem {
            snapshot_id: item.snapshot_id,
            repo: item.repo,
            commit_sha: item.commit_sha,
            path: item.path,
            blob_sha: item.blob_sha,
            start_line: item.start_line,
            end_line: item.end_line,
            score: 1,
            ranking_reasons: Vec::new(),
            queries: vec![item.query],
            lines: item.lines,
        });
    }

    for item in &mut merged {
        let hit_count = item.score;
        let exact_hits = queries
            .iter()
            .filter(|query| {
                item.lines
                    .iter()
                    .any(|line| line.text.contains(query.as_str()))
            })
            .count() as i64;
        let path_hits = queries
            .iter()
            .filter(|query| {
                item.path
                    .to_ascii_lowercase()
                    .contains(&query.to_ascii_lowercase())
            })
            .count() as i64;
        let position_bonus = 20_i64.saturating_sub((item.start_line / 100) as i64);
        item.score = exact_hits * 100 + path_hits * 25 + hit_count * 10 + position_bonus;
        if exact_hits > 0 {
            item.ranking_reasons
                .push(format!("{exact_hits} exact query text hit(s)"));
        }
        if path_hits > 0 {
            item.ranking_reasons
                .push(format!("{path_hits} path relevance hit(s)"));
        }
        item.ranking_reasons
            .push(format!("{hit_count} match(es) in merged window"));
        item.ranking_reasons.push(format!(
            "source position starts at line {}",
            item.start_line
        ));
        item.queries.sort();
    }
    merged.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.start_line.cmp(&right.start_line))
    });
    merged
}

fn render_search_items(items: &[SearchItem]) -> String {
    items
        .iter()
        .map(|item| {
            let lines = item
                .lines
                .iter()
                .map(|line| format!("{}: {}", line.line_number, line.text))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{}:{} [{}]\n{}",
                item.path, item.match_line, item.query, lines
            )
        })
        .collect::<Vec<_>>()
        .join("\n--\n")
}

fn render_read_items(items: &[ReadLineItem], number_lines: bool) -> String {
    items
        .iter()
        .map(|item| {
            if number_lines {
                format!("{}\t{}", item.start_line, item.text)
            } else {
                item.text.clone()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_context_items(items: &[ContextItem]) -> String {
    items
        .iter()
        .map(|item| {
            let lines = item
                .lines
                .iter()
                .map(|line| format!("{}: {}", line.line_number, line.text))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{}:{}-{} score={}\n{}",
                item.path, item.start_line, item.end_line, item.score, lines
            )
        })
        .collect::<Vec<_>>()
        .join("\n--\n")
}

#[allow(clippy::too_many_arguments)]
fn paginate_vec<T, F>(
    tool: &str,
    snapshot_id: Option<&str>,
    fingerprint: &str,
    cursor: Option<&str>,
    items: Vec<T>,
    max_items: usize,
    max_bytes: usize,
    include_rendered_text: bool,
    render: F,
) -> Result<Page<T>, String>
where
    T: Clone + Serialize,
    F: Fn(&[T]) -> String,
{
    let offset = cursor_offset(tool, snapshot_id, fingerprint, cursor)?;
    paginate_vec_with_offset(
        tool,
        snapshot_id,
        fingerprint,
        offset,
        items,
        max_items,
        max_bytes,
        include_rendered_text,
        render,
    )
}

#[allow(clippy::too_many_arguments)]
fn paginate_vec_with_offset<T, F>(
    tool: &str,
    snapshot_id: Option<&str>,
    fingerprint: &str,
    offset: usize,
    items: Vec<T>,
    max_items: usize,
    max_bytes: usize,
    include_rendered_text: bool,
    render: F,
) -> Result<Page<T>, String>
where
    T: Clone + Serialize,
    F: Fn(&[T]) -> String,
{
    let window = items.into_iter().skip(offset).take(max_items + 1).collect();
    paginate_window(
        tool,
        snapshot_id,
        fingerprint,
        offset,
        window,
        max_items,
        max_bytes,
        include_rendered_text,
        render,
    )
}

#[allow(clippy::too_many_arguments)]
fn paginate_window<T, F>(
    tool: &str,
    snapshot_id: Option<&str>,
    fingerprint: &str,
    offset: usize,
    mut window: Vec<T>,
    max_items: usize,
    max_bytes: usize,
    include_rendered_text: bool,
    render: F,
) -> Result<Page<T>, String>
where
    T: Clone + Serialize,
    F: Fn(&[T]) -> String,
{
    let mut has_more = window.len() > max_items;
    window.truncate(max_items);
    loop {
        let next_cursor = if has_more && !window.is_empty() {
            Some(encode_cursor(&CursorToken {
                version: 1,
                tool: tool.to_string(),
                snapshot_id: snapshot_id.map(str::to_string),
                fingerprint: fingerprint.to_string(),
                offset: offset + window.len(),
            })?)
        } else {
            None
        };
        let rendered_text = include_rendered_text.then(|| render(&window));
        let mut page = Page {
            api_version: "2".to_string(),
            returned_items: window.len(),
            items: window.clone(),
            has_more,
            next_cursor,
            budget: BudgetInfo {
                requested_bytes: max_bytes,
                serialized_bytes: 0,
            },
            rendered_text,
        };
        stabilize_serialized_size(&mut page)?;
        if page.budget.serialized_bytes <= max_bytes {
            if page.items.is_empty() && page.has_more {
                return Err(format!(
                    "max_bytes {max_bytes} cannot fit one {tool} item plus its provenance envelope"
                ));
            }
            return Ok(page);
        }
        if window.pop().is_none() {
            return Err(format!(
                "max_bytes {max_bytes} is too small for the {tool} response envelope"
            ));
        }
        has_more = true;
    }
}

fn stabilize_serialized_size<T: Serialize>(page: &mut Page<T>) -> Result<(), String> {
    for _ in 0..8 {
        let size = serde_json::to_vec(page)
            .map_err(|err| format!("failed to serialize MCP response: {err}"))?
            .len();
        if page.budget.serialized_bytes == size {
            return Ok(());
        }
        page.budget.serialized_bytes = size;
    }
    Err("MCP response size metadata did not stabilize".to_string())
}

fn fingerprint(value: &serde_json::Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|err| format!("failed to normalize cursor arguments: {err}"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn encode_cursor(cursor: &CursorToken) -> Result<String, String> {
    let bytes = serde_json::to_vec(cursor)
        .map_err(|err| format!("failed to encode continuation cursor: {err}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn decode_cursor(value: &str) -> Result<CursorToken, String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| "invalid continuation cursor encoding".to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "invalid continuation cursor payload".to_string())
}

fn cursor_offset(
    tool: &str,
    snapshot_id: Option<&str>,
    fingerprint: &str,
    cursor: Option<&str>,
) -> Result<usize, String> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let decoded = decode_cursor(cursor)?;
    if decoded.version != 1
        || decoded.tool != tool
        || decoded.snapshot_id.as_deref() != snapshot_id
        || decoded.fingerprint != fingerprint
    {
        return Err(
            "continuation cursor does not match this tool, snapshot, or normalized arguments"
                .to_string(),
        );
    }
    Ok(decoded.offset)
}

fn validate_budget(value: Option<usize>) -> Result<usize, String> {
    let value = value.unwrap_or(DEFAULT_BUDGET_BYTES);
    if !(MIN_BUDGET_BYTES..=MAX_BUDGET_BYTES).contains(&value) {
        return Err(format!(
            "max_bytes must be between {MIN_BUDGET_BYTES} and {MAX_BUDGET_BYTES}"
        ));
    }
    Ok(value)
}

fn validate_page_items(value: Option<usize>) -> Result<usize, String> {
    validate_range(value, DEFAULT_PAGE_ITEMS, MAX_PAGE_ITEMS, "max_items")
}

fn validate_range(
    value: Option<usize>,
    default: usize,
    max: usize,
    name: &str,
) -> Result<usize, String> {
    let value = value.unwrap_or(default);
    if value == 0 || value > max {
        return Err(format!("{name} must be between 1 and {max}"));
    }
    Ok(value)
}

fn json_response<T: Serialize>(
    value: T,
) -> Result<Json<BTreeMap<String, serde_json::Value>>, String> {
    let value = serde_json::to_value(value)
        .map_err(|err| format!("failed to serialize structured MCP response: {err}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "structured MCP response must be a JSON object".to_string())?;
    Ok(Json(
        object
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    ))
}

fn anyhow_error(err: anyhow::Error) -> String {
    format!("{err:#}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_tool_surface_is_semantic_and_small() {
        let server = WitMcpServer::new();
        let tools = server.tool_router.list_all();
        let names = tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "wit_context",
                "wit_find_repositories",
                "wit_list",
                "wit_open",
                "wit_read",
                "wit_refs",
                "wit_search_code",
            ]
        );
        assert!(!names.contains(&"wit_cat"));
        assert!(!names.contains(&"wit_skill_install"));
    }

    #[test]
    fn cursor_rejects_changed_query_or_snapshot() {
        let token = encode_cursor(&CursorToken {
            version: 1,
            tool: "wit_list".to_string(),
            snapshot_id: Some("owner/repo@abc".to_string()),
            fingerprint: "same".to_string(),
            offset: 10,
        })
        .unwrap();
        assert_eq!(
            cursor_offset("wit_list", Some("owner/repo@abc"), "same", Some(&token)).unwrap(),
            10
        );
        assert!(
            cursor_offset(
                "wit_list",
                Some("owner/repo@different"),
                "same",
                Some(&token)
            )
            .unwrap_err()
            .contains("does not match")
        );
        assert!(
            cursor_offset("wit_list", Some("owner/repo@abc"), "changed", Some(&token))
                .unwrap_err()
                .contains("does not match")
        );
    }

    #[test]
    fn whole_response_budget_shrinks_items_and_reports_exact_size() {
        let items = (0..100)
            .map(|index| format!("item-{index}-{}", "x".repeat(100)))
            .collect::<Vec<_>>();
        let page = paginate_vec(
            "test",
            None,
            "fingerprint",
            None,
            items,
            100,
            2048,
            false,
            |items| items.join("\n"),
        )
        .unwrap();
        let serialized = serde_json::to_vec(&page).unwrap();
        assert_eq!(serialized.len(), page.budget.serialized_bytes);
        assert!(serialized.len() <= 2048);
        assert!(page.has_more);
        assert!(page.next_cursor.is_some());
    }

    #[test]
    fn concatenating_cursor_pages_reconstructs_stable_order() {
        let expected = (0..11).collect::<Vec<_>>();
        let mut actual = Vec::new();
        let mut cursor = None;
        loop {
            let page = paginate_vec(
                "test",
                Some("owner/repo@abc"),
                "fixed",
                cursor.as_deref(),
                expected.clone(),
                3,
                DEFAULT_BUDGET_BYTES,
                false,
                |_| String::new(),
            )
            .unwrap();
            actual.extend(page.items);
            if !page.has_more {
                assert!(page.next_cursor.is_none());
                break;
            }
            cursor = page.next_cursor;
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn context_ranking_merges_overlapping_windows_deterministically() {
        let base = SearchItem {
            snapshot_id: "owner/repo@abc".to_string(),
            repo: "owner/repo".to_string(),
            commit_sha: "abc".to_string(),
            path: "src/cache.rs".to_string(),
            blob_sha: "blob".to_string(),
            query: "Cache".to_string(),
            match_line: 2,
            start_line: 1,
            end_line: 3,
            lines: vec![
                SourceLine {
                    line_number: 1,
                    text: "struct Cache".to_string(),
                },
                SourceLine {
                    line_number: 2,
                    text: "impl Cache".to_string(),
                },
            ],
        };
        let mut second = base.clone();
        second.query = "impl".to_string();
        second.match_line = 3;
        second.start_line = 2;
        second.end_line = 4;
        second.lines.push(SourceLine {
            line_number: 4,
            text: "}".to_string(),
        });
        let ranked = rank_context(
            vec![second, base],
            &["Cache".to_string(), "impl".to_string()],
        );
        assert_eq!(ranked.len(), 1);
        assert_eq!((ranked[0].start_line, ranked[0].end_line), (1, 4));
        assert_eq!(ranked[0].queries, vec!["Cache", "impl"]);
        assert_eq!(ranked[0].lines.len(), 3);
    }

    #[test]
    fn high_match_window_keeps_only_requested_items() {
        let items = (0..100_000).collect::<Vec<_>>();
        let page = paginate_vec(
            "test",
            None,
            "fixed",
            None,
            items,
            25,
            DEFAULT_BUDGET_BYTES,
            false,
            |_| String::new(),
        )
        .unwrap();
        assert_eq!(page.items.len(), 25);
        assert!(page.has_more);
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
        .inspect_err(|err| tracing::error!(?err, "wit MCP v2 server failed"))?;
    service.waiting().await?;
    Ok(())
}

struct SnapshotRecord {
    _temp_dir: TempDir,
    repo_path: PathBuf,
    repo: String,
    commit_sha: String,
}

impl SnapshotRecord {
    fn handle(&self) -> SnapshotHandle {
        SnapshotHandle {
            repo_path: self.repo_path.clone(),
            repo: self.repo.clone(),
            commit_sha: self.commit_sha.clone(),
            snapshot_id: snapshot_id(&self.repo, &self.commit_sha),
        }
    }
}

#[derive(Clone)]
struct SnapshotHandle {
    repo_path: PathBuf,
    repo: String,
    commit_sha: String,
    snapshot_id: String,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    #[default]
    AllowStale,
    RequireFresh,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OpenArgs {
    /// GitHub repository in owner/repo form.
    pub repo: String,
    /// Default branch when omitted; otherwise a branch, tag, refs/heads or refs/tags name, or full commit SHA.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// allow_stale uses branch cache SWR; require_fresh refreshes before pinning.
    #[serde(default)]
    pub freshness: Freshness,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct RefsArgs {
    /// GitHub repository in owner/repo form.
    pub repo: String,
    /// Optionally resolve one branch, tag, or full commit SHA alongside the ref listing.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub cursor: Option<String>,
    pub max_items: Option<usize>,
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub include_rendered_text: bool,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct FindRepositoriesArgs {
    pub pattern: Option<String>,
    pub lang: Option<String>,
    /// Raw GitHub repository-search terms and qualifiers.
    pub query: Option<String>,
    pub cursor: Option<String>,
    pub max_items: Option<usize>,
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub include_rendered_text: bool,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ListArgs {
    pub snapshot_id: String,
    /// Directory or file path; omit for repository root.
    pub path: Option<String>,
    /// Levels below path to return. Defaults to 2.
    pub depth: Option<usize>,
    /// Include sizes and line counts for returned text files.
    #[serde(default)]
    pub include_metadata: bool,
    pub cursor: Option<String>,
    pub max_items: Option<usize>,
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub include_rendered_text: bool,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct SearchCodeArgs {
    pub snapshot_id: String,
    /// One or more Rust regular expressions. Results identify the matching query.
    pub queries: Vec<String>,
    /// Optional git-style glob filters such as **/*.rs.
    #[serde(default)]
    pub globs: Vec<String>,
    pub context_lines: Option<usize>,
    pub max_results: Option<usize>,
    pub cursor: Option<String>,
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub include_rendered_text: bool,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ReadArgs {
    pub snapshot_id: String,
    pub path: String,
    /// One-based inclusive start line. Defaults to 1.
    pub start_line: Option<usize>,
    /// One-based inclusive end line. Defaults to end of file.
    pub end_line: Option<usize>,
    /// Controls optional rendered_text; structured line numbers are always returned for provenance.
    #[serde(default)]
    pub number_lines: bool,
    pub cursor: Option<String>,
    pub max_lines: Option<usize>,
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub include_rendered_text: bool,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ContextArgs {
    pub snapshot_id: String,
    pub queries: Vec<String>,
    #[serde(default)]
    pub globs: Vec<String>,
    pub context_lines: Option<usize>,
    pub max_results: Option<usize>,
    pub cursor: Option<String>,
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub include_rendered_text: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct OpenResponse {
    pub api_version: String,
    pub snapshot_id: String,
    pub repo: String,
    pub requested_ref: String,
    pub resolved_ref: String,
    pub commit_sha: String,
    pub cache: CacheProvenance,
    pub capabilities: SnapshotCapabilities,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CacheProvenance {
    pub state: String,
    pub last_checked_at: Option<u64>,
    pub last_updated_at: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SnapshotCapabilities {
    pub branches: bool,
    pub tags: bool,
    pub full_commit_sha: bool,
    pub pull_request_heads: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RefItem {
    pub repo: String,
    pub name: String,
    pub kind: String,
    pub resolved_ref: String,
    pub commit_sha: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RepositoryItem {
    pub name: String,
    pub full_name: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub stars: u32,
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListItem {
    pub snapshot_id: String,
    pub repo: String,
    pub commit_sha: String,
    pub path: String,
    pub kind: String,
    pub blob_sha: Option<String>,
    pub size_bytes: Option<u64>,
    pub lines: Option<usize>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SourceLine {
    pub line_number: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SearchItem {
    pub snapshot_id: String,
    pub repo: String,
    pub commit_sha: String,
    pub path: String,
    pub blob_sha: String,
    pub query: String,
    pub match_line: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub lines: Vec<SourceLine>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReadLineItem {
    pub snapshot_id: String,
    pub repo: String,
    pub commit_sha: String,
    pub path: String,
    pub blob_sha: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ContextItem {
    pub snapshot_id: String,
    pub repo: String,
    pub commit_sha: String,
    pub path: String,
    pub blob_sha: String,
    pub start_line: usize,
    pub end_line: usize,
    pub score: i64,
    pub ranking_reasons: Vec<String>,
    pub queries: Vec<String>,
    pub lines: Vec<SourceLine>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BudgetInfo {
    pub requested_bytes: usize,
    pub serialized_bytes: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Page<T> {
    pub api_version: String,
    pub items: Vec<T>,
    pub returned_items: usize,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub budget: BudgetInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CursorToken {
    version: u8,
    tool: String,
    snapshot_id: Option<String>,
    fingerprint: String,
    offset: usize,
}

#[tool_router(router = tool_router)]
impl WitMcpServer {
    #[tool(
        name = "wit_open",
        description = "Open one immutable repository snapshot before listing, searching, or reading; reuse its snapshot_id to prevent mixed revisions"
    )]
    pub async fn wit_open(
        &self,
        Parameters(args): Parameters<OpenArgs>,
    ) -> Result<Json<BTreeMap<String, serde_json::Value>>, String> {
        validate_repo(&args.repo)?;
        let requested = args
            .reference
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("HEAD")
            .to_string();
        let remote = github_remote_url(&args.repo);
        let resolved = resolve_requested_ref(&remote, &requested)?;
        let (record, commit_sha, cache) = match &resolved.kind {
            ResolvedRefKind::DefaultBranch(branch) | ResolvedRefKind::Branch(branch) => {
                let selection = if matches!(&resolved.kind, ResolvedRefKind::DefaultBranch(_)) {
                    CacheBranchSelection::Default
                } else {
                    CacheBranchSelection::named(branch)
                };
                let mode = match args.freshness {
                    Freshness::AllowStale => CacheAcquisitionMode::ServeStaleAndRevalidate,
                    Freshness::RequireFresh => CacheAcquisitionMode::ForceInvalidate,
                };
                let cached = cache_github_repo(&args.repo, selection, mode)
                    .await
                    .map_err(anyhow_error)?;
                let commit_sha = git_stdout(
                    cached.path(),
                    &["rev-parse", "HEAD^{commit}"],
                    "resolve cached snapshot commit",
                )?;
                let cache = cache_provenance(cached.path(), args.freshness);
                let record = clone_snapshot(&args.repo, cached.path(), &commit_sha)?;
                (record, commit_sha, cache)
            }
            ResolvedRefKind::Tag | ResolvedRefKind::Commit => {
                let (record, commit_sha) = fetch_snapshot(
                    &args.repo,
                    &remote,
                    &resolved.fetch_ref,
                    &resolved.resolved_ref,
                )?;
                (
                    record,
                    commit_sha,
                    CacheProvenance {
                        state: "fresh".to_string(),
                        last_checked_at: None,
                        last_updated_at: None,
                        last_error: None,
                    },
                )
            }
        };
        let id = snapshot_id(&args.repo, &commit_sha);
        self.snapshots
            .lock()
            .map_err(|_| "snapshot registry lock was poisoned".to_string())?
            .entry(id.clone())
            .or_insert(record);

        json_response(OpenResponse {
            api_version: "2".to_string(),
            snapshot_id: id,
            repo: args.repo,
            requested_ref: requested,
            resolved_ref: resolved.resolved_ref,
            commit_sha,
            cache,
            capabilities: SnapshotCapabilities {
                branches: true,
                tags: true,
                full_commit_sha: true,
                pull_request_heads: "not_supported; resolve the PR head to a full commit SHA"
                    .to_string(),
            },
        })
    }

    #[tool(
        name = "wit_refs",
        description = "Discover default branch, branches, and tags, or resolve one ref before opening an immutable snapshot"
    )]
    pub async fn wit_refs(
        &self,
        Parameters(args): Parameters<RefsArgs>,
    ) -> Result<Json<BTreeMap<String, serde_json::Value>>, String> {
        validate_repo(&args.repo)?;
        let max_items = validate_page_items(args.max_items)?;
        let max_bytes = validate_budget(args.max_bytes)?;
        let remote = github_remote_url(&args.repo);
        let mut refs = list_remote_refs(&args.repo, &remote)?;
        if let Some(reference) = args.reference.as_deref() {
            let resolved = resolve_requested_ref(&remote, reference)?;
            if !refs
                .iter()
                .any(|item| item.resolved_ref == resolved.resolved_ref)
            {
                refs.push(RefItem {
                    repo: args.repo.clone(),
                    name: reference.to_string(),
                    kind: resolved.kind.label().to_string(),
                    resolved_ref: resolved.resolved_ref,
                    commit_sha: resolved.commit_sha.unwrap_or_else(|| reference.to_string()),
                    is_default: false,
                });
            }
        }
        refs.sort_by(|left, right| {
            (!left.is_default, &left.kind, &left.name).cmp(&(
                !right.is_default,
                &right.kind,
                &right.name,
            ))
        });
        let fingerprint = fingerprint(&json!({
            "repo": args.repo,
            "ref": args.reference,
            "include_rendered_text": args.include_rendered_text,
        }))?;
        let page = paginate_vec(
            "wit_refs",
            None,
            &fingerprint,
            args.cursor.as_deref(),
            refs,
            max_items,
            max_bytes,
            args.include_rendered_text,
            |items| {
                items
                    .iter()
                    .map(|item| {
                        format!("{}\t{}\t{}", item.kind, item.commit_sha, item.resolved_ref)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        )?;
        json_response(page)
    }

    #[tool(
        name = "wit_find_repositories",
        description = "Discover GitHub repositories; use this only when owner/repo is unknown, then call wit_open"
    )]
    pub async fn wit_find_repositories(
        &self,
        Parameters(args): Parameters<FindRepositoriesArgs>,
    ) -> Result<Json<BTreeMap<String, serde_json::Value>>, String> {
        let max_items = validate_page_items(args.max_items)?;
        let max_bytes = validate_budget(args.max_bytes)?;
        let fingerprint = fingerprint(&json!({
            "pattern": args.pattern,
            "lang": args.lang,
            "query": args.query,
            "include_rendered_text": args.include_rendered_text,
        }))?;
        let offset = cursor_offset(
            "wit_find_repositories",
            None,
            &fingerprint,
            args.cursor.as_deref(),
        )?;
        let fetch_limit = (offset + max_items + 1).min(MAX_GITHUB_REPOS);
        let results = GitHubSearchClient::new()
            .search_repositories(
                args.pattern.as_deref(),
                args.lang.as_deref(),
                args.query.as_deref(),
                fetch_limit,
            )
            .await
            .map_err(anyhow_error)?;
        let items = results
            .repositories
            .into_iter()
            .map(|repo| RepositoryItem {
                name: repo.name,
                full_name: repo.full_name,
                description: repo.description,
                language: repo.language,
                stars: repo.stars,
                html_url: repo.html_url,
            })
            .collect();
        let page = paginate_vec_with_offset(
            "wit_find_repositories",
            None,
            &fingerprint,
            offset,
            items,
            max_items,
            max_bytes,
            args.include_rendered_text,
            |items| {
                items
                    .iter()
                    .map(|item| format!("{}\t{}", item.stars, item.full_name))
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        )?;
        json_response(page)
    }

    #[tool(
        name = "wit_list",
        description = "List bounded repository structure from a snapshot with explicit depth; use before code search when paths are unknown"
    )]
    pub async fn wit_list(
        &self,
        Parameters(args): Parameters<ListArgs>,
    ) -> Result<Json<BTreeMap<String, serde_json::Value>>, String> {
        let snapshot = self.snapshot(&args.snapshot_id)?;
        let depth = validate_range(args.depth, DEFAULT_LIST_DEPTH, MAX_LIST_DEPTH, "depth")?;
        let max_items = validate_page_items(args.max_items)?;
        let max_bytes = validate_budget(args.max_bytes)?;
        let base_path = normalize_repo_path(args.path.as_deref().unwrap_or(""))?;
        let fingerprint = fingerprint(&json!({
            "snapshot_id": args.snapshot_id,
            "path": base_path,
            "depth": depth,
            "include_metadata": args.include_metadata,
            "include_rendered_text": args.include_rendered_text,
        }))?;
        let offset = cursor_offset(
            "wit_list",
            Some(&snapshot.snapshot_id),
            &fingerprint,
            args.cursor.as_deref(),
        )?;
        let items = list_snapshot_window(
            &snapshot,
            &base_path,
            depth,
            args.include_metadata,
            offset,
            max_items + 1,
        )?;
        let page = paginate_window(
            "wit_list",
            Some(&snapshot.snapshot_id),
            &fingerprint,
            offset,
            items,
            max_items,
            max_bytes,
            args.include_rendered_text,
            |items| {
                items
                    .iter()
                    .map(|item| item.path.clone())
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        )?;
        json_response(page)
    }

    #[tool(
        name = "wit_search_code",
        description = "Search one immutable snapshot with one or more regex queries and return bounded atomic context groups with provenance"
    )]
    pub async fn wit_search_code(
        &self,
        Parameters(args): Parameters<SearchCodeArgs>,
    ) -> Result<Json<BTreeMap<String, serde_json::Value>>, String> {
        let snapshot = self.snapshot(&args.snapshot_id)?;
        let queries = compile_queries(&args.queries)?;
        let globs = compile_globs(&args.globs)?;
        let context_lines = validate_range(
            args.context_lines,
            DEFAULT_CONTEXT_LINES,
            MAX_CONTEXT_LINES,
            "context_lines",
        )?;
        let max_results = validate_range(
            args.max_results,
            DEFAULT_PAGE_ITEMS,
            MAX_PAGE_ITEMS,
            "max_results",
        )?;
        let max_bytes = validate_budget(args.max_bytes)?;
        let fingerprint = fingerprint(&json!({
            "snapshot_id": args.snapshot_id,
            "queries": args.queries,
            "globs": args.globs,
            "context_lines": context_lines,
            "max_results": max_results,
            "include_rendered_text": args.include_rendered_text,
        }))?;
        let offset = cursor_offset(
            "wit_search_code",
            Some(&snapshot.snapshot_id),
            &fingerprint,
            args.cursor.as_deref(),
        )?;
        let items = search_snapshot_window(
            &snapshot,
            &queries,
            &globs,
            context_lines,
            offset,
            max_results + 1,
        )?;
        let page = paginate_window(
            "wit_search_code",
            Some(&snapshot.snapshot_id),
            &fingerprint,
            offset,
            items,
            max_results,
            max_bytes,
            args.include_rendered_text,
            render_search_items,
        )?;
        json_response(page)
    }

    #[tool(
        name = "wit_read",
        description = "Read an explicit one-based inclusive line range from a snapshot; use after list or search identifies a file"
    )]
    pub async fn wit_read(
        &self,
        Parameters(args): Parameters<ReadArgs>,
    ) -> Result<Json<BTreeMap<String, serde_json::Value>>, String> {
        let snapshot = self.snapshot(&args.snapshot_id)?;
        let path = normalize_repo_path(&args.path)?;
        if path.is_empty() {
            return Err("path must identify a file".to_string());
        }
        let start_line = args.start_line.unwrap_or(1);
        if start_line == 0 {
            return Err("start_line is one-based and must be >= 1".to_string());
        }
        if args.end_line.is_some_and(|end| end < start_line) {
            return Err("end_line must be >= start_line".to_string());
        }
        let max_lines = validate_range(
            args.max_lines,
            DEFAULT_PAGE_ITEMS,
            MAX_PAGE_ITEMS,
            "max_lines",
        )?;
        let max_bytes = validate_budget(args.max_bytes)?;
        let fingerprint = fingerprint(&json!({
            "snapshot_id": args.snapshot_id,
            "path": path,
            "start_line": start_line,
            "end_line": args.end_line,
            "number_lines": args.number_lines,
            "max_lines": max_lines,
            "include_rendered_text": args.include_rendered_text,
        }))?;
        let offset = cursor_offset(
            "wit_read",
            Some(&snapshot.snapshot_id),
            &fingerprint,
            args.cursor.as_deref(),
        )?;
        let items = read_snapshot_window(
            &snapshot,
            &path,
            start_line,
            args.end_line,
            offset,
            max_lines + 1,
        )?;
        let number_lines = args.number_lines;
        let page = paginate_window(
            "wit_read",
            Some(&snapshot.snapshot_id),
            &fingerprint,
            offset,
            items,
            max_lines,
            max_bytes,
            args.include_rendered_text,
            move |items| render_read_items(items, number_lines),
        )?;
        json_response(page)
    }

    #[tool(
        name = "wit_context",
        description = "Gather deterministic ranked multi-file evidence from a snapshot; use when one answer needs several bounded supporting snippets"
    )]
    pub async fn wit_context(
        &self,
        Parameters(args): Parameters<ContextArgs>,
    ) -> Result<Json<BTreeMap<String, serde_json::Value>>, String> {
        let snapshot = self.snapshot(&args.snapshot_id)?;
        let queries = compile_queries(&args.queries)?;
        let globs = compile_globs(&args.globs)?;
        let context_lines = validate_range(
            args.context_lines,
            DEFAULT_CONTEXT_LINES,
            MAX_CONTEXT_LINES,
            "context_lines",
        )?;
        let max_results = validate_range(
            args.max_results,
            DEFAULT_CONTEXT_RESULTS,
            MAX_CONTEXT_RESULTS,
            "max_results",
        )?;
        let max_bytes = validate_budget(args.max_bytes)?;
        let fingerprint = fingerprint(&json!({
            "snapshot_id": args.snapshot_id,
            "queries": args.queries,
            "globs": args.globs,
            "context_lines": context_lines,
            "max_results": max_results,
            "include_rendered_text": args.include_rendered_text,
        }))?;
        let offset = cursor_offset(
            "wit_context",
            Some(&snapshot.snapshot_id),
            &fingerprint,
            args.cursor.as_deref(),
        )?;
        let candidates = search_snapshot_window(
            &snapshot,
            &queries,
            &globs,
            context_lines,
            0,
            MAX_CONTEXT_CANDIDATES + 1,
        )?;
        if candidates.len() > MAX_CONTEXT_CANDIDATES {
            return Err(format!(
                "wit_context matched more than {MAX_CONTEXT_CANDIDATES} candidate windows; narrow queries or globs so deterministic ranking stays memory-bounded"
            ));
        }
        let ranked = rank_context(candidates, &args.queries);
        let items = ranked
            .into_iter()
            .skip(offset)
            .take(max_results + 1)
            .collect();
        let page = paginate_window(
            "wit_context",
            Some(&snapshot.snapshot_id),
            &fingerprint,
            offset,
            items,
            max_results,
            max_bytes,
            args.include_rendered_text,
            render_context_items,
        )?;
        json_response(page)
    }
}
