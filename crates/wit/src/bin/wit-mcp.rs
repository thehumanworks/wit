use anyhow::Context;
use clap::{Parser, ValueEnum};
use tracing_subscriber::EnvFilter;
use wit::gitops::ops::{CacheBranchSelection, revalidate_github_repo};

#[derive(Debug, Parser)]
#[command(
    name = "wit-mcp",
    version,
    about = "Stdio wit MCP server (direct by default; Code Mode experimental)",
    after_help = "Direct mode is recommended for simple calls and exposes eight typed snapshot-first tools. Experimental Code Mode exposes one bounded native JavaScript code tool. Neither requires an external JavaScript runtime. Protocol frames are written to stdout; diagnostics are written to stderr."
)]
struct ServeArgs {
    /// MCP tool surface to expose
    #[arg(long, value_enum, default_value = "direct")]
    mode: McpMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum McpMode {
    /// Recommended default: eight snapshot-first repository tools
    Direct,
    /// Experimental: one bounded native JavaScript code tool
    Code,
}

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
        StartupMode::Worker => wit_quickjs_spike::worker::run_worker_process().await,
        StartupMode::Serve(McpMode::Direct) => {
            init_tracing();
            wit::mcp::serve_stdio().await
        }
        StartupMode::Serve(McpMode::Code) => {
            init_tracing();
            wit::codemode::serve_stdio_with_worker(std::env::current_exe()?).await
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

#[derive(Debug)]
enum StartupMode {
    Help,
    Version,
    Revalidate {
        repo: String,
        branch: CacheBranchSelection,
    },
    Worker,
    Serve(McpMode),
}

fn startup_mode() -> anyhow::Result<StartupMode> {
    match startup_mode_from(std::env::args_os()) {
        Ok(mode) => Ok(mode),
        Err(error) => match error.downcast::<clap::Error>() {
            Ok(error) => error.exit(),
            Err(error) => Err(error),
        },
    }
}

fn startup_mode_from<I, T>(args: I) -> anyhow::Result<StartupMode>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    match args.get(1).and_then(|argument| argument.to_str()) {
        Some("-h" | "--help") if args.len() == 2 => Ok(StartupMode::Help),
        Some("-V" | "--version") if args.len() == 2 => Ok(StartupMode::Version),
        Some("__cache-revalidate") => parse_revalidate(&args[2..]),
        Some("__codemode-worker") => {
            anyhow::ensure!(
                args.len() == 2,
                "__codemode-worker does not accept arguments"
            );
            Ok(StartupMode::Worker)
        }
        Some(argument)
            if argument != "--mode"
                && !argument.starts_with("--mode=")
                && !matches!(argument, "-h" | "--help" | "-V" | "--version") =>
        {
            anyhow::bail!("unsupported argument {argument}; run `wit-mcp --help` for usage")
        }
        _ => Ok(StartupMode::Serve(ServeArgs::try_parse_from(args)?.mode)),
    }
}

fn print_help() {
    println!(
        "\
wit-mcp {}

Stdio wit MCP server (direct by default; Code Mode experimental).

USAGE:
    wit-mcp [--mode <direct|code>]
    wit-mcp --version

OPTIONS:
        --mode <direct|code>  MCP tool surface to expose [default: direct]
    -h, --help                Print help
    -V, --version             Print version

Direct mode is recommended for simple calls and exposes eight typed snapshot-first tools. Experimental Code Mode exposes one bounded native JavaScript code tool. Neither requires an external JavaScript runtime. Protocol frames are written to stdout; diagnostics are written to stderr.",
        env!("CARGO_PKG_VERSION")
    );
}

fn parse_revalidate(args: &[std::ffi::OsString]) -> anyhow::Result<StartupMode> {
    let mut args = args.iter();
    let mut repo = None;
    let mut branch = None;
    while let Some(flag) = args.next().and_then(|argument| argument.to_str()) {
        match flag {
            "--repo" => {
                repo = Some(
                    args.next()
                        .and_then(|argument| argument.to_str())
                        .context("__cache-revalidate --repo requires a UTF-8 value")?
                        .to_owned(),
                );
            }
            "--branch" => {
                branch = Some(
                    args.next()
                        .and_then(|argument| argument.to_str())
                        .context("__cache-revalidate --branch requires a UTF-8 value")?
                        .to_owned(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, error::ErrorKind};

    #[test]
    fn public_modes_parse_and_direct_is_default() {
        assert!(matches!(
            startup_mode_from(["wit-mcp"]),
            Ok(StartupMode::Serve(McpMode::Direct))
        ));
        assert!(matches!(
            startup_mode_from(["wit-mcp", "--mode", "direct"]),
            Ok(StartupMode::Serve(McpMode::Direct))
        ));
        assert!(matches!(
            startup_mode_from(["wit-mcp", "--mode", "code"]),
            Ok(StartupMode::Serve(McpMode::Code))
        ));
    }

    #[test]
    fn unknown_modes_and_arguments_are_rejected() {
        let mode_error = startup_mode_from(["wit-mcp", "--mode", "unknown"])
            .expect_err("unknown mode must fail");
        let clap_error = mode_error
            .downcast_ref::<clap::Error>()
            .expect("invalid mode should be a clap error");
        assert_eq!(clap_error.kind(), ErrorKind::InvalidValue);
        assert!(
            clap_error
                .to_string()
                .contains("possible values: direct, code")
        );

        let argument_error =
            startup_mode_from(["wit-mcp", "--unknown"]).expect_err("unknown argument must fail");
        assert!(
            argument_error
                .to_string()
                .contains("unsupported argument --unknown")
        );
    }

    #[test]
    fn internal_modes_remain_hidden_and_strict() {
        assert!(matches!(
            startup_mode_from(["wit-mcp", "__codemode-worker"]),
            Ok(StartupMode::Worker)
        ));
        assert!(
            startup_mode_from(["wit-mcp", "__codemode-worker", "extra"])
                .expect_err("worker mode must reject trailing arguments")
                .to_string()
                .contains("does not accept arguments")
        );
        assert!(matches!(
            startup_mode_from(["wit-mcp", "__cache-revalidate", "--repo", "owner/repo"]),
            Ok(StartupMode::Revalidate { .. })
        ));

        let help = ServeArgs::command().render_long_help().to_string();
        assert!(!help.contains("__codemode-worker"));
        assert!(!help.contains("__cache-revalidate"));
    }

    #[test]
    fn help_agrees_on_default_and_experimental_status() {
        let help = ServeArgs::command().render_long_help().to_string();
        assert!(help.contains("direct by default; Code Mode experimental"));
        assert!(help.contains("[default: direct]"));
        assert!(help.contains("recommended for simple calls"));
        assert!(help.contains("Experimental Code Mode"));
        assert!(help.contains("one bounded native JavaScript code tool"));
        assert!(help.contains("external JavaScript"));
        assert!(help.contains("runtime"));
    }
}
