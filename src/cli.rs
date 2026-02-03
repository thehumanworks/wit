use clap::{Parser, Subcommand};
use colored::Colorize;
use wit::{
    gitops::ops::{
        GrepOptions, GrepResult, build_tree, cache_github_repo, grep_repo_with_options, head,
        read_file, tail,
    },
    grep,
};

#[derive(Parser)]
#[command(name = "wit")]
#[command(about = "Github for AI Agents", long_about = None)]
struct WitCli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(
        name = "search",
        visible_alias = "s",
        about = "Find repositories matching a pattern",
        override_usage = "wit <search|s> [--lang <LANG>] --pattern <PATTERN>",
        after_help = "Examples:\n  wit search -p 'deepagents' -l 'pyth'\n  wit search -p 'ratatui' -l 'Rust' -q 'Table'\n  wit search -p 'ratatui' -l 'Rust' -q 'Table' -w\n  wit search -p 'ratatui' -l 'Rust' -q 'Table' -w -c"
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
        about = "Cache a new repository locally or refresh the cache of an existing one.",
        after_help = "Examples:\n  wit cache ratatui/ratatui"
    )]
    Cache {
        /// Repository in "owner/repo" format
        repo: String,
    },
    #[command(
        name = "tree",
        visible_alias = "t",
        about = "Display repository file tree",
        override_usage = "wit <tree|t> <REPO> [PATH]",
        after_help = "Examples:\n  wit tree ratatui/ratatui\n  wit tree ratatui/ratatui src"
    )]
    Tree {
        /// Repository in "owner/repo" format
        repo: String,

        /// Optional subdirectory path to display tree from
        path: Option<String>,
    },
    #[command(
        name = "cat",
        about = "Display contents of a file from a repository (POSIX-style)",
        override_usage = "wit cat [OPTIONS] <REPO> <PATH>",
        after_help = "Examples:\n  wit cat ratatui/ratatui src/lib.rs\n  wit cat -n ratatui/ratatui Cargo.toml\n  wit cat -b ratatui/ratatui README.md"
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
        about = "Search for a pattern in repository files (ripgrep-style)",
        override_usage = "wit rg [OPTIONS] <PATTERN> <REPO>",
        after_help = "Examples:\n  wit rg 'impl Widget' ratatui/ratatui\n  wit rg -i -g '*.rs' 'widget' ratatui/ratatui\n  wit rg -l 'struct.*Frame' ratatui/ratatui\n  wit rg -C 3 'fn render' ratatui/ratatui"
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
    },
    #[command(
        name = "head",
        about = "Output the first part of a file",
        override_usage = "wit head [OPTIONS] <REPO> <PATH>",
        after_help = "Examples:\n  wit head ratatui/ratatui src/lib.rs\n  wit head -n 20 ratatui/ratatui Cargo.toml\n  wit head -N ratatui/ratatui README.md"
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
        about = "Output the last part of a file",
        override_usage = "wit tail [OPTIONS] <REPO> <PATH>",
        after_help = "Examples:\n  wit tail ratatui/ratatui src/lib.rs\n  wit tail -n 20 ratatui/ratatui Cargo.toml\n  wit tail -p 100 ratatui/ratatui file.rs  # From line 100 to end"
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
        Commands::Tree { repo, path } => {
            let repository = cache_github_repo(&repo, false).await?;
            build_tree(&repository, path.as_deref())?;
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
                    for file in files {
                        println!("{}", file.magenta());
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
