use anyhow::Context;
use tracing_subscriber::EnvFilter;
use wit::gitops::ops::revalidate_github_repo;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match startup_mode()? {
        StartupMode::Help => {
            print_help();
            Ok(())
        }
        StartupMode::Version => {
            println!("wit-mcp {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        StartupMode::Revalidate(repo) => {
            init_tracing();
            revalidate_github_repo(&repo)?;
            Ok(())
        }
        StartupMode::Serve => {
            init_tracing();
            wit::mcp::serve_stdio().await
        }
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}

enum StartupMode {
    Help,
    Version,
    Revalidate(String),
    Serve,
}

fn startup_mode() -> anyhow::Result<StartupMode> {
    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        return Ok(StartupMode::Serve);
    };
    if first == "-h" || first == "--help" {
        return Ok(StartupMode::Help);
    }
    if first == "-V" || first == "--version" {
        return Ok(StartupMode::Version);
    }
    if first != "__cache-revalidate" {
        return Ok(StartupMode::Serve);
    }

    let flag = args.next().context("__cache-revalidate requires --repo")?;
    if flag != "--repo" {
        anyhow::bail!("__cache-revalidate expects --repo, got {flag}");
    }
    let repo = args
        .next()
        .context("__cache-revalidate --repo requires a value")?;
    if args.next().is_some() {
        anyhow::bail!("__cache-revalidate received unexpected extra arguments");
    }
    Ok(StartupMode::Revalidate(repo))
}

fn print_help() {
    println!(
        "\
wit-mcp {}

Stdio MCP server for exploring GitHub repositories with wit.

USAGE:
    wit-mcp
    wit-mcp --version

MCP clients should launch wit-mcp with no arguments, or use `wit mcp --transport stdio`. Protocol frames are written to stdout; diagnostics are written to stderr.",
        env!("CARGO_PKG_VERSION")
    );
}
