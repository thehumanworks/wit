use anyhow::Context;
use tracing_subscriber::EnvFilter;
use wit::gitops::ops::{CacheBranchSelection, revalidate_github_repo};

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
        StartupMode::Revalidate { repo, branch } => {
            init_tracing();
            revalidate_github_repo(&repo, branch)?;
            Ok(())
        }
        StartupMode::Serve { compat_v1 } => {
            init_tracing();
            if compat_v1 {
                wit::mcp_compat::serve_stdio_compat().await
            } else {
                wit::mcp::serve_stdio().await
            }
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
    Revalidate {
        repo: String,
        branch: CacheBranchSelection,
    },
    Serve {
        compat_v1: bool,
    },
}

fn startup_mode() -> anyhow::Result<StartupMode> {
    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        return Ok(StartupMode::Serve { compat_v1: false });
    };
    if first == "-h" || first == "--help" {
        return Ok(StartupMode::Help);
    }
    if first == "-V" || first == "--version" {
        return Ok(StartupMode::Version);
    }
    if first == "--compat-v1" {
        if let Some(unexpected) = args.next() {
            anyhow::bail!("--compat-v1 received unexpected argument {unexpected}");
        }
        return Ok(StartupMode::Serve { compat_v1: true });
    }
    if first != "__cache-revalidate" {
        return Ok(StartupMode::Serve { compat_v1: false });
    }

    let mut repo = None;
    let mut branch = None;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--repo" => {
                repo = Some(
                    args.next()
                        .context("__cache-revalidate --repo requires a value")?,
                );
            }
            "--branch" => {
                branch = Some(
                    args.next()
                        .context("__cache-revalidate --branch requires a value")?,
                );
            }
            _ => anyhow::bail!("__cache-revalidate received unexpected argument {flag}"),
        }
    }
    let repo = repo.context("__cache-revalidate requires --repo")?;
    Ok(StartupMode::Revalidate {
        repo,
        branch: branch.map_or(CacheBranchSelection::Default, CacheBranchSelection::named),
    })
}

fn print_help() {
    println!(
        "\
wit-mcp {}

Stdio MCP server for exploring GitHub repositories with wit.

USAGE:
    wit-mcp
    wit-mcp --compat-v1
    wit-mcp --version

MCP clients should launch wit-mcp with no arguments for the agent-native v2 surface. Use --compat-v1 only for the deprecated Unix-shaped v1 tools. Protocol frames are written to stdout; diagnostics are written to stderr.",
        env!("CARGO_PKG_VERSION")
    );
}
