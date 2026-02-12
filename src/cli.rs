use clap::{Parser, Subcommand};
use colored::Colorize;
use std::fs;
use wit::{
    gitops::ops::{
        GrepOptions, GrepResult, build_tree, cache_github_repo, grep_repo_with_options, head,
        list_dir, read_file, tail,
    },
    grep, sed,
};

#[derive(Parser)]
#[command(name = "wit")]
#[command(
    about = "Explore GitHub repositories without cloning. Repos are cached as shallow bare clones in your system temp directory (override with WIT_CACHE_DIR).",
    long_about = None
)]
struct WitCli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(
        name = "search",
        visible_alias = "s",
        about = "Find GitHub repositories by name and search their code via grep.app",
        override_usage = "wit <search|s> [--lang <LANG>] --pattern <PATTERN>",
        after_help = "Use this to discover repositories. Combine -p (repo name pattern) with -q (code pattern) to find repos containing specific implementations. Add -w for code snippets, -c to strip context.\n\nExamples:\n  wit search -p 'ratatui' -l 'Rust'                  # Find Rust repos named 'ratatui'\n  wit search -p 'auth' -q 'JWT' -l 'Go' -w           # Find Go auth repos using JWT, show code\n  wit search -p 'ratatui' -q 'impl Widget' -w -c      # Matching lines only, no context"
    )]
    Search {
        /// Regex pattern to match repository names
        #[arg(short, long)]
        pattern: String,

        /// Optional language pattern to filter results
        #[arg(short, long)]
        lang: Option<String>,

        /// Flag to enable regex based-search - defaults to true
        #[arg(short, long, default_value_t = true)]
        regex: bool,

        /// Optional query to search for in repositories - defaults to ".*"
        #[arg(short, long, default_value = ".*")]
        query: String,

        /// Flag to enable snippets - defaults to false
        #[arg(short, long, default_value_t = false)]
        with_snippets: bool,

        /// Show only matching lines without context (requires --with-snippets)
        #[arg(short, long, default_value_t = false)]
        compact: bool,
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

        /// Maximum number of matches to show (0 = unlimited)
        #[arg(short = 'm', long, default_value_t = 0)]
        max_count: usize,

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
        after_help = "Use for precise line-range extraction or text transformation. Regex uses Rust syntax, not POSIX BRE. Supports addresses, substitution, hold space, branching, and most POSIX commands.\n\nExamples:\n  wit sed -n '320,460p' modal-labs/modal-client modal/image.py    # Print line range\n  wit sed -n '/TODO/p' ratatui/ratatui src/lib.rs                 # Lines matching pattern\n  wit sed 's/Widget/Component/g' ratatui/ratatui src/lib.rs       # Substitute text\n  wit sed -n '/^pub fn/p' ratatui/ratatui src/lib.rs              # Extract function signatures",
        trailing_var_arg = true
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = WitCli::parse();

    match cli.command {
        Commands::Search {
            pattern,
            lang,
            regex,
            query,
            with_snippets,
            compact,
        } => {
            search(
                &pattern,
                lang.as_deref(),
                regex,
                &query,
                with_snippets,
                compact,
            )
            .await?;
        }
        Commands::Cache { repo } => {
            let repo = cache_github_repo(&repo, true).await?;
            println!("Cached repository: {}", repo.path().display());
        }
        Commands::Tree { repo, path, long } => {
            let repository = cache_github_repo(&repo, false).await?;
            build_tree(&repository, path.as_deref(), long)?;
        }
        Commands::Ls { repo, path, long } => {
            let repository = cache_github_repo(&repo, false).await?;
            let entries = list_dir(&repository, path.as_deref(), long)?;

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
            let content = read_file(&repository, &path)?;

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
            let opts = GrepOptions::new()
                .ignore_case(ignore_case)
                .smart_case(smart_case)
                .word_regexp(word_regexp)
                .invert_match(invert_match)
                .max_count(max_count)
                .before_context(if context > 0 { context } else { before_context })
                .after_context(if context > 0 { context } else { after_context })
                .glob(glob)
                .files_with_matches(files_with_matches)
                .count(count);

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
            let output = head(&repository, &path, lines, number)?;
            println!("{}", output);
        }
        Commands::Sed {
            quiet,
            expressions,
            files,
            args,
        } => {
            let (scripts, repo, path) = parse_sed_invocation(expressions, files, args)?;
            let repository = cache_github_repo(&repo, false).await?;
            let content = read_file(&repository, &path)?;
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
            let output = tail(&repository, &path, lines, from_line, number)?;
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

async fn search(
    pattern: &str,
    lang: Option<&str>,
    regex: bool,
    query: &str,
    with_snippets: bool,
    compact: bool,
) -> anyhow::Result<()> {
    let client = grep::client::GrepClient::new();
    let repos = client
        .repo_search(pattern, lang, regex, query, with_snippets)
        .await?;

    if repos.is_empty() {
        println!("{}", "No repositories found.".yellow());
        return Ok(());
    }

    println!(
        "\n{} {} {}\n",
        "Found".green().bold(),
        repos.len().to_string().cyan().bold(),
        "repositories:".green().bold()
    );

    // Find max name length for alignment
    let max_name_len = repos.iter().map(|r| r.name.len()).max().unwrap_or(0);

    for (i, repo) in repos.iter().enumerate() {
        let rank = format!("{:>3}.", i + 1).dimmed();
        let name = format!("{:<width$}", repo.name, width = max_name_len)
            .white()
            .bold();
        let hits = format!("{:>6} hits", repo.hits).cyan();
        println!("  {} {} {}", rank, name, hits);

        if with_snippets && !repo.files.is_empty() {
            for file in &repo.files {
                // File path header
                println!();
                println!("      {} {}", "-->".dimmed(), file.path.blue());

                // Calculate max line number width for this file
                let max_line_num = file
                    .lines
                    .iter()
                    .filter(|l| !l.is_jump)
                    .map(|l| l.line_number)
                    .max()
                    .unwrap_or(0);
                let line_num_width = max_line_num.to_string().len().max(3);

                // Print code lines
                for line in &file.lines {
                    // In compact mode, skip non-matching lines and jump indicators
                    if compact && (line.is_jump || !line.has_match) {
                        continue;
                    }

                    if line.is_jump {
                        // Non-contiguous section separator
                        let dots = "...".dimmed();
                        println!("      {:>width$} {}", "", dots, width = line_num_width);
                    } else {
                        let line_num =
                            format!("{:>width$}", line.line_number, width = line_num_width);
                        let separator = "|".dimmed();

                        if line.has_match {
                            // Highlight the entire line for matches
                            println!(
                                "      {} {} {}",
                                line_num.yellow(),
                                separator,
                                line.content.yellow()
                            );
                        } else {
                            println!("      {} {} {}", line_num.dimmed(), separator, line.content);
                        }
                    }
                }

                // Show total matches for this file if more than shown
                if file.total_matches > file.lines.iter().filter(|l| l.has_match).count() as u32 {
                    println!(
                        "       {}",
                        format!("({} total matches in file)", file.total_matches).dimmed()
                    );
                }
            }
            println!();
        }
    }

    println!();
    Ok(())
}
