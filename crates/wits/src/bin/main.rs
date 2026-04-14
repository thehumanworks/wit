use clap::Parser;
use colored::Colorize;
use wits::RepoListMetric;
use wits::client::GrepClient;

#[derive(Parser)]
#[command(
    name = "wit-search",
    about = "Search GitHub repositories via grep.app",
    after_help = "Examples:\n  wit-search -p 'ratatui' -l 'Rust'                  # Find Rust repos named 'ratatui'\n  wit-search -p 'auth' -q 'JWT' -l 'Go' -w           # Find Go auth repos using JWT, show code\n  wit-search -p 'ratatui' -q 'impl Widget' -w -c      # Matching lines only, no context"
)]
struct SearchCli {
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = SearchCli::parse();

    let client = GrepClient::new();
    let repos = client
        .repo_search(
            &cli.pattern,
            cli.lang.as_deref(),
            cli.regex,
            &cli.query,
            cli.with_snippets,
        )
        .await?;

    if !cli.with_snippets && repos.is_empty() {
        println!("{}", "No repositories found.".yellow());
        return Ok(());
    }

    wits::print_search_results(
        &repos,
        cli.with_snippets,
        cli.compact,
        RepoListMetric::CodeHits,
    );
    Ok(())
}
