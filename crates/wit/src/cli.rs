use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use colored::Colorize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

const SKILL_MD: &str = include_str!("skill/SKILL.md");
use wit::{
    ensure_rustls_provider,
    gitops::ops::{
        BranchMetadata, CacheAcquisitionMode, CacheBranchSelection, GrepOptions, GrepResult,
        build_tree_with_ignore, cache_github_repo, grep_repo_with_options, head_with_ignore,
        list_dir_with_ignore, list_remote_branches, read_file, read_file_with_ignore,
        revalidate_github_repo, tail_with_ignore,
    },
    search::{DEFAULT_GITHUB_REPO_LIMIT, MAX_GITHUB_REPOS},
    search_run, sed,
    snapshot::CliSnapshotBackend,
};
use wit_snapshot::{
    DirEntry, EntryKind, MemoryBackend, RepoSnapshot, SnapshotBackend, SnapshotProvenance,
};

#[derive(Parser)]
#[command(name = "wit")]
#[command(
    about = "Explore GitHub repositories without cloning. Repos are cached as shallow bare clones in your system temp directory (override with WIT_CACHE_DIR).",
    long_about = None,
    after_help = "Branch discovery: run wit branches -r owner/repo to list available branch names with ahead/behind, merged, tip, author, and created-time metadata before choosing --branch BRANCH.\n\nCache behavior: repo-reading commands use a branch-keyed stale-while-revalidate cache by default. Pass --branch BRANCH on cache, tree, ls, cat, rg, sed, head, or tail to read a named branch instead of the repository default. Pass --refresh-cache on tree, ls, cat, rg, sed, head, or tail to force refresh the selected branch before reading. Use wit cache -r owner/repo for an explicit cache refresh. No public TTL/max-age option is exposed.\n\nSnapshot backends: tree/ls/cat default to the disk cache backend. Pass --backend memory (or set WIT_SNAPSHOT_BACKEND=memory) to load a public repo over the GitHub API into RAM with zero WIT_CACHE_DIR writes. Memory backend does not support rg/sed/head/tail in this slice."
)]
struct WitCli {
    /// Exclude files, directories, or glob patterns (repeatable)
    #[arg(
        long = "ignore",
        value_name = "PATH|GLOB",
        global = true,
        action = ArgAction::Append
    )]
    ignore: Vec<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(
        name = "search",
        visible_alias = "s",
        about = "Find GitHub repositories via the GitHub REST search API",
        override_usage = "wit <search|s> [OPTIONS]",
        after_help = "Use -p/--pattern to restrict repository names and -q/--query to pass raw GitHub search terms and qualifiers through to the REST API. Common qualifiers include language:, user:, org:, stars:, forks:, size:, created:, pushed:, topic:, archived:, mirror:, template:, license:, help-wanted-issues:, and good-first-issues:.\n\nwit fetches only enough GitHub pages to satisfy --limit (default: 10, max: 1000). Results are ordered by stars. GitHub repository search does not support regex name matching; --pattern is treated as a literal name filter.\n\nSet GITHUB_TOKEN for higher GitHub rate limits.\n\nExamples:\n  wit search -p 'ratatui' -l 'Rust' --limit 20\n  wit search -q 'stars:>1000 topic:tui archived:false' --limit 25\n  wit search -p 'auth' -q 'user:ory language:Go pushed:>2025-01-01'"
    )]
    Search {
        /// Optional repository name filter. GitHub treats this as a literal name search, not regex.
        #[arg(short, long)]
        pattern: Option<String>,

        /// Optional GitHub language qualifier.
        #[arg(short, long)]
        lang: Option<String>,

        /// Additional raw GitHub search terms and qualifiers, passed through as-is.
        #[arg(short, long)]
        query: Option<String>,

        /// Maximum number of repositories to print (GitHub search caps at 1000).
        #[arg(
            short = 'n',
            long = "limit",
            default_value_t = DEFAULT_GITHUB_REPO_LIMIT,
            value_parser = parse_search_limit
        )]
        limit: usize,
    },
    #[command(
        name = "branches",
        about = "List remote branches with default-branch comparison metadata",
        override_usage = "wit branches -r <REPO>",
        after_help = "Lists GitHub branches under refs/heads so you can choose an existing value for --branch on cache/read commands. Ahead, behind, and merged are computed against the repository default branch. Created is inferred from the first commit unique to the branch when one exists; otherwise it falls back to the branch tip commit time.\n\nExamples:\n  wit branches -r ratatui/ratatui"
    )]
    Branches {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: String,
    },
    #[command(
        name = "cache",
        visible_alias = "c",
        about = "Clone a repository into the local cache (or refresh an existing one)",
        after_help = "Repos are auto-cached on first use by other commands. Use this to force-refresh the default branch cache before returning, or pass --branch BRANCH to refresh a named branch cache.\n\nRepo-reading commands normally serve cached content immediately and revalidate the selected branch in the background. Pass --refresh-cache on tree, ls, cat, rg, sed, head, or tail when the read must wait for a fresh cache.\n\nExamples:\n  wit cache -r ratatui/ratatui                    # Force re-clone of default branch\n  wit cache -r ratatui/ratatui --branch main      # Force refresh a named branch"
    )]
    Cache {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: String,

        /// Branch name under refs/heads to cache instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,
    },
    #[command(
        name = "tree",
        visible_alias = "t",
        about = "Show the file tree of a repository (or subtree). Use -l for line counts",
        override_usage = "wit <tree|t> [OPTIONS] -r <REPO> [PATH]",
        after_help = "Start here to understand a repo's structure. Narrow with a path to avoid noise on large repos. Use -l to see file sizes and decide whether to cat or head.\n\nExamples:\n  wit tree -r ratatui/ratatui                # Full repo tree\n  wit tree -r ratatui/ratatui src/widgets    # Only the widgets subtree\n  wit tree -l -r ratatui/ratatui src         # With line counts and token estimates\n  wit tree -r octocat/Hello-World --backend memory"
    )]
    Tree {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: String,

        /// Branch name under refs/heads to read instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Force refresh the branch cache before reading
        #[arg(long = "refresh-cache", action = ArgAction::SetTrue)]
        refresh_cache: bool,

        /// Optional subdirectory path to display tree from
        path: Option<String>,

        /// Show file sizes: lines and approximate token count
        #[arg(short = 'l', long = "long")]
        long: bool,

        /// Snapshot backend: disk (default cache) or memory (no filesystem cache)
        #[arg(long = "backend", value_name = "disk|memory")]
        backend: Option<String>,
    },
    #[command(
        name = "ls",
        about = "List directory contents (non-recursive). Use -l for file sizes",
        override_usage = "wit ls [OPTIONS] -r <REPO> [PATH]",
        after_help = "Use to browse one directory level at a time. Unlike tree (recursive), ls shows only immediate children. Use -l to see line counts and token estimates before deciding what to read.\n\nExamples:\n  wit ls -r ratatui/ratatui                    # List repo root\n  wit ls -r ratatui/ratatui src/widgets        # List a subdirectory\n  wit ls -l -r ratatui/ratatui src             # With file sizes\n  wit ls -r ratatui/ratatui --backend memory   # No disk cache"
    )]
    Ls {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: String,

        /// Branch name under refs/heads to read instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Force refresh the branch cache before reading
        #[arg(long = "refresh-cache", action = ArgAction::SetTrue)]
        refresh_cache: bool,

        /// Directory path within the repository (default: root)
        path: Option<String>,

        /// Show file sizes: lines and approximate token count
        #[arg(short = 'l', long = "long")]
        long: bool,

        /// Snapshot backend: disk (default cache) or memory (no filesystem cache)
        #[arg(long = "backend", value_name = "disk|memory")]
        backend: Option<String>,
    },
    #[command(
        name = "cat",
        about = "Print a file's contents. Use -n for line numbers",
        override_usage = "wit cat [OPTIONS] -r <REPO> <PATH>",
        after_help = "Use for small-to-medium files. For large files, prefer head/tail/sed to read specific ranges, or rg to search for patterns.\n\nExamples:\n  wit cat -r ratatui/ratatui Cargo.toml             # Print file\n  wit cat -n -r ratatui/ratatui src/lib.rs           # With line numbers\n  wit cat -b -r ratatui/ratatui README.md            # Number non-blank lines only\n  wit cat -r octocat/Hello-World README --backend memory"
    )]
    Cat {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: String,

        /// Branch name under refs/heads to read instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Force refresh the branch cache before reading
        #[arg(long = "refresh-cache", action = ArgAction::SetTrue)]
        refresh_cache: bool,

        /// Path to the file within the repository
        path: String,

        /// Number all output lines
        #[arg(short = 'n', long = "number")]
        number: bool,

        /// Number non-blank output lines only (overrides -n)
        #[arg(short = 'b', long = "number-nonblank")]
        number_nonblank: bool,

        /// Suppress repeated empty output lines
        #[arg(short = 's', long = "squeeze-blank")]
        squeeze_blank: bool,

        /// Display $ at end of each line
        #[arg(short = 'E', long = "show-ends")]
        show_ends: bool,

        /// Display TAB characters as ^I
        #[arg(short = 'T', long = "show-tabs")]
        show_tabs: bool,

        /// Equivalent to -ET (show ends and tabs)
        #[arg(short = 'A', long = "show-all")]
        show_all: bool,

        /// Snapshot backend: disk (default cache) or memory (no filesystem cache)
        #[arg(long = "backend", value_name = "disk|memory")]
        backend: Option<String>,
    },
    #[command(
        name = "rg",
        about = "Search file contents (ripgrep-style). Use -l to find files, -g to filter by type",
        override_usage = "wit rg [OPTIONS] <PATTERN> -r <REPO>",
        after_help = "The primary tool for locating code. Use -l to discover which files contain a pattern (cheaper than full matches). Use -g to restrict to file types. Combine -C for context around matches.\n\nExamples:\n  wit rg 'impl Widget' -r ratatui/ratatui              # Find implementations\n  wit rg -l 'struct.*Frame' -r ratatui/ratatui          # List files containing pattern\n  wit rg -g '*.rs' -i 'todo' -r ratatui/ratatui         # Case-insensitive in .rs files\n  wit rg -C 3 'fn render' -r ratatui/ratatui             # 3 lines of context"
    )]
    Rg {
        /// Regex pattern to search for
        pattern: String,

        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: String,

        /// Branch name under refs/heads to search instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Force refresh the branch cache before reading
        #[arg(long = "refresh-cache", action = ArgAction::SetTrue)]
        refresh_cache: bool,

        /// Case insensitive search
        #[arg(short = 'i', long)]
        ignore_case: bool,

        /// Smart case: case-insensitive if pattern is all lowercase
        #[arg(short = 'S', long)]
        smart_case: bool,

        /// Match whole words only
        #[arg(short = 'w', long)]
        word_regexp: bool,

        /// Invert match (show non-matching lines)
        #[arg(short = 'v', long)]
        invert_match: bool,

        /// Maximum number of matches to show (-m 0 disables searching, like rg)
        #[arg(short = 'm', long)]
        max_count: Option<usize>,

        /// Lines of context to show before and after matches
        #[arg(short = 'C', long, default_value_t = 0)]
        context: usize,

        /// Lines of context to show before matches
        #[arg(short = 'B', long, default_value_t = 0)]
        before_context: usize,

        /// Lines of context to show after matches
        #[arg(short = 'A', long, default_value_t = 0)]
        after_context: usize,

        /// Glob pattern to filter files (e.g., "*.rs", "src/**")
        #[arg(short = 'g', long)]
        glob: Option<String>,

        /// Only show file names with matches
        #[arg(short = 'l', long)]
        files_with_matches: bool,

        /// Only show count of matches per file
        #[arg(short = 'c', long)]
        count: bool,

        /// Show file sizes alongside file names (useful with -l)
        #[arg(long = "long")]
        long_format: bool,
    },
    #[command(
        name = "sed",
        about = "Extract or transform file content using sed scripts (POSIX-style, Rust regex)",
        override_usage = "wit sed [OPTIONS] -r <REPO> [<SCRIPT>] <PATH>",
        after_help = "Use for precise line-range extraction or text transformation. Regex uses Rust syntax, not POSIX BRE. Supports addresses, substitution, hold space, branching, and most POSIX commands.\n\nExamples:\n  wit sed -n -r modal-labs/modal-client '320,460p' modal/image.py    # Print line range\n  wit sed -n -r ratatui/ratatui '/TODO/p' src/lib.rs                 # Lines matching pattern\n  wit sed -r ratatui/ratatui 's/Widget/Component/g' src/lib.rs       # Substitute text\n  wit sed -n -r ratatui/ratatui '/^pub fn/p' src/lib.rs              # Extract function signatures"
    )]
    Sed {
        /// Suppress automatic printing of pattern space
        #[arg(short = 'n', long = "quiet", alias = "silent")]
        quiet: bool,

        /// Force refresh the branch cache before reading
        #[arg(long = "refresh-cache", action = ArgAction::SetTrue)]
        refresh_cache: bool,

        /// Add script to the commands to be executed
        #[arg(short = 'e', long = "expression")]
        expressions: Vec<String>,

        /// Add script file to the commands to be executed
        #[arg(short = 'f', long = "file", value_name = "FILE")]
        files: Vec<String>,

        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: String,

        /// Branch name under refs/heads to read instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Sed script and file path (positional): <SCRIPT> <PATH>, or <PATH> when using -e/-f for the script
        #[arg(allow_hyphen_values = true)]
        args: Vec<String>,
    },
    #[command(
        name = "head",
        about = "Print the first N lines of a file (default: 10)",
        override_usage = "wit head [OPTIONS] -r <REPO> <PATH>",
        after_help = "Use to preview a file before deciding whether to read it fully. Pair with tail to read specific sections by position.\n\nExamples:\n  wit head -r ratatui/ratatui src/lib.rs            # First 10 lines\n  wit head -n 50 -r ratatui/ratatui Cargo.toml      # First 50 lines\n  wit head -N -r ratatui/ratatui README.md           # With line numbers"
    )]
    Head {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: String,

        /// Branch name under refs/heads to read instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Force refresh the branch cache before reading
        #[arg(long = "refresh-cache", action = ArgAction::SetTrue)]
        refresh_cache: bool,

        /// Path to the file within the repository
        path: String,

        /// Number of lines to show (default: 10)
        #[arg(short = 'n', long = "lines", default_value_t = 10)]
        lines: usize,

        /// Number all output lines
        #[arg(short = 'N', long = "number")]
        number: bool,
    },
    #[command(
        name = "tail",
        about = "Print the last N lines of a file, or from line N onward",
        override_usage = "wit tail [OPTIONS] -r <REPO> <PATH>",
        after_help = "Use -p to read from a specific line to end-of-file -- useful when you know a line number from rg output and want the surrounding code.\n\nExamples:\n  wit tail -r ratatui/ratatui src/lib.rs              # Last 10 lines\n  wit tail -n 20 -r ratatui/ratatui Cargo.toml        # Last 20 lines\n  wit tail -p 100 -r ratatui/ratatui src/lib.rs       # From line 100 to end"
    )]
    Tail {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: String,

        /// Branch name under refs/heads to read instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Force refresh the branch cache before reading
        #[arg(long = "refresh-cache", action = ArgAction::SetTrue)]
        refresh_cache: bool,

        /// Path to the file within the repository
        path: String,

        /// Number of lines to show (default: 10)
        #[arg(short = 'n', long = "lines", default_value_t = 10)]
        lines: usize,

        /// Start from line N (like tail -n +N)
        #[arg(short = 'p', long = "plus", value_name = "LINE")]
        from_line: Option<usize>,

        /// Number all output lines
        #[arg(short = 'N', long = "number")]
        number: bool,
    },
    #[command(
        name = "skill",
        about = "Manage the wit agent skill",
        after_help = "Examples:\n  wit skill load                        # Print the skill to stdout\n  wit skill install --path ~/skills     # Install skill directory to ~/skills/wit-skill"
    )]
    Skill {
        #[command(subcommand)]
        command: SkillCommands,
    },
    #[command(
        name = "mcp",
        about = "Start wit MCP (direct by default; Code Mode experimental)",
        after_help = "Direct mode exposes seven typed snapshot-first tools, is the default, and is recommended for simple calls. Experimental Code Mode exposes one native JavaScript code tool. Both are structured, bounded, and resumable; neither requires an external JavaScript runtime.\n\nExamples:\n  wit mcp --transport stdio --mode direct   # Default seven-tool surface\n  wit mcp --transport stdio --mode code     # Experimental one-tool surface"
    )]
    Mcp {
        /// MCP transport to use
        #[arg(long, value_enum, default_value = "stdio")]
        transport: McpTransport,

        /// MCP tool surface to expose
        #[arg(long, value_enum, default_value = "direct")]
        mode: McpMode,
    },
    #[command(name = "__cache-revalidate", hide = true)]
    CacheRevalidate {
        /// Repository in "owner/repo" format
        #[arg(long = "repo")]
        repo: String,

        /// Branch to revalidate
        #[arg(long = "branch", hide = true)]
        branch: Option<String>,
    },
}

#[derive(Subcommand)]
enum SkillCommands {
    #[command(about = "Print the wit skill (SKILL.md) to stdout")]
    Load,
    #[command(about = "Install the wit skill as a directory at the given path")]
    Install {
        /// Directory in which to create the wit-skill folder
        #[arg(long, value_name = "DIR")]
        path: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum McpTransport {
    Stdio,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum McpMode {
    /// Recommended default: seven snapshot-first repository tools
    Direct,
    /// Experimental: one bounded native JavaScript code tool
    Code,
}

fn parse_search_limit(value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| format!("`{value}` is not a valid positive integer"))?;

    if !(1..=MAX_GITHUB_REPOS).contains(&limit) {
        return Err(format!("limit must be between 1 and {MAX_GITHUB_REPOS}"));
    }

    Ok(limit)
}

fn repo_cache_mode(refresh_cache: bool) -> CacheAcquisitionMode {
    if refresh_cache {
        CacheAcquisitionMode::ForceInvalidate
    } else {
        CacheAcquisitionMode::ServeStaleAndRevalidate
    }
}

fn cache_branch_selection(branch: Option<String>) -> CacheBranchSelection {
    branch.map_or(CacheBranchSelection::Default, CacheBranchSelection::named)
}

async fn open_memory_snapshot(
    repo: &str,
    branch: Option<&str>,
) -> anyhow::Result<wit_snapshot::MemorySnapshot<wit_snapshot::ReqwestGitHubClient>> {
    let backend = MemoryBackend::from_env().map_err(|err| anyhow::anyhow!(err))?;
    backend
        .open(repo, branch)
        .await
        .map_err(|err| anyhow::anyhow!(err))
}

fn print_memory_provenance(provenance: &SnapshotProvenance) {
    eprintln!(
        "snapshot: backend={} repo={} commit={} cache={}",
        provenance.backend, provenance.repo, provenance.commit_sha, provenance.cache_state
    );
}

fn print_ls_entries(entries: &[wit::gitops::ops::FileMetadata], long: bool) {
    if entries.is_empty() {
        println!("{}", "Directory is empty or does not exist.".yellow());
        return;
    }
    if long {
        let max_lines = entries.iter().filter_map(|e| e.lines).max().unwrap_or(0);
        let lines_width = max_lines.to_string().len().max(1);
        for entry in entries {
            if entry.is_dir {
                println!(
                    "{:>width$}  {}/",
                    "",
                    entry.name,
                    width = lines_width + 3 + 3
                );
            } else if entry.is_binary {
                println!(
                    "{:>width$}   {}",
                    "[bin]",
                    entry.name,
                    width = lines_width + 3 + 2
                );
            } else if let Some(lines) = entry.lines {
                let tokens = lines * 5;
                println!(
                    "{:>width$} ln  {:<30} (~{} tok)",
                    lines,
                    entry.name,
                    tokens,
                    width = lines_width
                );
            }
        }
    } else {
        for entry in entries {
            if entry.is_dir {
                println!("{}/", entry.name);
            } else {
                println!("{}", entry.name);
            }
        }
    }
}

fn print_snapshot_ls(entries: &[DirEntry], long: bool) {
    if long {
        for entry in entries {
            match entry.kind {
                EntryKind::Dir => println!("            {}/", entry.name),
                EntryKind::File => {
                    if let Some(size) = entry.size_bytes {
                        println!("{:>8} B  {}", size, entry.name);
                    } else {
                        println!("            {}", entry.name);
                    }
                }
            }
        }
    } else {
        for entry in entries {
            match entry.kind {
                EntryKind::Dir => println!("{}/", entry.name),
                EntryKind::File => println!("{}", entry.name),
            }
        }
    }
}

fn print_snapshot_tree<S: RepoSnapshot>(
    snap: &S,
    path: Option<&str>,
    long: bool,
) -> anyhow::Result<()> {
    let view = snap.tree(path).map_err(|err| anyhow::anyhow!(err))?;
    println!("{}", view.root);
    let mut dirs: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for entry in &view.entries {
        let relative = if let Some(base) = path {
            let base = base.trim_end_matches('/');
            entry
                .path
                .strip_prefix(base)
                .map(|rest| rest.trim_start_matches('/'))
                .unwrap_or(entry.path.as_str())
        } else {
            entry.path.as_str()
        };
        if relative.is_empty() {
            continue;
        }
        if let Some((dir, name)) = relative.rsplit_once('/') {
            dirs.entry(dir.to_string()).or_default().push(name);
        } else {
            dirs.entry(String::new()).or_default().push(relative);
        }
        let _ = long;
        let label = if long {
            if let Some(size) = entry.size_bytes {
                format!("{relative} ({size} B)")
            } else {
                relative.to_string()
            }
        } else {
            relative.to_string()
        };
        println!("  {label}");
    }
    let _ = dirs;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("__codemode-worker")) {
        anyhow::ensure!(
            std::env::args_os().nth(2).is_none(),
            "__codemode-worker does not accept arguments"
        );
        wit_quickjs_spike::worker::run_worker_process().await;
    }

    ensure_rustls_provider();
    let cli = WitCli::parse();
    let ignore_patterns = cli.ignore;

    match cli.command {
        Commands::Search {
            pattern,
            lang,
            query,
            limit,
        } => {
            search(
                pattern.as_deref(),
                lang.as_deref(),
                query.as_deref(),
                limit,
                &ignore_patterns,
            )
            .await?;
        }
        Commands::Branches { repo } => {
            let branches = list_remote_branches(&repo)?;
            print_branch_results(&branches);
        }
        Commands::Cache { repo, branch } => {
            let repo = cache_github_repo(
                &repo,
                cache_branch_selection(branch),
                CacheAcquisitionMode::ForceInvalidate,
            )
            .await?;
            println!("Cached repository: {}", repo.path().display());
        }
        Commands::Tree {
            repo,
            branch,
            refresh_cache,
            path,
            long,
            backend,
        } => {
            match CliSnapshotBackend::from_env_or_flag(backend.as_deref())
                .map_err(anyhow::Error::msg)?
            {
                CliSnapshotBackend::Disk => {
                    let repository = cache_github_repo(
                        &repo,
                        cache_branch_selection(branch),
                        repo_cache_mode(refresh_cache),
                    )
                    .await?;
                    build_tree_with_ignore(&repository, path.as_deref(), long, &ignore_patterns)?;
                }
                CliSnapshotBackend::Memory => {
                    let snap = open_memory_snapshot(&repo, branch.as_deref()).await?;
                    print_memory_provenance(snap.provenance());
                    print_snapshot_tree(&snap, path.as_deref(), long)?;
                }
            }
        }
        Commands::Ls {
            repo,
            branch,
            refresh_cache,
            path,
            long,
            backend,
        } => {
            match CliSnapshotBackend::from_env_or_flag(backend.as_deref())
                .map_err(anyhow::Error::msg)?
            {
                CliSnapshotBackend::Disk => {
                    let repository = cache_github_repo(
                        &repo,
                        cache_branch_selection(branch),
                        repo_cache_mode(refresh_cache),
                    )
                    .await?;
                    let entries =
                        list_dir_with_ignore(&repository, path.as_deref(), long, &ignore_patterns)?;
                    print_ls_entries(&entries, long);
                }
                CliSnapshotBackend::Memory => {
                    let snap = open_memory_snapshot(&repo, branch.as_deref()).await?;
                    print_memory_provenance(snap.provenance());
                    let entries = snap
                        .list(path.as_deref())
                        .map_err(|err| anyhow::anyhow!(err))?;
                    if entries.is_empty() {
                        println!("{}", "Directory is empty or does not exist.".yellow());
                    } else {
                        print_snapshot_ls(&entries, long);
                    }
                }
            }
        }
        Commands::Cat {
            repo,
            branch,
            refresh_cache,
            path,
            number,
            number_nonblank,
            squeeze_blank,
            show_ends,
            show_tabs,
            show_all,
            backend,
        } => {
            let content = match CliSnapshotBackend::from_env_or_flag(backend.as_deref())
                .map_err(anyhow::Error::msg)?
            {
                CliSnapshotBackend::Disk => {
                    let repository = cache_github_repo(
                        &repo,
                        cache_branch_selection(branch),
                        repo_cache_mode(refresh_cache),
                    )
                    .await?;
                    read_file_with_ignore(&repository, &path, &ignore_patterns)?
                }
                CliSnapshotBackend::Memory => {
                    let snap = open_memory_snapshot(&repo, branch.as_deref()).await?;
                    print_memory_provenance(snap.provenance());
                    snap.read(&path)
                        .await
                        .map_err(|err| anyhow::anyhow!(err))?
                        .text
                }
            };

            // -A is equivalent to -ET
            let show_ends = show_ends || show_all;
            let show_tabs = show_tabs || show_all;

            // -b overrides -n
            let number_lines = number && !number_nonblank;

            let mut line_num = 0usize;
            let mut prev_blank = false;

            for line in content.lines() {
                let is_blank = line.is_empty();

                // -s: squeeze multiple blank lines
                if squeeze_blank && is_blank && prev_blank {
                    continue;
                }
                prev_blank = is_blank;

                // Apply transformations
                let mut output = line.to_string();

                // -T: show tabs as ^I
                if show_tabs {
                    output = output.replace('\t', "^I");
                }

                // -E: show $ at end of line
                if show_ends {
                    output.push('$');
                }

                // -n or -b: line numbering
                if number_lines || (number_nonblank && !is_blank) {
                    line_num += 1;
                    println!("{:>6}  {}", line_num, output);
                } else if number_nonblank && is_blank {
                    // For -b, blank lines don't get numbered but still printed
                    println!("{:>6}  {}", "", output);
                } else {
                    println!("{}", output);
                }
            }
        }
        Commands::Rg {
            repo,
            branch,
            refresh_cache,
            pattern,
            ignore_case,
            smart_case,
            word_regexp,
            invert_match,
            max_count,
            context,
            before_context,
            after_context,
            glob,
            files_with_matches,
            count,
            long_format,
        } => {
            let repository = cache_github_repo(
                &repo,
                cache_branch_selection(branch),
                repo_cache_mode(refresh_cache),
            )
            .await?;

            // Build options from CLI flags
            let mut opts = GrepOptions::new()
                .ignore_case(ignore_case)
                .smart_case(smart_case)
                .word_regexp(word_regexp)
                .invert_match(invert_match)
                .before_context(if context > 0 { context } else { before_context })
                .after_context(if context > 0 { context } else { after_context })
                .glob(glob)
                .ignore(ignore_patterns.clone())
                .files_with_matches(files_with_matches)
                .count(count);
            if let Some(max_count) = max_count {
                opts = opts.max_count(max_count);
            }

            let result = grep_repo_with_options(&repository, &pattern, &opts)?;

            match result {
                GrepResult::Matches(matches) => {
                    if matches.is_empty() {
                        // No output for no matches (like rg)
                        return Ok(());
                    }

                    let mut current_file = String::new();
                    let has_context = before_context > 0 || after_context > 0 || context > 0;

                    for m in matches {
                        // Print file header when file changes
                        if m.path != current_file {
                            if !current_file.is_empty() && has_context {
                                println!(); // Blank line between files
                            }
                            current_file = m.path.clone();
                        }

                        // Handle context separator
                        if m.line_number == 0 && m.content == "--" {
                            println!("{}", "--".dimmed());
                            continue;
                        }

                        let line_num = m.line_number.to_string();
                        if m.is_context {
                            // Context line (dimmed)
                            println!(
                                "{}{}{}{} {}",
                                m.path.magenta(),
                                "-".dimmed(),
                                line_num.dimmed(),
                                "-".dimmed(),
                                m.content.dimmed()
                            );
                        } else {
                            // Match line (highlighted)
                            println!(
                                "{}{}{}{}{}",
                                m.path.magenta(),
                                ":".cyan(),
                                line_num.green(),
                                ":".cyan(),
                                m.content
                            );
                        }
                    }
                }
                GrepResult::Files(files) => {
                    if long_format {
                        for file in &files {
                            let content = read_file(&repository, file);
                            match content {
                                Ok(text) => {
                                    let lines = text.lines().count();
                                    let tokens = lines * 5;
                                    println!(
                                        "{:>6} ln  {:<40} (~{} tok)",
                                        lines,
                                        file.magenta(),
                                        tokens
                                    );
                                }
                                Err(_) => {
                                    println!("{}", file.magenta());
                                }
                            }
                        }
                    } else {
                        for file in files {
                            println!("{}", file.magenta());
                        }
                    }
                }
                GrepResult::Counts(counts) => {
                    for (path, count) in counts {
                        println!("{}:{}", path.magenta(), count.to_string().green());
                    }
                }
            }
        }
        Commands::Head {
            repo,
            branch,
            refresh_cache,
            path,
            lines,
            number,
        } => {
            let repository = cache_github_repo(
                &repo,
                cache_branch_selection(branch),
                repo_cache_mode(refresh_cache),
            )
            .await?;
            let output = head_with_ignore(&repository, &path, lines, number, &ignore_patterns)?;
            println!("{}", output);
        }
        Commands::Sed {
            quiet,
            refresh_cache,
            expressions,
            files,
            repo,
            branch,
            args,
        } => {
            let (args, inline_ignores) = extract_sed_inline_ignores(args)?;
            let mut effective_ignore_patterns = ignore_patterns.clone();
            effective_ignore_patterns.extend(inline_ignores);

            let (scripts, path) = parse_sed_invocation(expressions, files, args)?;
            let repository = cache_github_repo(
                &repo,
                cache_branch_selection(branch),
                repo_cache_mode(refresh_cache),
            )
            .await?;
            let content = read_file_with_ignore(&repository, &path, &effective_ignore_patterns)?;
            let program = sed::parse_script(&scripts)?;
            let output = sed::run(
                &program,
                &content,
                &sed::SedOptions {
                    quiet,
                    allow_file_io: true,
                },
            )?;
            print!("{}", output.output);
            if output.exit_code != 0 {
                std::process::exit(output.exit_code);
            }
        }
        Commands::Tail {
            repo,
            branch,
            refresh_cache,
            path,
            lines,
            from_line,
            number,
        } => {
            let repository = cache_github_repo(
                &repo,
                cache_branch_selection(branch),
                repo_cache_mode(refresh_cache),
            )
            .await?;
            let output = tail_with_ignore(
                &repository,
                &path,
                lines,
                from_line,
                number,
                &ignore_patterns,
            )?;
            println!("{}", output);
        }
        Commands::CacheRevalidate { repo, branch } => {
            revalidate_github_repo(&repo, cache_branch_selection(branch))?;
        }
        Commands::Skill { command } => match command {
            SkillCommands::Load => {
                print!("{SKILL_MD}");
                if !SKILL_MD.ends_with('\n') {
                    println!();
                }
            }
            SkillCommands::Install { path } => {
                let skill_dir = path.join("wit-skill");
                fs::create_dir_all(&skill_dir).map_err(|e| {
                    anyhow::anyhow!(
                        "failed to create skill directory '{}': {}",
                        skill_dir.display(),
                        e
                    )
                })?;
                let skill_path = skill_dir.join("SKILL.md");
                fs::write(&skill_path, SKILL_MD).map_err(|e| {
                    anyhow::anyhow!("failed to write '{}': {}", skill_path.display(), e)
                })?;
                eprintln!("Installed wit skill to {}", skill_path.display());
                println!("{}", skill_path.display());
            }
        },
        Commands::Mcp { transport, mode } => match (transport, mode) {
            (McpTransport::Stdio, McpMode::Direct) => wit::mcp::serve_stdio().await?,
            (McpTransport::Stdio, McpMode::Code) => {
                wit::codemode::serve_stdio_with_worker(std::env::current_exe()?).await?
            }
        },
    }

    Ok(())
}

fn parse_sed_invocation(
    expressions: Vec<String>,
    files: Vec<String>,
    args: Vec<String>,
) -> anyhow::Result<(Vec<String>, String)> {
    let mut scripts = Vec::new();
    scripts.extend(expressions);

    for file in files {
        let content = fs::read_to_string(&file)
            .map_err(|e| anyhow::anyhow!("failed to read sed script file '{}': {}", file, e))?;
        scripts.push(content);
    }

    let (script_arg, path) = match args.len() {
        2 => (Some(args[0].clone()), args[1].clone()),
        1 => (None, args[0].clone()),
        _ => {
            return Err(anyhow::anyhow!(
                "sed expects <SCRIPT> <PATH> or <PATH> with -e/-f (repository via -r/--repo)"
            ));
        }
    };

    if let Some(script) = script_arg {
        scripts.push(script);
    }

    if scripts.is_empty() {
        return Err(anyhow::anyhow!(
            "missing sed script (provide SCRIPT or use -e/-f)"
        ));
    }

    Ok((scripts, path))
}

fn extract_sed_inline_ignores(args: Vec<String>) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let mut remaining_args = Vec::new();
    let mut ignores = Vec::new();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        if arg == "--ignore" {
            let pattern = iter
                .next()
                .ok_or_else(|| anyhow::anyhow!("--ignore requires a value"))?;
            ignores.push(pattern);
            continue;
        }

        if let Some(pattern) = arg.strip_prefix("--ignore=") {
            if pattern.is_empty() {
                return Err(anyhow::anyhow!("--ignore requires a value"));
            }
            ignores.push(pattern.to_string());
            continue;
        }

        remaining_args.push(arg);
    }

    Ok((remaining_args, ignores))
}

fn print_branch_results(branches: &[BranchMetadata]) {
    println!(
        "{:<30} {:<7} {:<40} {:>5} {:>6} {:<6} {:<20} {:<20} {:<20} AUTHOR",
        "BRANCH",
        "DEFAULT",
        "TIP",
        "AHEAD",
        "BEHIND",
        "MERGED",
        "TIP_TIME",
        "CREATED",
        "CREATED_SOURCE"
    );

    for branch in branches {
        println!(
            "{:<30} {:<7} {:<40} {:>5} {:>6} {:<6} {:<20} {:<20} {:<20} {}",
            branch.name,
            if branch.is_default { "*" } else { "-" },
            branch.tip_sha,
            branch.ahead,
            branch.behind,
            if branch.merged { "yes" } else { "no" },
            branch.tip_time,
            branch.created_time,
            branch.created_source.label(),
            branch.tip_author
        );
    }
}

async fn search(
    pattern: Option<&str>,
    lang: Option<&str>,
    query: Option<&str>,
    limit: usize,
    ignore_patterns: &[String],
) -> anyhow::Result<()> {
    let (repos, metric, incomplete_github) =
        search_run::run_repository_search(pattern, lang, query, limit).await?;

    if incomplete_github {
        eprintln!(
            "{}",
            "warning: GitHub reported incomplete_results (index timeout); the repository list may be truncated."
                .yellow()
        );
    }

    if !ignore_patterns.is_empty() {
        println!(
            "{}",
            "note: search --ignore does not affect repository discovery".dimmed()
        );
        println!();
    }

    wits::print_search_results(&repos, false, false, metric);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Command, CommandFactory, error::ErrorKind};

    fn find_subcommand<'a>(command: &'a Command, name: &str) -> &'a Command {
        command
            .find_subcommand(name)
            .unwrap_or_else(|| panic!("expected `{name}` subcommand"))
    }

    fn find_arg<'a>(command: &'a Command, id: &str) -> &'a clap::Arg {
        command
            .get_arguments()
            .find(|arg| arg.get_id().as_str() == id)
            .unwrap_or_else(|| panic!("expected `{id}` arg on `{}`", command.get_name()))
    }

    #[test]
    fn test_cli_command_contract_is_stable() {
        WitCli::command().debug_assert();

        let command = WitCli::command();
        let subcommands: Vec<_> = command
            .get_subcommands()
            .filter(|subcommand| !subcommand.is_hide_set())
            .map(|subcommand| subcommand.get_name())
            .collect();
        assert_eq!(
            subcommands,
            vec![
                "search", "branches", "cache", "tree", "ls", "cat", "rg", "sed", "head", "tail",
                "skill", "mcp"
            ]
        );
        assert_eq!(
            find_subcommand(&command, "search")
                .get_visible_aliases()
                .collect::<Vec<_>>(),
            vec!["s"]
        );
        assert_eq!(
            find_subcommand(&command, "cache")
                .get_visible_aliases()
                .collect::<Vec<_>>(),
            vec!["c"]
        );
        assert_eq!(
            find_subcommand(&command, "tree")
                .get_visible_aliases()
                .collect::<Vec<_>>(),
            vec!["t"]
        );
        assert!(
            find_subcommand(&command, "branches")
                .get_visible_aliases()
                .next()
                .is_none(),
            "branches should not add a short alias"
        );

        let skill = find_subcommand(&command, "skill");
        let skill_subcommands: Vec<_> = skill
            .get_subcommands()
            .map(|subcommand| subcommand.get_name())
            .collect();
        assert_eq!(skill_subcommands, vec!["load", "install"]);

        let mcp = find_subcommand(&command, "mcp");
        let transport = find_arg(mcp, "transport");
        assert_eq!(transport.get_long(), Some("transport"));
        assert_eq!(transport.get_default_values(), ["stdio"]);
    }

    #[test]
    fn test_repo_and_skill_flag_contracts_are_stable() {
        let command = WitCli::command();

        for command_name in ["cache", "tree", "ls", "cat", "rg", "sed", "head", "tail"] {
            let subcommand = find_subcommand(&command, command_name);
            let repo = find_arg(subcommand, "repo");
            assert_eq!(repo.get_short(), Some('r'), "{command_name} should keep -r");
            assert_eq!(
                repo.get_long(),
                Some("repo"),
                "{command_name} should keep --repo"
            );
            assert!(
                repo.is_required_set(),
                "{command_name} should require --repo"
            );
            assert_eq!(
                repo.get_index(),
                None,
                "{command_name} should not accept repo positionally"
            );
        }

        let install = find_subcommand(find_subcommand(&command, "skill"), "install");
        let path = find_arg(install, "path");
        assert_eq!(path.get_short(), None);
        assert_eq!(path.get_long(), Some("path"));
        assert!(
            path.is_required_set(),
            "skill install should require --path"
        );
        assert_eq!(path.get_index(), None);

        let load = find_subcommand(find_subcommand(&command, "skill"), "load");
        assert!(
            !load
                .get_arguments()
                .any(|arg| arg.get_id().as_str() != "help"),
            "skill load should not gain user-facing flags or positionals"
        );
    }

    #[test]
    fn test_global_ignore_parses_for_rg() {
        let cli = WitCli::try_parse_from([
            "wit",
            "rg",
            "needle",
            "-r",
            "owner/repo",
            "--ignore",
            ".git",
            "--ignore",
            "*.png",
        ])
        .expect("rg args should parse");

        assert_eq!(cli.ignore, vec![".git".to_string(), "*.png".to_string()]);
    }

    #[test]
    fn test_global_ignore_parses_for_sed_after_positionals() {
        let cli = WitCli::try_parse_from([
            "wit",
            "sed",
            "-n",
            "--refresh-cache",
            "-r",
            "owner/repo",
            "1,3p",
            "src/lib.rs",
            "--ignore",
            "vendor",
        ])
        .expect("sed args should parse");

        assert!(cli.ignore.is_empty());

        match cli.command {
            Commands::Sed {
                quiet,
                refresh_cache,
                expressions,
                files,
                repo,
                branch,
                args,
            } => {
                let (filtered_args, inline_ignores) =
                    extract_sed_inline_ignores(args).expect("inline sed ignores should parse");

                assert!(quiet);
                assert!(refresh_cache);
                assert!(expressions.is_empty());
                assert!(files.is_empty());
                assert_eq!(repo, "owner/repo");
                assert_eq!(branch, None);
                assert_eq!(inline_ignores, vec!["vendor".to_string()]);
                assert_eq!(filtered_args, vec!["1,3p", "src/lib.rs"]);
            }
            _ => panic!("expected sed command"),
        }
    }

    #[test]
    fn cli_force_cache_invalidation_parses_for_repo_reads() {
        let tree =
            WitCli::try_parse_from(["wit", "tree", "-r", "owner/repo", "--refresh-cache", "src"])
                .expect("tree --refresh-cache should parse");
        match tree.command {
            Commands::Tree {
                refresh_cache,
                path,
                ..
            } => {
                assert!(refresh_cache);
                assert_eq!(path, Some("src".to_string()));
            }
            _ => panic!("expected tree command"),
        }

        let cat = WitCli::try_parse_from([
            "wit",
            "cat",
            "-r",
            "owner/repo",
            "--refresh-cache",
            "README.md",
        ])
        .expect("cat --refresh-cache should parse");
        match cat.command {
            Commands::Cat {
                refresh_cache,
                path,
                ..
            } => {
                assert!(refresh_cache);
                assert_eq!(path, "README.md");
            }
            _ => panic!("expected cat command"),
        }

        let rg =
            WitCli::try_parse_from(["wit", "rg", "needle", "-r", "owner/repo", "--refresh-cache"])
                .expect("rg --refresh-cache should parse");
        match rg.command {
            Commands::Rg {
                refresh_cache,
                pattern,
                ..
            } => {
                assert!(refresh_cache);
                assert_eq!(pattern, "needle");
            }
            _ => panic!("expected rg command"),
        }
    }

    #[test]
    fn cli_branch_flag_parses_and_routes() {
        let command = WitCli::command();
        for command_name in ["cache", "tree", "ls", "cat", "rg", "sed", "head", "tail"] {
            let branch = find_arg(find_subcommand(&command, command_name), "branch");
            assert_eq!(
                branch.get_long(),
                Some("branch"),
                "{command_name} should expose --branch"
            );
            assert_eq!(
                branch.get_short(),
                None,
                "{command_name} should not add a branch alias"
            );
        }

        let branch = Some("feature/api".to_string());
        let cache = WitCli::try_parse_from([
            "wit",
            "cache",
            "-r",
            "owner/repo",
            "--branch",
            "feature/api",
        ])
        .expect("cache --branch should parse");
        assert!(
            matches!(cache.command, Commands::Cache { branch: parsed, .. } if parsed == branch)
        );

        let tree = WitCli::try_parse_from([
            "wit",
            "tree",
            "-r",
            "owner/repo",
            "--branch",
            "feature/api",
            "src",
        ])
        .expect("tree --branch should parse");
        assert!(matches!(tree.command, Commands::Tree { branch: parsed, .. } if parsed == branch));

        let ls = WitCli::try_parse_from([
            "wit",
            "ls",
            "-r",
            "owner/repo",
            "--branch",
            "feature/api",
            "src",
        ])
        .expect("ls --branch should parse");
        assert!(matches!(ls.command, Commands::Ls { branch: parsed, .. } if parsed == branch));

        let cat = WitCli::try_parse_from([
            "wit",
            "cat",
            "-r",
            "owner/repo",
            "--branch",
            "feature/api",
            "README.md",
        ])
        .expect("cat --branch should parse");
        assert!(matches!(cat.command, Commands::Cat { branch: parsed, .. } if parsed == branch));

        let rg = WitCli::try_parse_from([
            "wit",
            "rg",
            "needle",
            "-r",
            "owner/repo",
            "--branch",
            "feature/api",
        ])
        .expect("rg --branch should parse");
        assert!(matches!(rg.command, Commands::Rg { branch: parsed, .. } if parsed == branch));

        let sed = WitCli::try_parse_from([
            "wit",
            "sed",
            "-r",
            "owner/repo",
            "--branch",
            "feature/api",
            "1p",
            "README.md",
        ])
        .expect("sed --branch should parse");
        assert!(matches!(sed.command, Commands::Sed { branch: parsed, .. } if parsed == branch));

        let head = WitCli::try_parse_from([
            "wit",
            "head",
            "-r",
            "owner/repo",
            "--branch",
            "feature/api",
            "README.md",
        ])
        .expect("head --branch should parse");
        assert!(matches!(head.command, Commands::Head { branch: parsed, .. } if parsed == branch));

        let tail = WitCli::try_parse_from([
            "wit",
            "tail",
            "-r",
            "owner/repo",
            "--branch",
            "feature/api",
            "README.md",
        ])
        .expect("tail --branch should parse");
        assert!(matches!(tail.command, Commands::Tail { branch: parsed, .. } if parsed == branch));

        let source = include_str!("cli.rs");
        assert!(
            source.matches("cache_branch_selection(branch)").count() >= 8,
            "repo-reading handlers should route parsed branches to cache acquisition"
        );
    }

    #[test]
    fn cli_branches_command_parses() {
        let command = WitCli::command();
        let branches = find_subcommand(&command, "branches");
        let repo = find_arg(branches, "repo");
        assert_eq!(repo.get_short(), Some('r'));
        assert_eq!(repo.get_long(), Some("repo"));
        assert!(repo.is_required_set());
        assert_eq!(repo.get_index(), None);
        assert!(
            branches.get_visible_aliases().next().is_none(),
            "branches should not have a short alias"
        );

        let parsed = WitCli::try_parse_from(["wit", "branches", "-r", "owner/repo"])
            .expect("branches -r owner/repo should parse");
        assert!(matches!(parsed.command, Commands::Branches { repo } if repo == "owner/repo"));

        let missing_repo = match WitCli::try_parse_from(["wit", "branches"]) {
            Ok(_) => panic!("branches should require --repo"),
            Err(err) => err,
        };
        assert_eq!(missing_repo.kind(), ErrorKind::MissingRequiredArgument);

        let short_alias = match WitCli::try_parse_from(["wit", "b", "-r", "owner/repo"]) {
            Ok(_) => panic!("branches should not have a short alias"),
            Err(err) => err,
        };
        assert_eq!(short_alias.kind(), ErrorKind::InvalidSubcommand);

        let branch = Some("feature/api".to_string());
        let cat = WitCli::try_parse_from([
            "wit",
            "cat",
            "-r",
            "owner/repo",
            "--branch",
            "feature/api",
            "README.md",
        ])
        .expect("cat --branch should still parse");
        assert!(matches!(cat.command, Commands::Cat { branch: parsed, .. } if parsed == branch));
    }

    #[test]
    fn cli_cache_help_text_mentions_cache_contract() {
        let mut command = WitCli::command();
        let help = command.render_long_help().to_string();

        assert!(help.contains("branch-keyed stale-while-revalidate cache"));
        assert!(help.contains("--branch BRANCH"));
        assert!(help.contains("--refresh-cache"));
        assert!(help.contains("No public TTL/max-age option is exposed."));

        let mut cache = find_subcommand(&command, "cache").clone();
        let cache_help = cache.render_long_help().to_string();
        assert!(cache_help.contains("selected branch"));
        assert!(cache_help.contains("--branch"));
        assert!(cache_help.contains("--refresh-cache"));
    }

    #[test]
    fn cli_branches_help_text() {
        let command = WitCli::command();
        let mut branches = find_subcommand(&command, "branches").clone();
        let help = branches.render_long_help().to_string();
        let help_lower = help.to_ascii_lowercase();

        assert!(help.contains("Usage: wit branches -r <REPO>"));
        assert!(help.contains("default-branch comparison metadata"));
        assert!(help_lower.contains("ahead"));
        assert!(help_lower.contains("behind"));
        assert!(help_lower.contains("merged"));
        assert!(help.contains("first commit unique to the branch"));
        assert!(help.contains("branch tip commit time"));
    }

    #[test]
    fn test_search_parses_github_query_and_limit() {
        let cli = WitCli::try_parse_from([
            "wit",
            "search",
            "-p",
            "ratatui",
            "-l",
            "Rust",
            "-q",
            "stars:>1000 archived:false",
            "--limit",
            "25",
        ])
        .expect("search args should parse");

        match cli.command {
            Commands::Search {
                pattern,
                lang,
                query,
                limit,
                ..
            } => {
                assert_eq!(pattern.as_deref(), Some("ratatui"));
                assert_eq!(lang.as_deref(), Some("Rust"));
                assert_eq!(query.as_deref(), Some("stars:>1000 archived:false"));
                assert_eq!(limit, 25);
            }
            _ => panic!("expected search command"),
        }
    }

    #[test]
    fn test_search_allows_raw_query_without_pattern() {
        let cli = WitCli::try_parse_from([
            "wit",
            "search",
            "-q",
            "stars:>5000 topic:tui",
            "--limit",
            "10",
        ])
        .expect("query-only search args should parse");

        match cli.command {
            Commands::Search {
                pattern,
                lang,
                query,
                limit,
                ..
            } => {
                assert_eq!(pattern, None);
                assert_eq!(lang, None);
                assert_eq!(query.as_deref(), Some("stars:>5000 topic:tui"));
                assert_eq!(limit, 10);
            }
            _ => panic!("expected search command"),
        }
    }

    #[test]
    fn test_search_uses_default_limit_when_omitted() {
        let cli = WitCli::try_parse_from(["wit", "search", "-q", "stars:>5000 topic:tui"])
            .expect("query-only search args should parse");

        match cli.command {
            Commands::Search { limit, .. } => assert_eq!(limit, 10),
            _ => panic!("expected search command"),
        }
    }

    #[test]
    fn test_skill_load_parses() {
        let cli =
            WitCli::try_parse_from(["wit", "skill", "load"]).expect("skill load args should parse");

        match cli.command {
            Commands::Skill {
                command: SkillCommands::Load,
            } => {}
            _ => panic!("expected skill load command"),
        }
    }

    #[test]
    fn test_skill_install_parses_required_path_flag() {
        let cli = WitCli::try_parse_from(["wit", "skill", "install", "--path", "/tmp/skills"])
            .expect("skill install args should parse");

        match cli.command {
            Commands::Skill {
                command: SkillCommands::Install { path },
            } => assert_eq!(path, PathBuf::from("/tmp/skills")),
            _ => panic!("expected skill install command"),
        }
    }

    #[test]
    fn test_skill_requires_subcommand() {
        let err = match WitCli::try_parse_from(["wit", "skill"]) {
            Ok(_) => panic!("skill should require a subcommand"),
            Err(err) => err,
        };

        assert_eq!(
            err.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
        let rendered = err.to_string();
        assert!(rendered.contains("Usage: wit skill [OPTIONS] <COMMAND>"));
        assert!(rendered.contains("load"));
        assert!(rendered.contains("install"));
    }

    #[test]
    fn test_skill_install_requires_path_flag() {
        let err = match WitCli::try_parse_from(["wit", "skill", "install"]) {
            Ok(_) => panic!("skill install should require --path"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        assert!(err.to_string().contains("--path <DIR>"));
    }

    #[test]
    fn test_mcp_stdio_parses() {
        let cli = WitCli::try_parse_from(["wit", "mcp", "--transport", "stdio", "--mode", "code"])
            .expect("mcp stdio args should parse");

        match cli.command {
            Commands::Mcp { transport, mode } => {
                assert_eq!(transport, McpTransport::Stdio);
                assert_eq!(mode, McpMode::Code);
            }
            _ => panic!("expected mcp command"),
        }

        let defaulted = WitCli::try_parse_from(["wit", "mcp"]).expect("mcp should default stdio");
        match defaulted.command {
            Commands::Mcp { transport, mode } => {
                assert_eq!(transport, McpTransport::Stdio);
                assert_eq!(mode, McpMode::Direct);
            }
            _ => panic!("expected mcp command"),
        }

        let error = match WitCli::try_parse_from(["wit", "mcp", "--mode", "unknown"]) {
            Ok(_) => panic!("unknown MCP mode must fail"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), ErrorKind::InvalidValue);
        assert!(error.to_string().contains("possible values: direct, code"));
    }

    #[test]
    fn mcp_help_agrees_on_default_and_experimental_status() {
        let command = WitCli::command();
        let root_help = command.clone().render_long_help().to_string();
        assert!(root_help.contains("direct by default; Code Mode experimental"));

        let mcp_help = find_subcommand(&command, "mcp")
            .clone()
            .render_long_help()
            .to_string();
        assert!(mcp_help.contains("[default: direct]"));
        assert!(mcp_help.contains("recommended for simple calls"));
        assert!(mcp_help.contains("Experimental Code Mode"));
        assert!(mcp_help.contains("one native JavaScript code tool"));
        assert!(mcp_help.contains("--mode <MODE>"));
    }

    #[test]
    fn snapshot_backend_flag_parses_for_tree_ls_cat() {
        let tree = WitCli::try_parse_from([
            "wit",
            "tree",
            "-r",
            "owner/repo",
            "--backend",
            "memory",
        ])
        .expect("tree --backend memory should parse");
        assert!(matches!(
            tree.command,
            Commands::Tree {
                backend: Some(ref value),
                ..
            } if value == "memory"
        ));

        let ls = WitCli::try_parse_from(["wit", "ls", "-r", "owner/repo", "--backend", "disk"])
            .expect("ls --backend disk should parse");
        assert!(matches!(
            ls.command,
            Commands::Ls {
                backend: Some(ref value),
                ..
            } if value == "disk"
        ));

        let cat = WitCli::try_parse_from([
            "wit",
            "cat",
            "-r",
            "owner/repo",
            "README.md",
            "--backend",
            "memory",
        ])
        .expect("cat --backend memory should parse");
        assert!(matches!(
            cat.command,
            Commands::Cat {
                backend: Some(ref value),
                ..
            } if value == "memory"
        ));

        let root_help = WitCli::command().render_long_help().to_string();
        assert!(root_help.contains("--backend memory"));
        assert!(root_help.contains("WIT_SNAPSHOT_BACKEND=memory"));
    }
}
