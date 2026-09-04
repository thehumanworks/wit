use clap::{ArgAction, ArgGroup, Parser, Subcommand, ValueEnum};
use colored::Colorize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

const SKILL_MD: &str = include_str!("skill/SKILL.md");
use wit::{
    ast::{self, AstLanguage, SymbolFilter},
    ensure_rustls_provider,
    gitops::ops::{
        BlobWalkOptions, BranchMetadata, CacheAcquisitionMode, CacheBranchSelection, GrepOptions,
        GrepResult, IgnoreMatcher, build_tree_with_ignore, cache_github_repo,
        grep_repo_with_options, head_with_ignore, list_dir_with_ignore, list_remote_branches,
        read_file, read_file_with_ignore, revalidate_github_repo, tail_with_ignore,
        walk_text_blobs,
    },
    search::{DEFAULT_GITHUB_REPO_LIMIT, MAX_GITHUB_REPOS},
    search_run, sed,
    snapshot::{
        CliSnapshotBackend, grep_memory_snapshot, head_from_text, list_remote_branches_api,
        read_memory_text, tail_from_text, walk_memory_text_blobs,
    },
};
use wit_snapshot::{
    DirEntry, EntryKind, MemoryBackend, RepoSnapshot, SnapshotBackend, SnapshotProvenance,
};

#[derive(Parser)]
#[command(name = "wit")]
#[command(
    about = "Explore GitHub repositories without cloning. Repos are cached as shallow bare clones in your system temp directory (override with WIT_CACHE_DIR).",
    long_about = None,
    after_help = "Branch discovery: run wit branches owner/repo (or -r owner/repo) to list available branch names with ahead/behind, merged, tip, author, and created-time metadata before choosing --branch BRANCH.\n\nCache behavior: repo-reading commands use a branch-keyed stale-while-revalidate cache by default. Pass --branch BRANCH on cache, tree, ls, cat, rg, sed, head, or tail to read a named branch instead of the repository default. Pass --refresh-cache on tree, ls, cat, rg, sed, head, or tail to force refresh the selected branch before reading. Use wit cache owner/repo for an explicit cache refresh. No public TTL/max-age option is exposed.\n\nSnapshot backends: repo-reading commands default to the disk cache backend. Pass --backend memory (or set WIT_SNAPSHOT_BACKEND=memory) to load a public repo over the GitHub API into RAM with zero WIT_CACHE_DIR writes. Memory covers tree/ls/cat/rg/sed/head/tail, maps cache to an in-memory pin/prefetch, and lists branches via the GitHub API. search always uses the GitHub REST API (no disk cache). Repo-scoped commands accept owner/repo as a positional argument or via -r/--repo (if both are given they must match)."
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
        override_usage = "wit branches [OPTIONS] [REPO]",
        group(ArgGroup::new("repo_input").args(["repo", "repo_positional"]).required(true).multiple(true)),
        after_help = "Lists GitHub branches under refs/heads so you can choose an existing value for --branch on cache/read commands. Ahead, behind, and merged are computed against the repository default branch. Created is inferred from the first commit unique to the branch when one exists; otherwise it falls back to the branch tip commit time.\n\nPass the repository as a positional `owner/repo` or with -r/--repo. If both are given they must match.\n\nWith --backend memory (or WIT_SNAPSHOT_BACKEND=memory), branch listing uses the GitHub REST API and does not write WIT_CACHE_DIR.\n\nExamples:\n  wit branches ratatui/ratatui\n  wit branches -r ratatui/ratatui\n  wit branches octocat/Hello-World --backend memory"
    )]
    Branches {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: Option<String>,

        /// Repository in "owner/repo" format (alternative to -r/--repo)
        #[arg(value_name = "REPO")]
        repo_positional: Option<String>,

        /// Snapshot backend: disk (default cache) or memory (no filesystem cache)
        #[arg(long = "backend", value_name = "disk|memory")]
        backend: Option<String>,
    },
    #[command(
        name = "cache",
        visible_alias = "c",
        about = "Pin a repository snapshot (disk cache refresh, or memory open/prefetch)",
        override_usage = "wit <cache|c> [OPTIONS] [REPO]",
        group(ArgGroup::new("repo_input").args(["repo", "repo_positional"]).required(true).multiple(true)),
        after_help = "Disk backend: force-refresh the bare-repo cache for the repository default branch, or the selected branch when --branch is set. Memory backend: open the repo over the GitHub API into RAM (prefetch tree; no WIT_CACHE_DIR writes).\n\nPass the repository as a positional `owner/repo` or with -r/--repo. If both are given they must match.\n\nRepo-reading commands on disk normally serve cached content immediately and revalidate in the background. Pass --refresh-cache on tree, ls, cat, rg, sed, head, or tail when a disk read must wait for a fresh cache.\n\nExamples:\n  wit cache ratatui/ratatui\n  wit cache -r ratatui/ratatui --branch main\n  wit cache octocat/Hello-World --backend memory"
    )]
    Cache {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: Option<String>,

        /// Repository in "owner/repo" format (alternative to -r/--repo)
        #[arg(value_name = "REPO")]
        repo_positional: Option<String>,

        /// Branch name under refs/heads to cache instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Snapshot backend: disk (default cache) or memory (no filesystem cache)
        #[arg(long = "backend", value_name = "disk|memory")]
        backend: Option<String>,
    },
    #[command(
        name = "tree",
        visible_alias = "t",
        about = "Show the file tree of a repository (or subtree). Use -l for line counts",
        override_usage = "wit <tree|t> [OPTIONS] [REPO] [PATH]",
        after_help = "Start here to understand a repo's structure. Narrow with a path to avoid noise on large repos. Use -l to see file sizes and decide whether to cat or head.\n\nPass the repository as a positional `owner/repo` or with -r/--repo. If both are given they must match.\n\nExamples:\n  wit tree ratatui/ratatui\n  wit tree ratatui/ratatui src/widgets\n  wit tree -r ratatui/ratatui src\n  wit tree -l ratatui/ratatui src\n  wit tree octocat/Hello-World --backend memory"
    )]
    Tree {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: Option<String>,

        /// Branch name under refs/heads to read instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Force refresh the branch cache before reading
        #[arg(long = "refresh-cache", action = ArgAction::SetTrue)]
        refresh_cache: bool,

        /// Repository and optional path: `owner/repo [path]`, or just `[path]` when using -r/--repo
        #[arg(value_name = "REPO|PATH")]
        args: Vec<String>,

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
        override_usage = "wit ls [OPTIONS] [REPO] [PATH]",
        after_help = "Use to browse one directory level at a time. Unlike tree (recursive), ls shows only immediate children. Use -l to see line counts and token estimates before deciding what to read.\n\nPass the repository as a positional `owner/repo` or with -r/--repo. If both are given they must match.\n\nExamples:\n  wit ls ratatui/ratatui\n  wit ls ratatui/ratatui src/widgets\n  wit ls -r ratatui/ratatui src\n  wit ls -l ratatui/ratatui src\n  wit ls ratatui/ratatui --backend memory"
    )]
    Ls {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: Option<String>,

        /// Branch name under refs/heads to read instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Force refresh the branch cache before reading
        #[arg(long = "refresh-cache", action = ArgAction::SetTrue)]
        refresh_cache: bool,

        /// Repository and optional path: `owner/repo [path]`, or just `[path]` when using -r/--repo
        #[arg(value_name = "REPO|PATH")]
        args: Vec<String>,

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
        override_usage = "wit cat [OPTIONS] [REPO] <PATH>",
        after_help = "Use for small-to-medium files. For large files, prefer head/tail/sed to read specific ranges, or rg to search for patterns.\n\nPass the repository as a positional `owner/repo` or with -r/--repo. If both are given they must match.\n\nExamples:\n  wit cat ratatui/ratatui Cargo.toml\n  wit cat -n -r ratatui/ratatui src/lib.rs\n  wit cat -b ratatui/ratatui README.md\n  wit cat octocat/Hello-World README --backend memory"
    )]
    Cat {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: Option<String>,

        /// Branch name under refs/heads to read instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Force refresh the branch cache before reading
        #[arg(long = "refresh-cache", action = ArgAction::SetTrue)]
        refresh_cache: bool,

        /// Repository and path: `owner/repo PATH`, or just `PATH` when using -r/--repo
        #[arg(value_name = "REPO|PATH", num_args = 1..=2)]
        args: Vec<String>,

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
        override_usage = "wit rg [OPTIONS] <PATTERN> [REPO]",
        group(ArgGroup::new("repo_input").args(["repo", "repo_positional"]).required(true).multiple(true)),
        after_help = "The primary tool for locating code. Use -l to discover which files contain a pattern (cheaper than full matches). Use -g to restrict to file types. Combine -C for context around matches.\n\nPass the repository as a positional `owner/repo` after the pattern, or with -r/--repo. If both are given they must match.\n\nExamples:\n  wit rg 'impl Widget' ratatui/ratatui\n  wit rg -l 'struct.*Frame' -r ratatui/ratatui\n  wit rg -g '*.rs' -i 'todo' ratatui/ratatui\n  wit rg -C 3 'fn render' ratatui/ratatui\n  wit rg 'Hello' octocat/Hello-World --backend memory"
    )]
    Rg {
        /// Regex pattern to search for
        pattern: String,

        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: Option<String>,

        /// Repository in "owner/repo" format (alternative to -r/--repo)
        #[arg(value_name = "REPO")]
        repo_positional: Option<String>,

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

        /// Snapshot backend: disk (default cache) or memory (no filesystem cache)
        #[arg(long = "backend", value_name = "disk|memory")]
        backend: Option<String>,
    },
    #[command(
        name = "sed",
        about = "Extract or transform file content using sed scripts (POSIX-style, Rust regex)",
        override_usage = "wit sed [OPTIONS] [<SCRIPT>] [REPO] <PATH>",
        after_help = "Use for precise line-range extraction or text transformation. Regex uses Rust syntax, not POSIX BRE. Supports addresses, substitution, hold space, branching, and most POSIX commands.\n\nPass the repository as a positional `owner/repo` or with -r/--repo. If both are given they must match. Trailing --backend/--repo after the script/path are accepted (same as tree/ls/cat).\n\nExamples:\n  wit sed -n '320,460p' modal-labs/modal-client modal/image.py\n  wit sed -n '1,5p' octocat/Hello-World README --backend memory\n  wit sed --backend memory -e 's/Hello/Hi/' octocat/Hello-World README\n  wit sed -r ratatui/ratatui 's/Widget/Component/g' src/lib.rs"
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
        repo: Option<String>,

        /// Branch name under refs/heads to read instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Snapshot backend: disk (default cache) or memory (no filesystem cache)
        #[arg(long = "backend", value_name = "disk|memory")]
        backend: Option<String>,

        /// Positionals: with -r, `<SCRIPT> <PATH>` or `<PATH>` (-e/-f); without -r, `<SCRIPT> <REPO> <PATH>` or `<REPO> <PATH>` (-e/-f)
        #[arg(allow_hyphen_values = true)]
        args: Vec<String>,
    },
    #[command(
        name = "head",
        about = "Print the first N lines of a file (default: 10)",
        override_usage = "wit head [OPTIONS] [REPO] <PATH>",
        after_help = "Use to preview a file before deciding whether to read it fully. Pair with tail to read specific sections by position.\n\nPass the repository as a positional `owner/repo` or with -r/--repo. If both are given they must match.\n\nExamples:\n  wit head ratatui/ratatui src/lib.rs\n  wit head -n 50 -r ratatui/ratatui Cargo.toml\n  wit head -N ratatui/ratatui README.md\n  wit head -n 5 octocat/Hello-World README --backend memory"
    )]
    Head {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: Option<String>,

        /// Branch name under refs/heads to read instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Force refresh the branch cache before reading
        #[arg(long = "refresh-cache", action = ArgAction::SetTrue)]
        refresh_cache: bool,

        /// Repository and path: `owner/repo PATH`, or just `PATH` when using -r/--repo
        #[arg(value_name = "REPO|PATH", num_args = 1..=2)]
        args: Vec<String>,

        /// Number of lines to show (default: 10)
        #[arg(short = 'n', long = "lines", default_value_t = 10)]
        lines: usize,

        /// Number all output lines
        #[arg(short = 'N', long = "number")]
        number: bool,

        /// Snapshot backend: disk (default cache) or memory (no filesystem cache)
        #[arg(long = "backend", value_name = "disk|memory")]
        backend: Option<String>,
    },
    #[command(
        name = "tail",
        about = "Print the last N lines of a file, or from line N onward",
        override_usage = "wit tail [OPTIONS] [REPO] <PATH>",
        after_help = "Use -p to read from a specific line to end-of-file -- useful when you know a line number from rg output and want the surrounding code.\n\nPass the repository as a positional `owner/repo` or with -r/--repo. If both are given they must match.\n\nExamples:\n  wit tail ratatui/ratatui src/lib.rs\n  wit tail -n 20 -r ratatui/ratatui Cargo.toml\n  wit tail -p 100 ratatui/ratatui src/lib.rs\n  wit tail -n 5 octocat/Hello-World README --backend memory"
    )]
    Tail {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: Option<String>,

        /// Branch name under refs/heads to read instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Force refresh the branch cache before reading
        #[arg(long = "refresh-cache", action = ArgAction::SetTrue)]
        refresh_cache: bool,

        /// Repository and path: `owner/repo PATH`, or just `PATH` when using -r/--repo
        #[arg(value_name = "REPO|PATH", num_args = 1..=2)]
        args: Vec<String>,

        /// Number of lines to show (default: 10)
        #[arg(short = 'n', long = "lines", default_value_t = 10)]
        lines: usize,

        /// Start from line N (like tail -n +N)
        #[arg(short = 'p', long = "plus", value_name = "LINE")]
        from_line: Option<usize>,

        /// Number all output lines
        #[arg(short = 'N', long = "number")]
        number: bool,

        /// Snapshot backend: disk (default cache) or memory (no filesystem cache)
        #[arg(long = "backend", value_name = "disk|memory")]
        backend: Option<String>,
    },
    #[command(
        name = "ast",
        about = "AST-backed search: list definitions or run tree-sitter queries",
        after_help = "Structural search built on tree-sitter (rust, python, javascript, typescript, tsx, go, java, c). `symbols` indexes definitions with exact line ranges and nesting so you can read precisely the lines you need; `query` runs a raw tree-sitter S-expression query and prints every capture with its position.\n\nExamples:\n  wit ast symbols ratatui/ratatui src/widgets/block.rs\n  wit ast symbols ratatui/ratatui src/widgets --kind fn --name '^render'\n  wit ast symbols -r ratatui/ratatui --glob '*.rs' --json\n  wit ast query '(call_expression function: (identifier) @callee (#eq? @callee \"render\"))' ratatui/ratatui src --lang rust\n  wit ast query '(function_definition name: (identifier) @name)' owner/repo app.py"
    )]
    Ast {
        #[command(subcommand)]
        command: AstCommands,
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
        after_help = "Direct mode exposes eight typed snapshot-first tools, is the default, and is recommended for simple calls. Experimental Code Mode exposes one native JavaScript code tool. Both are structured, bounded, and resumable; neither requires an external JavaScript runtime.\n\nExamples:\n  wit mcp --transport stdio --mode direct   # Default eight-tool surface\n  wit mcp --transport stdio --mode code     # Experimental one-tool surface"
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

#[derive(Subcommand)]
enum AstCommands {
    #[command(
        about = "List definitions (functions, types, methods, ...) with exact line ranges",
        override_usage = "wit ast symbols [OPTIONS] [REPO] [PATH]"
    )]
    Symbols {
        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: Option<String>,

        /// Branch name under refs/heads to read instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Force refresh the branch cache before reading
        #[arg(long = "refresh-cache", action = ArgAction::SetTrue)]
        refresh_cache: bool,

        /// Repository and optional file or directory: `owner/repo [path]`, or just `[path]` when using -r/--repo
        #[arg(value_name = "REPO|PATH")]
        args: Vec<String>,

        /// Keep only these kind labels (fn, struct, class, method, ...); repeatable
        #[arg(short = 'k', long = "kind", value_name = "KIND", action = ArgAction::Append)]
        kind: Vec<String>,

        /// Keep only definitions whose name matches this regex
        #[arg(long = "name", value_name = "REGEX")]
        name: Option<String>,

        /// Glob pattern to filter files (e.g., "*.rs", "src/**")
        #[arg(short = 'g', long = "glob")]
        glob: Option<String>,

        /// Restrict to one language (rust, python, javascript, typescript, tsx, go, java, c)
        #[arg(long = "lang", value_name = "LANG")]
        lang: Option<String>,

        /// Maximum files to parse (default: 500)
        #[arg(long = "max-files", default_value_t = 500)]
        max_files: usize,

        /// Emit JSON instead of plaintext
        #[arg(long = "json")]
        json: bool,

        /// Snapshot backend: disk (default cache) or memory (no filesystem cache)
        #[arg(long = "backend", value_name = "disk|memory")]
        backend: Option<String>,
    },
    #[command(
        about = "Run a tree-sitter S-expression query and print every capture",
        override_usage = "wit ast query [OPTIONS] <QUERY> [REPO] [PATH]"
    )]
    Query {
        /// tree-sitter query, e.g. '(function_item name: (identifier) @name)'
        query: String,

        /// Repository in "owner/repo" format
        #[arg(short = 'r', long = "repo")]
        repo: Option<String>,

        /// Branch name under refs/heads to read instead of the repository default branch
        #[arg(long = "branch", value_name = "BRANCH")]
        branch: Option<String>,

        /// Force refresh the branch cache before reading
        #[arg(long = "refresh-cache", action = ArgAction::SetTrue)]
        refresh_cache: bool,

        /// Repository and optional file or directory: `owner/repo [path]`, or just `[path]` when using -r/--repo
        #[arg(value_name = "REPO|PATH")]
        args: Vec<String>,

        /// Language the query is written for (required unless PATH is a single source file)
        #[arg(long = "lang", value_name = "LANG")]
        lang: Option<String>,

        /// Glob pattern to filter files (e.g., "*.rs", "src/**")
        #[arg(short = 'g', long = "glob")]
        glob: Option<String>,

        /// Maximum files to parse (default: 500)
        #[arg(long = "max-files", default_value_t = 500)]
        max_files: usize,

        /// Emit JSON instead of plaintext
        #[arg(long = "json")]
        json: bool,

        /// Snapshot backend: disk (default cache) or memory (no filesystem cache)
        #[arg(long = "backend", value_name = "disk|memory")]
        backend: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum McpTransport {
    Stdio,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum McpMode {
    /// Recommended default: eight snapshot-first repository tools
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

/// Resolve repository from optional `-r/--repo` and optional positional `owner/repo`.
///
/// If both are present they must match; if neither is present, return an error.
fn resolve_repo(flag: Option<String>, positional: Option<String>) -> anyhow::Result<String> {
    match (flag, positional) {
        (None, None) => anyhow::bail!(
            "missing repository: pass owner/repo as a positional argument or with -r/--repo"
        ),
        (Some(repo), None) | (None, Some(repo)) => Ok(repo),
        (Some(flag_repo), Some(positional_repo)) if flag_repo == positional_repo => Ok(flag_repo),
        (Some(flag_repo), Some(positional_repo)) => anyhow::bail!(
            "conflicting repository arguments: -r/--repo '{flag_repo}' vs positional '{positional_repo}'"
        ),
    }
}

/// True when a non-flag positional appears before `-r`/`--repo` for the subcommand.
///
/// Distinguishes `wit tree other/repo -r owner/repo` (positional repo) from
/// `wit tree -r owner/repo src/widgets` (path after `-r`).
fn nonflag_positional_before_repo_flag(argv: &[impl AsRef<str>]) -> bool {
    let argv: Vec<&str> = argv.iter().map(AsRef::as_ref).collect();
    let mut after_subcommand = false;
    let mut i = 1usize; // skip binary name
    while i < argv.len() {
        let a = argv[i];
        if !after_subcommand {
            if a == "--ignore" {
                i += 2;
                continue;
            }
            if a.starts_with("--ignore=") {
                i += 1;
                continue;
            }
            if a.starts_with('-') {
                i += 1;
                continue;
            }
            after_subcommand = true;
            i += 1;
            continue;
        }

        if a == "-r" || a == "--repo" || a.starts_with("--repo=") {
            return false;
        }
        if a == "--branch" || a == "--backend" {
            i += 2;
            continue;
        }
        if a.starts_with("--branch=") || a.starts_with("--backend=") {
            i += 1;
            continue;
        }
        if matches!(
            a,
            "--refresh-cache" | "--long" | "-l" | "-h" | "--help" | "-n" | "--number" | "-N"
        ) {
            i += 1;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        return true;
    }
    false
}

/// Tree/ls style: `wit tree owner/repo [path]` or `wit tree -r owner/repo [path]`.
///
/// `argv` is the original process argv (including binary name) used to detect
/// `wit tree other/repo -r owner/repo` as a conflicting positional repo, while
/// still treating `wit tree -r owner/repo src/widgets` as a path.
fn resolve_repo_and_optional_path(
    flag: Option<String>,
    args: Vec<String>,
    argv: &[impl AsRef<str>],
) -> anyhow::Result<(String, Option<String>)> {
    match flag {
        None => match args.len() {
            0 => Err(anyhow::anyhow!(
                "missing repository: pass owner/repo as a positional argument or with -r/--repo"
            )),
            1 => Ok((args[0].clone(), None)),
            2 => Ok((args[0].clone(), Some(args[1].clone()))),
            _ => Err(anyhow::anyhow!(
                "too many arguments: expected owner/repo [path]"
            )),
        },
        Some(flag_repo) => match args.len() {
            0 => Ok((flag_repo, None)),
            1 if args[0] == flag_repo => Ok((flag_repo, None)),
            1 if nonflag_positional_before_repo_flag(argv) => {
                // Positional appeared before -r/--repo → treat as repo, not path.
                let repo = resolve_repo(Some(flag_repo), Some(args[0].clone()))?;
                Ok((repo, None))
            }
            1 => Ok((flag_repo, Some(args[0].clone()))),
            2 => {
                let repo = resolve_repo(Some(flag_repo), Some(args[0].clone()))?;
                Ok((repo, Some(args[1].clone())))
            }
            _ => Err(anyhow::anyhow!(
                "too many arguments: expected [-r owner/repo] [owner/repo] [path]"
            )),
        },
    }
}

/// Cat/head/tail style: `wit cat owner/repo PATH` or `wit cat -r owner/repo PATH`.
fn resolve_repo_and_required_path(
    flag: Option<String>,
    args: Vec<String>,
) -> anyhow::Result<(String, String)> {
    match flag {
        None => match args.len() {
            2 => Ok((args[0].clone(), args[1].clone())),
            0 | 1 => Err(anyhow::anyhow!(
                "missing arguments: expected owner/repo PATH (or -r owner/repo PATH)"
            )),
            _ => Err(anyhow::anyhow!(
                "too many arguments: expected owner/repo PATH"
            )),
        },
        Some(flag_repo) => match args.len() {
            1 => Ok((flag_repo, args[0].clone())),
            2 => {
                let repo = resolve_repo(Some(flag_repo), Some(args[0].clone()))?;
                Ok((repo, args[1].clone()))
            }
            0 => Err(anyhow::anyhow!(
                "missing path: expected PATH after repository"
            )),
            _ => Err(anyhow::anyhow!(
                "too many arguments: expected [-r owner/repo] [owner/repo] PATH"
            )),
        },
    }
}

/// Rg style: pattern is separate; repo via `-r` and/or trailing positional.
fn resolve_repo_only(flag: Option<String>, positional: Option<String>) -> anyhow::Result<String> {
    resolve_repo(flag, positional)
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

/// Rough token estimate from a byte size (~4 bytes per token). The memory
/// backend only knows sizes without fetching blobs, so this is the budgeting
/// hint the disk backend's `lines * 5` provides once content is available.
/// The URL API (`showcase/url-api`) prints the same figure.
fn estimate_tokens_from_bytes(bytes: u64) -> u64 {
    bytes.div_ceil(4)
}

fn print_snapshot_ls(entries: &[DirEntry], long: bool) {
    if long {
        for entry in entries {
            match entry.kind {
                EntryKind::Dir => println!("            {}/", entry.name),
                EntryKind::File => {
                    if let Some(size) = entry.size_bytes {
                        println!(
                            "{:>8} B  {}  (~{} tok)",
                            size,
                            entry.name,
                            estimate_tokens_from_bytes(size)
                        );
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
    ignore_patterns: &[String],
) -> anyhow::Result<()> {
    let view = snap.tree(path).map_err(|err| anyhow::anyhow!(err))?;
    let ignore = IgnoreMatcher::new(ignore_patterns)?;
    println!("{}", view.root);
    let mut dirs: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for entry in &view.entries {
        if ignore.is_ignored(&entry.path) {
            continue;
        }
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
        let label = if long {
            if let Some(size) = entry.size_bytes {
                format!(
                    "{relative} ({size} B, ~{} tok)",
                    estimate_tokens_from_bytes(size)
                )
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

/// What `wit ast` does with each visited file.
enum AstJob {
    Symbols(SymbolFilter),
    Query {
        language: AstLanguage,
        query: String,
    },
}

#[derive(serde::Serialize)]
struct AstFileReport {
    path: String,
    language: &'static str,
    total_lines: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    symbols: Vec<ast::AstSymbol>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    captures: Vec<ast::AstCapture>,
}

/// Shared `wit ast symbols|query` driver over disk or memory snapshots.
async fn run_ast(
    command: AstCommands,
    ignore_patterns: &[String],
    argv: &[impl AsRef<str>],
) -> anyhow::Result<()> {
    let (repo, branch, refresh_cache, args, glob, lang, max_files, json, backend, job) =
        match command {
            AstCommands::Symbols {
                repo,
                branch,
                refresh_cache,
                args,
                kind,
                name,
                glob,
                lang,
                max_files,
                json,
                backend,
            } => {
                let filter = SymbolFilter {
                    kinds: kind,
                    name: match name {
                        Some(pattern) => Some(
                            regex::Regex::new(&pattern)
                                .map_err(|err| anyhow::anyhow!("invalid --name regex: {err}"))?,
                        ),
                        None => None,
                    },
                };
                (
                    repo,
                    branch,
                    refresh_cache,
                    args,
                    glob,
                    lang,
                    max_files,
                    json,
                    backend,
                    AstJob::Symbols(filter),
                )
            }
            AstCommands::Query {
                query,
                repo,
                branch,
                refresh_cache,
                args,
                lang,
                glob,
                max_files,
                json,
                backend,
            } => (
                repo,
                branch,
                refresh_cache,
                args,
                glob,
                lang,
                max_files,
                json,
                backend,
                // The language is resolved below once the path is known.
                AstJob::Query {
                    language: AstLanguage::Rust,
                    query,
                },
            ),
        };
    let (repo, path) = resolve_repo_and_optional_path(repo, args, argv)?;
    let language_filter = match lang.as_deref() {
        Some(name) => Some(AstLanguage::from_name(name).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown --lang '{name}'; supported: {}",
                ast::supported_languages_summary()
            )
        })?),
        None => None,
    };
    let job = match job {
        AstJob::Query { query, .. } => {
            let language = language_filter
                .or_else(|| path.as_deref().and_then(AstLanguage::from_path))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "wit ast query needs --lang LANG unless PATH is a single source file (supported: {})",
                        ast::supported_languages_summary()
                    )
                })?;
            ast::validate_query(language, &query)?;
            AstJob::Query { language, query }
        }
        symbols => symbols,
    };
    let walk = BlobWalkOptions {
        path_prefix: path.clone(),
        glob,
        ignore: ignore_patterns.to_vec(),
        max_bytes: ast::MAX_AST_SOURCE_BYTES,
    };

    let mut reports: Vec<AstFileReport> = Vec::new();
    let mut parsed = 0usize;
    let mut truncated = false;
    let mut visit = |file_path: &str, text: &str| -> anyhow::Result<bool> {
        let Some(file_language) = AstLanguage::from_path(file_path) else {
            return Ok(true);
        };
        if language_filter.is_some_and(|wanted| wanted != file_language) {
            return Ok(true);
        }
        if parsed >= max_files {
            truncated = true;
            return Ok(false);
        }
        let report = match &job {
            AstJob::Symbols(filter) => {
                parsed += 1;
                let symbols = ast::symbols(file_language, text, filter)?;
                AstFileReport {
                    path: file_path.to_string(),
                    language: file_language.name(),
                    total_lines: text.lines().count(),
                    symbols,
                    captures: Vec::new(),
                }
            }
            AstJob::Query { language, query } => {
                if *language != file_language {
                    return Ok(true);
                }
                parsed += 1;
                let captures = ast::run_query(file_language, text, query)?;
                AstFileReport {
                    path: file_path.to_string(),
                    language: file_language.name(),
                    total_lines: text.lines().count(),
                    symbols: Vec::new(),
                    captures,
                }
            }
        };
        reports.push(report);
        Ok(true)
    };

    match CliSnapshotBackend::from_env_or_flag(backend.as_deref()).map_err(anyhow::Error::msg)? {
        CliSnapshotBackend::Disk => {
            let repository = cache_github_repo(
                &repo,
                cache_branch_selection(branch),
                repo_cache_mode(refresh_cache),
            )
            .await?;
            walk_text_blobs(&repository, &walk, &mut visit)?;
        }
        CliSnapshotBackend::Memory => {
            let snap = open_memory_snapshot(&repo, branch.as_deref()).await?;
            print_memory_provenance(snap.provenance());
            walk_memory_text_blobs(&snap, &walk, &mut visit).await?;
        }
    }

    if let Some(path) = &path
        && reports.is_empty()
        && AstLanguage::from_path(path).is_none()
        && !path.contains('/')
        && parsed == 0
    {
        // A bare file name with no grammar: say so instead of printing nothing.
        eprintln!(
            "note: no parseable files under '{path}' (supported: {})",
            ast::supported_languages_summary()
        );
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        let mut blocks = Vec::new();
        for report in &reports {
            match &job {
                AstJob::Symbols(_) => {
                    if report.symbols.is_empty() && path.as_deref() != Some(report.path.as_str()) {
                        continue;
                    }
                    let language =
                        AstLanguage::from_name(report.language).unwrap_or(AstLanguage::Rust);
                    blocks.push(ast::format_symbols(
                        &report.path,
                        language,
                        &report.symbols,
                        report.total_lines,
                    ));
                }
                AstJob::Query { .. } => {
                    if report.captures.is_empty() {
                        continue;
                    }
                    blocks.push(ast::format_captures(&report.path, &report.captures));
                }
            }
        }
        if !blocks.is_empty() {
            println!("{}", blocks.join("\n\n"));
        }
    }
    if truncated {
        eprintln!(
            "note: stopped after {max_files} files (raise --max-files or narrow PATH/--glob)"
        );
    }
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
    let argv: Vec<String> = std::env::args().collect();
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
        Commands::Branches {
            repo,
            repo_positional,
            backend,
        } => {
            let repo = resolve_repo(repo, repo_positional)?;
            let branches = match CliSnapshotBackend::from_env_or_flag(backend.as_deref())
                .map_err(anyhow::Error::msg)?
            {
                CliSnapshotBackend::Disk => list_remote_branches(&repo)?,
                CliSnapshotBackend::Memory => list_remote_branches_api(&repo).await?,
            };
            print_branch_results(&branches);
        }
        Commands::Cache {
            repo,
            repo_positional,
            branch,
            backend,
        } => {
            let repo = resolve_repo(repo, repo_positional)?;
            match CliSnapshotBackend::from_env_or_flag(backend.as_deref())
                .map_err(anyhow::Error::msg)?
            {
                CliSnapshotBackend::Disk => {
                    let repo = cache_github_repo(
                        &repo,
                        cache_branch_selection(branch),
                        CacheAcquisitionMode::ForceInvalidate,
                    )
                    .await?;
                    println!("Cached repository: {}", repo.path().display());
                }
                CliSnapshotBackend::Memory => {
                    let snap = open_memory_snapshot(&repo, branch.as_deref()).await?;
                    print_memory_provenance(snap.provenance());
                    // Prefetch tree is already done by open; warm a few root blobs when cheap.
                    let root = snap.list(None).map_err(|err| anyhow::anyhow!(err))?;
                    let mut warmed = 0usize;
                    for entry in root.into_iter().take(8) {
                        if entry.kind == EntryKind::File && snap.read(&entry.path).await.is_ok() {
                            warmed += 1;
                        }
                    }
                    println!(
                        "Pinned memory snapshot {}@{} (warmed {warmed} root blobs)",
                        snap.provenance().repo,
                        snap.provenance().commit_sha
                    );
                }
            }
        }
        Commands::Tree {
            repo,
            branch,
            refresh_cache,
            args,
            long,
            backend,
        } => {
            let (repo, path) = resolve_repo_and_optional_path(repo, args, &argv)?;
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
                    print_snapshot_tree(&snap, path.as_deref(), long, &ignore_patterns)?;
                }
            }
        }
        Commands::Ls {
            repo,
            branch,
            refresh_cache,
            args,
            long,
            backend,
        } => {
            let (repo, path) = resolve_repo_and_optional_path(repo, args, &argv)?;
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
                    let ignore = IgnoreMatcher::new(&ignore_patterns)?;
                    let entries: Vec<_> = entries
                        .into_iter()
                        .filter(|entry| {
                            let full = match path.as_deref() {
                                Some(base) if !base.is_empty() => {
                                    format!("{}/{}", base.trim_end_matches('/'), entry.name)
                                }
                                _ => entry.name.clone(),
                            };
                            !ignore.is_ignored(&full)
                        })
                        .collect();
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
            args,
            number,
            number_nonblank,
            squeeze_blank,
            show_ends,
            show_tabs,
            show_all,
            backend,
        } => {
            let (repo, path) = resolve_repo_and_required_path(repo, args)?;
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
                    read_memory_text(&snap, &path, &ignore_patterns).await?
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
            repo_positional,
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
            backend,
        } => {
            let repo = resolve_repo_only(repo, repo_positional)?;
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

            #[allow(clippy::large_enum_variant)]
            enum RgSource {
                Disk(gix::Repository),
                Memory(wit_snapshot::MemorySnapshot<wit_snapshot::ReqwestGitHubClient>),
            }

            let source = match CliSnapshotBackend::from_env_or_flag(backend.as_deref())
                .map_err(anyhow::Error::msg)?
            {
                CliSnapshotBackend::Disk => {
                    let repository = cache_github_repo(
                        &repo,
                        cache_branch_selection(branch),
                        repo_cache_mode(refresh_cache),
                    )
                    .await?;
                    RgSource::Disk(repository)
                }
                CliSnapshotBackend::Memory => {
                    let snap = open_memory_snapshot(&repo, branch.as_deref()).await?;
                    print_memory_provenance(snap.provenance());
                    RgSource::Memory(snap)
                }
            };

            let result = match &source {
                RgSource::Disk(repository) => grep_repo_with_options(repository, &pattern, &opts)?,
                RgSource::Memory(snap) => grep_memory_snapshot(snap, &pattern, &opts).await?,
            };

            match result {
                GrepResult::Matches(matches) => {
                    if matches.is_empty() {
                        return Ok(());
                    }

                    let mut current_file = String::new();
                    let has_context = before_context > 0 || after_context > 0 || context > 0;

                    for m in matches {
                        if m.path != current_file {
                            if !current_file.is_empty() && has_context {
                                println!();
                            }
                            current_file = m.path.clone();
                        }

                        if m.line_number == 0 && m.content == "--" {
                            println!("{}", "--".dimmed());
                            continue;
                        }

                        let line_num = m.line_number.to_string();
                        if m.is_context {
                            println!(
                                "{}{}{}{} {}",
                                m.path.magenta(),
                                "-".dimmed(),
                                line_num.dimmed(),
                                "-".dimmed(),
                                m.content.dimmed()
                            );
                        } else {
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
                            let content = match &source {
                                RgSource::Disk(repository) => read_file(repository, file).ok(),
                                RgSource::Memory(snap) => {
                                    read_memory_text(snap, file, &[]).await.ok()
                                }
                            };
                            match content {
                                Some(text) => {
                                    let lines = text.lines().count();
                                    let tokens = lines * 5;
                                    println!(
                                        "{:>6} ln  {:<40} (~{} tok)",
                                        lines,
                                        file.magenta(),
                                        tokens
                                    );
                                }
                                None => {
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
            args,
            lines,
            number,
            backend,
        } => {
            let (repo, path) = resolve_repo_and_required_path(repo, args)?;
            let output = match CliSnapshotBackend::from_env_or_flag(backend.as_deref())
                .map_err(anyhow::Error::msg)?
            {
                CliSnapshotBackend::Disk => {
                    let repository = cache_github_repo(
                        &repo,
                        cache_branch_selection(branch),
                        repo_cache_mode(refresh_cache),
                    )
                    .await?;
                    head_with_ignore(&repository, &path, lines, number, &ignore_patterns)?
                }
                CliSnapshotBackend::Memory => {
                    let snap = open_memory_snapshot(&repo, branch.as_deref()).await?;
                    print_memory_provenance(snap.provenance());
                    let content = read_memory_text(&snap, &path, &ignore_patterns).await?;
                    head_from_text(&content, lines, number)
                }
            };
            println!("{}", output);
        }
        Commands::Sed {
            quiet,
            mut refresh_cache,
            expressions,
            files,
            repo,
            branch,
            backend,
            args,
        } => {
            let extracted = extract_sed_inline_flags(args)?;
            let mut effective_ignore_patterns = ignore_patterns.clone();
            effective_ignore_patterns.extend(extracted.ignores);
            refresh_cache = refresh_cache || extracted.refresh_cache;
            let backend = merge_optional_flags("backend", backend, extracted.backend)?;
            let branch = merge_optional_flags("branch", branch, extracted.branch)?;
            let repo = match (repo, extracted.repo) {
                (None, None) => None,
                (Some(a), None) | (None, Some(a)) => Some(a),
                (Some(a), Some(b)) => Some(resolve_repo(Some(a), Some(b))?),
            };

            let (repo, scripts, path) =
                parse_sed_invocation(repo, expressions, files, extracted.args)?;
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
                    read_file_with_ignore(&repository, &path, &effective_ignore_patterns)?
                }
                CliSnapshotBackend::Memory => {
                    let snap = open_memory_snapshot(&repo, branch.as_deref()).await?;
                    print_memory_provenance(snap.provenance());
                    read_memory_text(&snap, &path, &effective_ignore_patterns).await?
                }
            };
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
            args,
            lines,
            from_line,
            number,
            backend,
        } => {
            let (repo, path) = resolve_repo_and_required_path(repo, args)?;
            let output = match CliSnapshotBackend::from_env_or_flag(backend.as_deref())
                .map_err(anyhow::Error::msg)?
            {
                CliSnapshotBackend::Disk => {
                    let repository = cache_github_repo(
                        &repo,
                        cache_branch_selection(branch),
                        repo_cache_mode(refresh_cache),
                    )
                    .await?;
                    tail_with_ignore(
                        &repository,
                        &path,
                        lines,
                        from_line,
                        number,
                        &ignore_patterns,
                    )?
                }
                CliSnapshotBackend::Memory => {
                    let snap = open_memory_snapshot(&repo, branch.as_deref()).await?;
                    print_memory_provenance(snap.provenance());
                    let content = read_memory_text(&snap, &path, &ignore_patterns).await?;
                    tail_from_text(&content, lines, from_line, number)
                }
            };
            println!("{}", output);
        }
        Commands::CacheRevalidate { repo, branch } => {
            revalidate_github_repo(&repo, cache_branch_selection(branch))?;
        }
        Commands::Ast { command } => run_ast(command, &ignore_patterns, &argv).await?,
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
    repo_flag: Option<String>,
    expressions: Vec<String>,
    files: Vec<String>,
    args: Vec<String>,
) -> anyhow::Result<(String, Vec<String>, String)> {
    let mut scripts = Vec::new();
    scripts.extend(expressions);

    for file in files {
        let content = fs::read_to_string(&file)
            .map_err(|e| anyhow::anyhow!("failed to read sed script file '{}': {}", file, e))?;
        scripts.push(content);
    }

    let has_ef = !scripts.is_empty();

    let (repo, script_arg, path) = match (repo_flag.as_deref(), has_ef, args.as_slice()) {
        // -r REPO SCRIPT PATH
        (Some(repo), false, [script, path]) => {
            (repo.to_string(), Some(script.to_string()), path.to_string())
        }
        // -r REPO PATH (with -e/-f)
        (Some(repo), true, [path]) => (repo.to_string(), None, path.to_string()),
        // -r REPO REPO SCRIPT PATH / -r REPO REPO PATH
        (Some(repo), false, [pos, script, path]) => (
            resolve_repo(Some(repo.to_string()), Some(pos.to_string()))?,
            Some(script.to_string()),
            path.to_string(),
        ),
        (Some(repo), true, [pos, path]) => (
            resolve_repo(Some(repo.to_string()), Some(pos.to_string()))?,
            None,
            path.to_string(),
        ),
        // SCRIPT REPO PATH
        (None, false, [script, repo, path]) => {
            (repo.to_string(), Some(script.to_string()), path.to_string())
        }
        // REPO PATH (with -e/-f)
        (None, true, [repo, path]) => (repo.to_string(), None, path.to_string()),
        _ => {
            return Err(anyhow::anyhow!(
                "sed expects [<SCRIPT>] <REPO> <PATH>, or -r <REPO> [<SCRIPT>] <PATH> (use -e/-f for the script)"
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

    Ok((repo, scripts, path))
}

fn merge_optional_flags(
    name: &str,
    clap_value: Option<String>,
    extracted: Option<String>,
) -> anyhow::Result<Option<String>> {
    match (clap_value, extracted) {
        (None, None) => Ok(None),
        (Some(value), None) | (None, Some(value)) => Ok(Some(value)),
        (Some(a), Some(b)) if a == b => Ok(Some(a)),
        (Some(a), Some(b)) => anyhow::bail!("conflicting --{name} values: '{a}' vs '{b}'"),
    }
}

struct SedInlineFlags {
    args: Vec<String>,
    ignores: Vec<String>,
    repo: Option<String>,
    branch: Option<String>,
    backend: Option<String>,
    refresh_cache: bool,
}

/// Pull flags that `allow_hyphen_values` on sed positionals would otherwise swallow.
///
/// Enables trailing forms like:
/// `wit sed -n '1,5p' owner/repo README --backend memory`
fn extract_sed_inline_flags(args: Vec<String>) -> anyhow::Result<SedInlineFlags> {
    let mut remaining_args = Vec::new();
    let mut ignores = Vec::new();
    let mut repo = None;
    let mut branch = None;
    let mut backend = None;
    let mut refresh_cache = false;
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

        if arg == "--backend" {
            let value = iter
                .next()
                .ok_or_else(|| anyhow::anyhow!("--backend requires a value"))?;
            if backend.is_some() {
                anyhow::bail!("duplicate --backend flag in sed arguments");
            }
            backend = Some(value);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--backend=") {
            if value.is_empty() {
                return Err(anyhow::anyhow!("--backend requires a value"));
            }
            if backend.is_some() {
                anyhow::bail!("duplicate --backend flag in sed arguments");
            }
            backend = Some(value.to_string());
            continue;
        }

        if arg == "-r" || arg == "--repo" {
            let value = iter
                .next()
                .ok_or_else(|| anyhow::anyhow!("{arg} requires a value"))?;
            if repo.is_some() {
                anyhow::bail!("duplicate -r/--repo flag in sed arguments");
            }
            repo = Some(value);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--repo=") {
            if value.is_empty() {
                return Err(anyhow::anyhow!("--repo requires a value"));
            }
            if repo.is_some() {
                anyhow::bail!("duplicate -r/--repo flag in sed arguments");
            }
            repo = Some(value.to_string());
            continue;
        }

        if arg == "--branch" {
            let value = iter
                .next()
                .ok_or_else(|| anyhow::anyhow!("--branch requires a value"))?;
            if branch.is_some() {
                anyhow::bail!("duplicate --branch flag in sed arguments");
            }
            branch = Some(value);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--branch=") {
            if value.is_empty() {
                return Err(anyhow::anyhow!("--branch requires a value"));
            }
            if branch.is_some() {
                anyhow::bail!("duplicate --branch flag in sed arguments");
            }
            branch = Some(value.to_string());
            continue;
        }

        if arg == "--refresh-cache" {
            refresh_cache = true;
            continue;
        }

        remaining_args.push(arg);
    }

    Ok(SedInlineFlags {
        args: remaining_args,
        ignores,
        repo,
        branch,
        backend,
        refresh_cache,
    })
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
                "ast", "skill", "mcp"
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
                !repo.is_required_set(),
                "{command_name} should allow positional owner/repo instead of requiring --repo"
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
                backend,
                args,
            } => {
                let extracted =
                    extract_sed_inline_flags(args).expect("inline sed flags should parse");

                assert!(quiet);
                assert!(refresh_cache);
                assert!(expressions.is_empty());
                assert!(files.is_empty());
                assert_eq!(repo, Some("owner/repo".to_string()));
                assert_eq!(branch, None);
                assert_eq!(backend, None);
                assert_eq!(extracted.ignores, vec!["vendor".to_string()]);
                assert_eq!(extracted.args, vec!["1,3p", "src/lib.rs"]);
                let (resolved, scripts, path) =
                    parse_sed_invocation(repo, expressions, files, extracted.args)
                        .expect("sed invocation should resolve");
                assert_eq!(resolved, "owner/repo");
                assert_eq!(scripts, vec!["1,3p".to_string()]);
                assert_eq!(path, "src/lib.rs");
            }
            _ => panic!("expected sed command"),
        }
    }

    #[test]
    fn sed_trailing_backend_and_repo_flags_are_extracted() {
        // Flag-first form still parses via clap.
        let flag_first = WitCli::try_parse_from([
            "wit",
            "sed",
            "--backend",
            "memory",
            "-n",
            "1,5p",
            "octocat/Hello-World",
            "README",
        ])
        .expect("flag-first sed --backend should parse");
        match flag_first.command {
            Commands::Sed {
                backend,
                repo,
                args,
                expressions,
                files,
                ..
            } => {
                assert_eq!(backend.as_deref(), Some("memory"));
                let extracted = extract_sed_inline_flags(args).unwrap();
                let backend = merge_optional_flags("backend", backend, extracted.backend).unwrap();
                assert_eq!(backend.as_deref(), Some("memory"));
                assert_eq!(
                    CliSnapshotBackend::from_env_or_flag(backend.as_deref()).unwrap(),
                    CliSnapshotBackend::Memory
                );
                let (resolved, scripts, path) =
                    parse_sed_invocation(repo, expressions, files, extracted.args).unwrap();
                assert_eq!(resolved, "octocat/Hello-World");
                assert_eq!(scripts, vec!["1,5p".to_string()]);
                assert_eq!(path, "README");
            }
            _ => panic!("expected sed"),
        }

        // Trailing flags are swallowed into positionals by allow_hyphen_values; extract them.
        let trailing = WitCli::try_parse_from([
            "wit",
            "sed",
            "-n",
            "1,5p",
            "octocat/Hello-World",
            "README",
            "--backend",
            "memory",
        ])
        .expect("trailing sed --backend should parse into args");
        match trailing.command {
            Commands::Sed {
                backend,
                repo,
                args,
                expressions,
                files,
                ..
            } => {
                assert_eq!(backend, None, "trailing --backend is not seen by clap");
                assert!(
                    args.iter().any(|a| a == "--backend"),
                    "trailing --backend should land in sed args: {args:?}"
                );
                let extracted = extract_sed_inline_flags(args).unwrap();
                assert_eq!(extracted.backend.as_deref(), Some("memory"));
                let backend = merge_optional_flags("backend", backend, extracted.backend).unwrap();
                assert_eq!(
                    CliSnapshotBackend::from_env_or_flag(backend.as_deref()).unwrap(),
                    CliSnapshotBackend::Memory
                );
                let (resolved, scripts, path) =
                    parse_sed_invocation(repo, expressions, files, extracted.args).unwrap();
                assert_eq!(resolved, "octocat/Hello-World");
                assert_eq!(scripts, vec!["1,5p".to_string()]);
                assert_eq!(path, "README");
            }
            _ => panic!("expected sed"),
        }

        let trailing_repo = WitCli::try_parse_from([
            "wit",
            "sed",
            "-n",
            "1,5p",
            "README",
            "--repo",
            "octocat/Hello-World",
            "--backend",
            "memory",
        ])
        .expect("trailing --repo/--backend should parse");
        match trailing_repo.command {
            Commands::Sed {
                backend,
                repo,
                args,
                expressions,
                files,
                ..
            } => {
                let extracted = extract_sed_inline_flags(args).unwrap();
                let backend = merge_optional_flags("backend", backend, extracted.backend).unwrap();
                let repo = match (repo, extracted.repo) {
                    (None, None) => None,
                    (Some(a), None) | (None, Some(a)) => Some(a),
                    (Some(a), Some(b)) => Some(resolve_repo(Some(a), Some(b)).unwrap()),
                };
                assert_eq!(backend.as_deref(), Some("memory"));
                let (resolved, _, path) =
                    parse_sed_invocation(repo, expressions, files, extracted.args).unwrap();
                assert_eq!(resolved, "octocat/Hello-World");
                assert_eq!(path, "README");
            }
            _ => panic!("expected sed"),
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
                args,
                ..
            } => {
                assert!(refresh_cache);
                assert_eq!(args, vec!["src".to_string()]);
                let argv = ["wit", "tree", "-r", "owner/repo", "--refresh-cache", "src"];
                let (repo, path) =
                    resolve_repo_and_optional_path(Some("owner/repo".to_string()), args, &argv)
                        .unwrap();
                assert_eq!(repo, "owner/repo");
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
                args,
                ..
            } => {
                assert!(refresh_cache);
                assert_eq!(args, vec!["README.md".to_string()]);
                let (repo, path) =
                    resolve_repo_and_required_path(Some("owner/repo".to_string()), args).unwrap();
                assert_eq!(repo, "owner/repo");
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
        assert!(
            !repo.is_required_set(),
            "branches should allow positional owner/repo"
        );
        assert_eq!(repo.get_index(), None);
        assert!(
            branches.get_visible_aliases().next().is_none(),
            "branches should not have a short alias"
        );

        let parsed = WitCli::try_parse_from(["wit", "branches", "-r", "owner/repo"])
            .expect("branches -r owner/repo should parse");
        assert!(matches!(
            parsed.command,
            Commands::Branches {
                repo: Some(ref value),
                ..
            } if value == "owner/repo"
        ));

        let positional = WitCli::try_parse_from(["wit", "branches", "owner/repo"])
            .expect("branches owner/repo should parse");
        assert!(matches!(
            positional.command,
            Commands::Branches {
                repo_positional: Some(ref value),
                ..
            } if value == "owner/repo"
        ));

        let missing_repo = match WitCli::try_parse_from(["wit", "branches"]) {
            Ok(_) => panic!("branches should require a repository"),
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

        assert!(help.contains("Usage: wit branches [OPTIONS] [REPO]"));
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
    fn snapshot_backend_flag_parses_for_repo_reading_commands() {
        let tree =
            WitCli::try_parse_from(["wit", "tree", "-r", "owner/repo", "--backend", "memory"])
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

        let rg = WitCli::try_parse_from([
            "wit",
            "rg",
            "pattern",
            "-r",
            "owner/repo",
            "--backend",
            "memory",
        ])
        .expect("rg --backend memory should parse");
        assert!(matches!(
            rg.command,
            Commands::Rg {
                backend: Some(ref value),
                ..
            } if value == "memory"
        ));

        let sed = WitCli::try_parse_from([
            "wit",
            "sed",
            "-e",
            "s/a/b/",
            "-r",
            "owner/repo",
            "--backend",
            "memory",
            "README.md",
        ])
        .expect("sed --backend memory should parse");
        assert!(matches!(
            sed.command,
            Commands::Sed {
                backend: Some(ref value),
                ..
            } if value == "memory"
        ));

        let head = WitCli::try_parse_from([
            "wit",
            "head",
            "-r",
            "owner/repo",
            "README.md",
            "--backend",
            "memory",
        ])
        .expect("head --backend memory should parse");
        assert!(matches!(
            head.command,
            Commands::Head {
                backend: Some(ref value),
                ..
            } if value == "memory"
        ));

        let tail = WitCli::try_parse_from([
            "wit",
            "tail",
            "-r",
            "owner/repo",
            "README.md",
            "--backend",
            "memory",
        ])
        .expect("tail --backend memory should parse");
        assert!(matches!(
            tail.command,
            Commands::Tail {
                backend: Some(ref value),
                ..
            } if value == "memory"
        ));

        let cache =
            WitCli::try_parse_from(["wit", "cache", "-r", "owner/repo", "--backend", "memory"])
                .expect("cache --backend memory should parse");
        assert!(matches!(
            cache.command,
            Commands::Cache {
                backend: Some(ref value),
                ..
            } if value == "memory"
        ));

        let branches =
            WitCli::try_parse_from(["wit", "branches", "-r", "owner/repo", "--backend", "memory"])
                .expect("branches --backend memory should parse");
        assert!(matches!(
            branches.command,
            Commands::Branches {
                backend: Some(ref value),
                ..
            } if value == "memory"
        ));

        let root_help = WitCli::command().render_long_help().to_string();
        assert!(root_help.contains("--backend memory"));
        assert!(root_help.contains("WIT_SNAPSHOT_BACKEND=memory"));
        assert!(root_help.contains("Memory covers tree/ls/cat/rg/sed/head/tail"));
        assert!(
            !root_help.contains("does not cover") && !root_help.to_lowercase().contains("lacks rg"),
            "help must not claim memory lacks rg/sed/head/tail"
        );
    }

    #[test]
    fn positional_repo_parses_agrees_and_rejects_conflicts() {
        // Positional-only
        let tree = WitCli::try_parse_from(["wit", "tree", "owner/repo", "src"])
            .expect("tree owner/repo src");
        match tree.command {
            Commands::Tree { repo, args, .. } => {
                let argv = ["wit", "tree", "owner/repo", "src"];
                let (resolved, path) = resolve_repo_and_optional_path(repo, args, &argv).unwrap();
                assert_eq!(resolved, "owner/repo");
                assert_eq!(path.as_deref(), Some("src"));
            }
            _ => panic!("expected tree"),
        }

        let cat = WitCli::try_parse_from(["wit", "cat", "owner/repo", "README.md"])
            .expect("cat owner/repo README.md");
        match cat.command {
            Commands::Cat { repo, args, .. } => {
                let (resolved, path) = resolve_repo_and_required_path(repo, args).unwrap();
                assert_eq!(resolved, "owner/repo");
                assert_eq!(path, "README.md");
            }
            _ => panic!("expected cat"),
        }

        let rg =
            WitCli::try_parse_from(["wit", "rg", "TODO", "owner/repo"]).expect("rg positional");
        match rg.command {
            Commands::Rg {
                pattern,
                repo,
                repo_positional,
                ..
            } => {
                assert_eq!(pattern, "TODO");
                assert_eq!(
                    resolve_repo_only(repo, repo_positional).unwrap(),
                    "owner/repo"
                );
            }
            _ => panic!("expected rg"),
        }

        let sed = WitCli::try_parse_from(["wit", "sed", "-n", "1,10p", "owner/repo", "src/lib.rs"])
            .expect("sed positional");
        match sed.command {
            Commands::Sed {
                repo,
                expressions,
                files,
                args,
                ..
            } => {
                let (resolved, scripts, path) =
                    parse_sed_invocation(repo, expressions, files, args).unwrap();
                assert_eq!(resolved, "owner/repo");
                assert_eq!(scripts, vec!["1,10p".to_string()]);
                assert_eq!(path, "src/lib.rs");
            }
            _ => panic!("expected sed"),
        }

        for (label, argv) in [
            ("head", vec!["wit", "head", "owner/repo", "Cargo.toml"]),
            ("tail", vec!["wit", "tail", "owner/repo", "Cargo.toml"]),
            ("ls", vec!["wit", "ls", "owner/repo", "src"]),
            ("branches", vec!["wit", "branches", "owner/repo"]),
            ("cache", vec!["wit", "cache", "owner/repo"]),
        ] {
            WitCli::try_parse_from(argv).unwrap_or_else(|err| panic!("{label} positional: {err}"));
        }

        // Flag-only still works
        WitCli::try_parse_from(["wit", "tree", "-r", "owner/repo", "src"]).unwrap();
        WitCli::try_parse_from(["wit", "cat", "-r", "owner/repo", "README.md"]).unwrap();
        WitCli::try_parse_from(["wit", "rg", "TODO", "-r", "owner/repo"]).unwrap();

        // Both agree
        assert_eq!(
            resolve_repo(
                Some("owner/repo".to_string()),
                Some("owner/repo".to_string())
            )
            .unwrap(),
            "owner/repo"
        );
        let tree_agree =
            WitCli::try_parse_from(["wit", "tree", "-r", "owner/repo", "owner/repo", "src"])
                .unwrap();
        match tree_agree.command {
            Commands::Tree { repo, args, .. } => {
                let argv = ["wit", "tree", "-r", "owner/repo", "owner/repo", "src"];
                let (resolved, path) = resolve_repo_and_optional_path(repo, args, &argv).unwrap();
                assert_eq!(resolved, "owner/repo");
                assert_eq!(path.as_deref(), Some("src"));
            }
            _ => panic!("expected tree"),
        }

        let branches_agree =
            WitCli::try_parse_from(["wit", "branches", "-r", "owner/repo", "owner/repo"]).unwrap();
        match branches_agree.command {
            Commands::Branches {
                repo,
                repo_positional,
                ..
            } => {
                assert_eq!(resolve_repo(repo, repo_positional).unwrap(), "owner/repo");
            }
            _ => panic!("expected branches"),
        }

        // Both disagree
        let err = resolve_repo(
            Some("owner/repo".to_string()),
            Some("other/repo".to_string()),
        )
        .expect_err("disagreeing repos must error");
        assert!(err.to_string().contains("conflicting repository arguments"));

        let tree_disagree_argv = ["wit", "tree", "-r", "owner/repo", "other/repo", "src"];
        let tree_disagree = WitCli::try_parse_from(tree_disagree_argv).unwrap();
        match tree_disagree.command {
            Commands::Tree { repo, args, .. } => {
                let err = resolve_repo_and_optional_path(repo, args, &tree_disagree_argv)
                    .expect_err("tree disagree must error");
                assert!(err.to_string().contains("conflicting repository arguments"));
            }
            _ => panic!("expected tree"),
        }

        // Unambiguous both-disagree: positional repo before -r
        let order_argv = ["wit", "tree", "other/repo", "-r", "octocat/Hello-World"];
        let order_disagree = WitCli::try_parse_from(order_argv).unwrap();
        match order_disagree.command {
            Commands::Tree { repo, args, .. } => {
                let err = resolve_repo_and_optional_path(repo, args, &order_argv)
                    .expect_err("positional-before -r disagree must error");
                assert!(err.to_string().contains("conflicting repository arguments"));
            }
            _ => panic!("expected tree"),
        }

        // Path after -r with a slash must remain a path, not a conflict
        let path_argv = ["wit", "tree", "-r", "owner/repo", "src/widgets"];
        let path_ok = WitCli::try_parse_from(path_argv).unwrap();
        match path_ok.command {
            Commands::Tree { repo, args, .. } => {
                let (resolved, path) =
                    resolve_repo_and_optional_path(repo, args, &path_argv).unwrap();
                assert_eq!(resolved, "owner/repo");
                assert_eq!(path.as_deref(), Some("src/widgets"));
            }
            _ => panic!("expected tree"),
        }

        let branches_disagree =
            WitCli::try_parse_from(["wit", "branches", "-r", "owner/repo", "other/repo"]).unwrap();
        match branches_disagree.command {
            Commands::Branches {
                repo,
                repo_positional,
                ..
            } => {
                let err = resolve_repo(repo, repo_positional).expect_err("branches disagree");
                assert!(err.to_string().contains("conflicting repository arguments"));
            }
            _ => panic!("expected branches"),
        }

        // search stays repo-free
        let search = WitCli::try_parse_from(["wit", "search", "-p", "ratatui", "--limit", "5"])
            .expect("search should not require a repo");
        assert!(matches!(search.command, Commands::Search { .. }));
    }
}
