pub mod client;
pub mod types;

use colored::Colorize;
use types::{CodeLine, RepoMatch};

/// Print search results to stdout with colored formatting.
///
/// When `with_snippets` is true, code snippets are shown beneath each repo.
/// When `compact` is true, only matching lines are printed (no context/jumps).
pub fn print_search_results(repos: &[RepoMatch], with_snippets: bool, compact: bool) {
    if repos.is_empty() {
        println!("{}", "No repositories found.".yellow());
        return;
    }

    println!(
        "\n{} {} {}\n",
        "Found".green().bold(),
        repos.len().to_string().cyan().bold(),
        "repositories:".green().bold()
    );

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
                println!();
                println!("      {} {}", "-->".dimmed(), file.path.blue());

                let max_line_num = file
                    .lines
                    .iter()
                    .filter(|l| !l.is_jump)
                    .map(|l| l.line_number)
                    .max()
                    .unwrap_or(0);
                let line_num_width = max_line_num.to_string().len().max(3);

                for line in &file.lines {
                    if compact && (line.is_jump || !line.has_match) {
                        continue;
                    }

                    print_code_line(line, line_num_width);
                }

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
}

fn print_code_line(line: &CodeLine, line_num_width: usize) {
    if line.is_jump {
        let dots = "...".dimmed();
        println!("      {:>width$} {}", "", dots, width = line_num_width);
    } else {
        let line_num = format!("{:>width$}", line.line_number, width = line_num_width);
        let separator = "|".dimmed();

        if line.has_match {
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
