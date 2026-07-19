use rmcp::{
    ServiceExt,
    model::{ReadResourceRequestParams, ResourceContents},
    transport::TokioChildProcess,
};
use std::process::Command;

#[tokio::test]
async fn code_mode_documentation_surfaces_hold_their_individual_contracts() -> anyhow::Result<()> {
    let readme = include_str!("../../../README.md");
    assert_surface(
        "README.md",
        readme,
        &[
            "Direct MCP is the default and the recommended mode for a simple operation",
            "normal MCP tool named `code`",
            "wit mcp --transport stdio --mode direct|code",
            "wit-mcp --mode direct|code",
            "codemode.wit.open",
            "next_cursor",
            "provenance-bearing evidence",
            "no filesystem",
            "remain in the Rust parent",
            "not persisted or logged",
            "external model evaluation has not run",
            "fail-closed recommendation remains direct mode",
        ],
    );

    let skill = include_str!("../src/skill/SKILL.md");
    assert_surface(
        "bundled skill",
        skill,
        &[
            "default and current recommendation",
            "one normal MCP `code` tool",
            "codemode.wit.open",
            "snapshot_id",
            "next_cursor",
            "line ranges for evidence",
            "no filesystem",
            "remain in the Rust parent",
            "not persisted or logged",
            "external model evaluation is unrun",
            "fail-closed recommendation",
        ],
    );

    let mcp = env!("CARGO_BIN_EXE_wit-mcp");
    let client = ().serve(TokioChildProcess::new(tokio::process::Command::new(mcp))?).await?;
    let workflow = read_text_resource(&client, "wit://guide/workflow").await?;
    assert_surface(
        "wit://guide/workflow",
        &workflow,
        &[
            "Direct mode is the default and current recommendation",
            "one normal MCP `code` tool",
            "codemode.wit.open",
            "next_cursor",
            "line provenance",
            "filesystem, network, environment, process",
            "remain in the Rust parent",
            "not persisted or logged",
            "external model evaluation is unrun",
            "fail-closed recommendation",
        ],
    );

    let tools = read_text_resource(&client, "wit://guide/tools").await?;
    assert_surface(
        "wit://guide/tools",
        &tools,
        &[
            "default and recommendation for simple calls",
            "one normal MCP tool named `code`",
            "codemode.wit.open",
            "next_cursor",
            "budgets, snapshots, and provenance",
            "There is no filesystem, network, environment, process",
            "not persisted or logged",
            "model evaluation is unrun",
            "fail-closed recommendation",
        ],
    );
    client.cancel().await?;

    let wit_help = command_stdout(
        env!("CARGO_BIN_EXE_wit"),
        &["mcp", "--help"],
        "wit mcp --help",
    )?;
    assert_surface(
        "wit mcp --help",
        &wit_help,
        &[
            "[default: direct]",
            "recommended for simple calls",
            "Experimental Code Mode",
            "one native JavaScript code tool",
            "external JavaScript runtime",
        ],
    );

    let standalone_help = command_stdout(mcp, &["--help"], "wit-mcp --help")?;
    assert_surface(
        "wit-mcp --help",
        &standalone_help,
        &[
            "[default: direct]",
            "recommended for simple calls",
            "Experimental Code Mode",
            "one bounded native JavaScript code tool",
            "external JavaScript runtime",
        ],
    );

    Ok(())
}

fn assert_surface(name: &str, text: &str, required: &[&str]) {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for claim in required {
        assert!(normalized.contains(claim), "{name} omits `{claim}`");
    }
    let lower = normalized.to_ascii_lowercase();
    assert!(!lower.contains("compat-v1"), "{name} mentions compat-v1");
    assert!(!lower.contains("mcp v1"), "{name} mentions MCP v1");
}

async fn read_text_resource(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    uri: &str,
) -> anyhow::Result<String> {
    let result = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await?;
    let Some(ResourceContents::TextResourceContents { text, .. }) = result.contents.first() else {
        anyhow::bail!("{uri} did not return one text resource");
    };
    anyhow::ensure!(result.contents.len() == 1, "{uri} returned extra contents");
    Ok(text.clone())
}

fn command_stdout(binary: &str, args: &[&str], name: &str) -> anyhow::Result<String> {
    let output = Command::new(binary).args(args).output()?;
    anyhow::ensure!(
        output.status.success(),
        "{name} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}
