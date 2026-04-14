use clap::{ArgAction, Parser, Subcommand};
use colored::Colorize;
use std::fs;
use wit::{
    ensure_rustls_provider,
    gitops::ops::{
        GrepOptions, GrepResult, build_tree_with_ignore, cache_github_repo, grep_repo_with_options,
        head_with_ignore, list_dir_with_ignore, read_file, read_file_with_ignore, tail_with_ignore,
    },
    search::{DEFAULT_GITHUB_REPO_LIMIT, MAX_GITHUB_REPOS},
    search_run, sed,
};

#[derive(Parser)]
#[command(name = "wit")]
#[command(
    about = "Explore GitHub repositories without cloning. Repos are cached as shallow bare clones in your system temp directory (override with WIT_CACHE_DIR).",
    long_about = None
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
        after_help = "Use -p/--pattern to restrict repository names and -q/--query to pass raw GitHub search terms and qualifiers through to the REST API. Common qualifiers include language:, user:, org:, stars:, forks:, size:, created:, pushed:, topic:, archived:, mirror:, template:, license:, help-wanted-issues:, and good-first-issues:.\n\nwit fetches only enough GitHub pages to satisfy --limit (default: 30, max: 1000). Results are ordered by stars. GitHub repository search does not support regex name matching; --pattern is treated as a literal name filter.\n\nSet GITHUB_TOKEN for higher GitHub rate limits.\n\nExamples:\n  wit search -p 'ratatui' -l 'Rust' --limit 20\n  wit search -q 'stars:>1000 topic:tui archived:false' --limit 25\n  wit search -p 'auth' -q 'user:ory language:Go pushed:>2025-01-01'"
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
        name = "cache",
        visible_alias = "c",
        about = "Clone a repository into the local cache (or refresh an existing one)",
        after_help = "Repos are auto-cached on first use by other commands. Use this to force-refresh a stale cache.\n\nExamples:\n  wit cache ratatui/ratatui          # Force re-clone of ratatui"
    )]
    Cache {
        /// Repository in "owner/repo" format
        repo: String,
    },
    #[command(
        name = "tree",
        visible_alias = "t",
        about = "Show the file tree of a repository (or subtree). Use -l for line counts",
        override_usage = "wit <tree|t> [OPTIONS] <REPO> [PATH]",
        after_help = "Start here to understand a repo's structure. Narrow with a path to avoid noise on large repos. Use -l to see file sizes and decide whether to cat or head.\n\nExamples:\n  wit tree ratatui/ratatui                # Full repo tree\n  wit tree ratatui/ratatui src/widgets    # Only the widgets subtree\n  wit tree -l ratatui/ratatui src         # With line counts and token estimates"
    )]
    Tree {
        /// Repository in "owner/repo" format
        repo: String,

        /// Optional subdirectory path to display tree from
        path: Option<String>,

        /// Show file sizes: lines and approximate token count
        #[arg(short = 'l', long = "long")]
        long: bool,
    },
    #[command(
        name = "ls",
        about = "List directory contents (non-recursive). Use -l for file sizes",
        override_usage = "wit ls [OPTIONS] <REPO> [PATH]",
        after_help = "Use to browse one directory level at a time. Unlike tree (recursive), ls shows only immediate children. Use -l to see line counts and token estimates before deciding what to read.\n\nExamples:\n  wit ls ratatui/ratatui                    # List repo root\n  wit ls ratatui/ratatui src/widgets        # List a subdirectory\n  wit ls -l ratatui/ratatui src             # With file sizes"
    )]
    Ls {
        /// Repository in "owner/repo" format
        repo: String,

        /// Directory path within the repository (default: root)
        path: Option<String>,

        /// Show file sizes: lines and approximate token count
        #[arg(short = 'l', long = "long")]
        long: bool,
    },
    #[command(
        name = "cat",
        about = "Print a file's contents. Use -n for line numbers",
        override_usage = "wit cat [OPTIONS] <REPO> <PATH>",
        after_help = "Use for small-to-medium files. For large files, prefer head/tail/sed to read specific ranges, or rg to search for patterns.\n\nExamples:\n  wit cat ratatui/ratatui Cargo.toml             # Print file\n  wit cat -n ratatui/ratatui src/lib.rs           # With line numbers\n  wit cat -b ratatui/ratatui README.md            # Number non-blank lines only"
    )]
    Cat {
        /// Repository in "owner/repo" format
        repo: String,

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
    },
    #[command(
        name = "rg",
        about = "Search file contents (ripgrep-style). Use -l to find files, -g to filter by type",
        override_usage = "wit rg [OPTIONS] <PATTERN> <REPO>",
        after_help = "The primary tool for locating code. Use -l to discover which files contain a pattern (cheaper than full matches). Use -g to restrict to file types. Combine -C for context around matches.\n\nExamples:\n  wit rg 'impl Widget' ratatui/ratatui              # Find implementations\n  wit rg -l 'struct.*Frame' ratatui/ratatui          # List files containing pattern\n  wit rg -g '*.rs' -i 'todo' ratatui/ratatui         # Case-insensitive in .rs files\n  wit rg -C 3 'fn render' ratatui/ratatui             # 3 lines of context"
    )]
    Rg {
        /// Regex pattern to search for
        pattern: String,

        /// Repository in "owner/repo" format
        repo: String,

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
        override_usage = "wit sed [OPTIONS] <SCRIPT> <REPO> <PATH>",
        after_help = "Use for precise line-range extraction or text transformation. Regex uses Rust syntax, not POSIX BRE. Supports addresses, substitution, hold space, branching, and most POSIX commands.\n\nExamples:\n  wit sed -n '320,460p' modal-labs/modal-client modal/image.py    # Print line range\n  wit sed -n '/TODO/p' ratatui/ratatui src/lib.rs                 # Lines matching pattern\n  wit sed 's/Widget/Component/g' ratatui/ratatui src/lib.rs       # Substitute text\n  wit sed -n '/^pub fn/p' ratatui/ratatui src/lib.rs              # Extract function signatures"
    )]
    Sed {
        /// Suppress automatic printing of pattern space
        #[arg(short = 'n', long = "quiet", alias = "silent")]
        quiet: bool,

        /// Add script to the commands to be executed
        #[arg(short = 'e', long = "expression")]
        expressions: Vec<String>,

        /// Add script file to the commands to be executed
        #[arg(short = 'f', long = "file", value_name = "FILE")]
        files: Vec<String>,

        /// Script, repo, path (positional). With -e/-f, SCRIPT is optional.
        #[arg(allow_hyphen_values = true)]
        args: Vec<String>,
    },
    #[command(
        name = "head",
        about = "Print the first N lines of a file (default: 10)",
        override_usage = "wit head [OPTIONS] <REPO> <PATH>",
        after_help = "Use to preview a file before deciding whether to read it fully. Pair with tail to read specific sections by position.\n\nExamples:\n  wit head ratatui/ratatui src/lib.rs            # First 10 lines\n  wit head -n 50 ratatui/ratatui Cargo.toml      # First 50 lines\n  wit head -N ratatui/ratatui README.md           # With line numbers"
    )]
    Head {
        /// Repository in "owner/repo" format
        repo: String,

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
        override_usage = "wit tail [OPTIONS] <REPO> <PATH>",
        after_help = "Use -p to read from a specific line to end-of-file -- useful when you know a line number from rg output and want the surrounding code.\n\nExamples:\n  wit tail ratatui/ratatui src/lib.rs              # Last 10 lines\n  wit tail -n 20 ratatui/ratatui Cargo.toml        # Last 20 lines\n  wit tail -p 100 ratatui/ratatui src/lib.rs       # From line 100 to end"
    )]
    Tail {
        /// Repository in "owner/repo" format
        repo: String,

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
        Commands::Cache { repo } => {
            let repo = cache_github_repo(&repo, true).await?;
            println!("Cached repository: {}", repo.path().display());
        }
        Commands::Tree { repo, path, long } => {
            let repository = cache_github_repo(&repo, false).await?;
            build_tree_with_ignore(&repository, path.as_deref(), long, &ignore_patterns)?;
        }
        Commands::Ls { repo, path, long } => {
            let repository = cache_github_repo(&repo, false).await?;
            let entries =
                list_dir_with_ignore(&repository, path.as_deref(), long, &ignore_patterns)?;

            if entries.is_empty() {
                println!("{}", "Directory is empty or does not exist.".yellow());
                return Ok(());
            }

            if long {
                // Find max line count width for alignment
                let max_lines = entries.iter().filter_map(|e| e.lines).max().unwrap_or(0);
                let lines_width = max_lines.to_string().len().max(1);

                for entry in &entries {
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
                for entry in &entries {
                    if entry.is_dir {
                        println!("{}/", entry.name);
                    } else {
                        println!("{}", entry.name);
                    }
                }
            }
        }
        Commands::Cat {
            repo,
            path,
            number,
            number_nonblank,
            squeeze_blank,
            show_ends,
            show_tabs,
            show_all,
        } => {
            let repository = cache_github_repo(&repo, false).await?;
            let content = read_file_with_ignore(&repository, &path, &ignore_patterns)?;

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
            let repository = cache_github_repo(&repo, false).await?;

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
            path,
            lines,
            number,
        } => {
            let repository = cache_github_repo(&repo, false).await?;
            let output = head_with_ignore(&repository, &path, lines, number, &ignore_patterns)?;
            println!("{}", output);
        }
        Commands::Sed {
            quiet,
            expressions,
            files,
            args,
        } => {
            let (args, inline_ignores) = extract_sed_inline_ignores(args)?;
            let mut effective_ignore_patterns = ignore_patterns.clone();
            effective_ignore_patterns.extend(inline_ignores);

            let (scripts, repo, path) = parse_sed_invocation(expressions, files, args)?;
            let repository = cache_github_repo(&repo, false).await?;
            let content = read_file_with_ignore(&repository, &path, &effective_ignore_patterns)?;
            let program = sed::parse_script(&scripts)?;
            let output = sed::run(&program, &content, &sed::SedOptions { quiet })?;
            print!("{}", output.output);
            if output.exit_code != 0 {
                std::process::exit(output.exit_code);
            }
        }
        Commands::Tail {
            repo,
            path,
            lines,
            from_line,
            number,
        } => {
            let repository = cache_github_repo(&repo, false).await?;
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
    }

    Ok(())
}

fn parse_sed_invocation(
    expressions: Vec<String>,
    files: Vec<String>,
    args: Vec<String>,
) -> anyhow::Result<(Vec<String>, String, String)> {
    let mut scripts = Vec::new();
    scripts.extend(expressions);

    for file in files {
        let content = fs::read_to_string(&file)
            .map_err(|e| anyhow::anyhow!("failed to read sed script file '{}': {}", file, e))?;
        scripts.push(content);
    }

    let (script_arg, repo, path) = match args.len() {
        3 => (Some(args[0].clone()), args[1].clone(), args[2].clone()),
        2 => (None, args[0].clone(), args[1].clone()),
        _ => {
            return Err(anyhow::anyhow!(
                "sed expects <SCRIPT> <REPO> <PATH> or <REPO> <PATH> with -e/-f"
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

    Ok((scripts, repo, path))
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

    #[test]
    fn test_global_ignore_parses_for_rg() {
        let cli = WitCli::try_parse_from([
            "wit",
            "rg",
            "needle",
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
            "1,3p",
            "owner/repo",
            "src/lib.rs",
            "--ignore",
            "vendor",
        ])
        .expect("sed args should parse");

        assert!(cli.ignore.is_empty());

        match cli.command {
            Commands::Sed {
                quiet,
                expressions,
                files,
                args,
            } => {
                let (filtered_args, inline_ignores) =
                    extract_sed_inline_ignores(args).expect("inline sed ignores should parse");

                assert!(quiet);
                assert!(expressions.is_empty());
                assert!(files.is_empty());
                assert_eq!(inline_ignores, vec!["vendor".to_string()]);
                assert_eq!(filtered_args, vec!["1,3p", "owner/repo", "src/lib.rs"]);
            }
            _ => panic!("expected sed command"),
        }
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
}
