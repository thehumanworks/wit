use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ReadResourceRequestParams},
    object,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[tokio::test]
async fn wit_mcp_stdio_lists_guidance_and_static_tools() -> anyhow::Result<()> {
    let bin = env!("CARGO_BIN_EXE_wit-mcp");
    let cache = tempfile::tempdir()?;
    let fixture = seed_cached_repo(cache.path())?;
    let transport = TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
        cmd.env("WIT_CACHE_DIR", cache.path())
            .env("GIT_CONFIG_GLOBAL", &fixture.git_config)
            .env("GIT_CONFIG_NOSYSTEM", "1");
    }))?;

    let client = ().serve(transport).await?;

    let tools = client.list_all_tools().await?;
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert!(names.contains(&"wit_search"));
    assert!(names.contains(&"wit_tree"));
    assert!(names.contains(&"wit_cat"));
    assert!(names.contains(&"wit_skill_load"));

    let resources = client.list_all_resources().await?;
    assert!(
        resources
            .iter()
            .any(|resource| resource.uri == "wit://skill/SKILL.md")
    );
    let skill = client
        .read_resource(ReadResourceRequestParams::new("wit://skill/SKILL.md"))
        .await?;
    assert_eq!(skill.contents.len(), 1);

    let prompts = client.list_all_prompts().await?;
    assert!(
        prompts
            .iter()
            .any(|prompt| prompt.name == "wit_explore_repo")
    );

    let result = client
        .call_tool(CallToolRequestParams::new("wit_skill_load").with_arguments(object!({})))
        .await?;
    assert!(!result.content.is_empty());

    let tree = call_tool_json(
        &client,
        "wit_tree",
        object!({
            "repo": "owner/repo",
            "max_entries": 20,
            "max_bytes": 4096
        }),
    )
    .await?;
    assert!(tree["text"].as_str().unwrap().contains("README.md"));

    let ls = call_tool_json(
        &client,
        "wit_ls",
        object!({
            "repo": "owner/repo",
            "path": "src",
            "long": true
        }),
    )
    .await?;
    assert_eq!(ls["entries"][0]["name"], "lib.rs");

    let cat = call_tool_json(
        &client,
        "wit_cat",
        object!({
            "repo": "owner/repo",
            "path": "README.md",
            "number": true
        }),
    )
    .await?;
    assert!(cat["text"].as_str().unwrap().contains("     1  alpha"));

    let rg = call_tool_json(
        &client,
        "wit_rg",
        object!({
            "repo": "owner/repo",
            "pattern": "beta",
            "glob": "*.md"
        }),
    )
    .await?;
    assert_eq!(rg["matches"][0]["path"], "README.md");

    let sed = call_tool_json(
        &client,
        "wit_sed",
        object!({
            "repo": "owner/repo",
            "path": "README.md",
            "script": "2p",
            "quiet": true
        }),
    )
    .await?;
    assert_eq!(sed["text"], "beta\n");

    let head = call_tool_json(
        &client,
        "wit_head",
        object!({
            "repo": "owner/repo",
            "path": "README.md",
            "lines": 1
        }),
    )
    .await?;
    assert_eq!(head["text"], "alpha");

    let tail = call_tool_json(
        &client,
        "wit_tail",
        object!({
            "repo": "owner/repo",
            "path": "README.md",
            "from_line": 2,
            "number": true
        }),
    )
    .await?;
    assert!(tail["text"].as_str().unwrap().contains("     2  beta"));

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn wit_mcp_stdio_branch_parameter_reads_named_branch() -> anyhow::Result<()> {
    let bin = env!("CARGO_BIN_EXE_wit-mcp");
    let cache = tempfile::tempdir()?;
    let fixture = seed_cached_repo(cache.path())?;
    let transport = TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
        cmd.env("WIT_CACHE_DIR", cache.path())
            .env("GIT_CONFIG_GLOBAL", &fixture.git_config)
            .env("GIT_CONFIG_NOSYSTEM", "1");
    }))?;

    let client = ().serve(transport).await?;

    let refresh = call_tool_json(
        &client,
        "wit_cache_refresh",
        object!({
            "repo": "owner/repo",
            "branch": "feature/mcp"
        }),
    )
    .await?;
    assert_eq!(refresh["refreshed"], true);
    assert!(
        refresh["cache_path"]
            .as_str()
            .unwrap()
            .contains("b-feature%2Fmcp")
    );

    let tree = call_tool_json(
        &client,
        "wit_tree",
        object!({
            "repo": "owner/repo",
            "branch": "feature/mcp",
            "max_entries": 20,
            "max_bytes": 4096
        }),
    )
    .await?;
    assert!(tree["text"].as_str().unwrap().contains("feature.txt"));

    let ls = call_tool_json(
        &client,
        "wit_ls",
        object!({
            "repo": "owner/repo",
            "branch": "feature/mcp",
            "path": "src",
            "long": true
        }),
    )
    .await?;
    assert_eq!(ls["entries"][0]["name"], "lib.rs");

    let cat = call_tool_json(
        &client,
        "wit_cat",
        object!({
            "repo": "owner/repo",
            "branch": "feature/mcp",
            "path": "README.md"
        }),
    )
    .await?;
    assert!(cat["text"].as_str().unwrap().contains("feature alpha"));
    assert!(!cat["text"].as_str().unwrap().contains("alpha\nbeta"));

    let rg = call_tool_json(
        &client,
        "wit_rg",
        object!({
            "repo": "owner/repo",
            "branch": "feature/mcp",
            "pattern": "feature branch",
            "glob": "feature.txt"
        }),
    )
    .await?;
    assert_eq!(rg["matches"][0]["path"], "feature.txt");

    let sed = call_tool_json(
        &client,
        "wit_sed",
        object!({
            "repo": "owner/repo",
            "branch": "feature/mcp",
            "path": "README.md",
            "script": "2p",
            "quiet": true
        }),
    )
    .await?;
    assert_eq!(sed["text"], "feature beta\n");

    let head = call_tool_json(
        &client,
        "wit_head",
        object!({
            "repo": "owner/repo",
            "branch": "feature/mcp",
            "path": "README.md",
            "lines": 1
        }),
    )
    .await?;
    assert_eq!(head["text"], "feature alpha");

    let tail = call_tool_json(
        &client,
        "wit_tail",
        object!({
            "repo": "owner/repo",
            "branch": "feature/mcp",
            "path": "README.md",
            "from_line": 2,
            "number": true
        }),
    )
    .await?;
    assert!(
        tail["text"]
            .as_str()
            .unwrap()
            .contains("     2  feature beta")
    );

    client.cancel().await?;
    Ok(())
}

async fn call_tool_json(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    name: &str,
    arguments: serde_json::Map<String, Value>,
) -> anyhow::Result<Value> {
    let result = client
        .call_tool(CallToolRequestParams::new(name.to_string()).with_arguments(arguments))
        .await?;
    assert!(
        !result.is_error.unwrap_or(false),
        "{name} returned tool error: {:?}",
        result.content
    );
    result
        .structured_content
        .ok_or_else(|| anyhow::anyhow!("{name} should return structured content"))
}

struct CachedRepoFixture {
    _temp: tempfile::TempDir,
    git_config: PathBuf,
}

fn seed_cached_repo(cache_dir: &Path) -> anyhow::Result<CachedRepoFixture> {
    let temp = tempfile::tempdir()?;
    let worktree = temp.path().join("worktree");
    let remote = temp.path().join("remote.git");
    let branch_dir = cache_dir
        .join("owner")
        .join("repo")
        .join("branches")
        .join("b-main");
    let repo_path = branch_dir.join("repo.git");

    run_git(&["init", worktree.to_str().unwrap()], None)?;
    run_git(&["checkout", "-b", "main"], Some(&worktree))?;
    std::fs::create_dir_all(worktree.join("src"))?;
    std::fs::write(worktree.join("README.md"), "alpha\nbeta\ngamma\n")?;
    std::fs::write(worktree.join("src").join("lib.rs"), "pub fn demo() {}\n")?;
    run_git(&["add", "."], Some(&worktree))?;
    run_git(
        &[
            "-c",
            "user.name=wit-test",
            "-c",
            "user.email=wit-test@example.com",
            "commit",
            "-m",
            "init",
        ],
        Some(&worktree),
    )?;
    let sha = git_stdout(&["rev-parse", "HEAD"], Some(&worktree))?;

    run_git(&["init", "--bare", remote.to_str().unwrap()], None)?;
    run_git(
        &["remote", "add", "origin", remote.to_str().unwrap()],
        Some(&worktree),
    )?;
    run_git(&["push", "origin", "main"], Some(&worktree))?;
    run_git(&["symbolic-ref", "HEAD", "refs/heads/main"], Some(&remote))?;

    run_git(&["checkout", "-b", "feature/mcp"], Some(&worktree))?;
    std::fs::write(
        worktree.join("README.md"),
        "feature alpha\nfeature beta\nfeature gamma\n",
    )?;
    std::fs::write(worktree.join("feature.txt"), "feature branch marker\n")?;
    std::fs::write(
        worktree.join("src").join("lib.rs"),
        "pub fn feature_mcp() {}\n",
    )?;
    run_git(&["add", "."], Some(&worktree))?;
    run_git(
        &[
            "-c",
            "user.name=wit-test",
            "-c",
            "user.email=wit-test@example.com",
            "commit",
            "-m",
            "feature branch",
        ],
        Some(&worktree),
    )?;
    run_git(&["push", "origin", "feature/mcp"], Some(&worktree))?;
    run_git(&["checkout", "main"], Some(&worktree))?;

    std::fs::create_dir_all(&branch_dir)?;
    run_git(
        &[
            "clone",
            "--bare",
            worktree.to_str().unwrap(),
            repo_path.to_str().unwrap(),
        ],
        None,
    )?;

    std::fs::write(
        branch_dir.join("metadata.json"),
        serde_json::to_vec_pretty(&json!({
            "cache_schema_version": 1,
            "owner_repo": "owner/repo",
            "branch": "main",
            "remote_url": "https://github.com/owner/repo",
            "current_sha": sha,
            "last_checked_at": 1,
            "last_updated_at": 1
        }))?,
    )?;
    let git_config = temp.path().join("gitconfig");
    std::fs::write(
        &git_config,
        format!(
            "[url \"file://{}\"]\n\tinsteadOf = https://github.com/owner/repo\n\tinsteadOf = https://github.com/owner/repo.git\n",
            remote.display()
        ),
    )?;

    Ok(CachedRepoFixture {
        _temp: temp,
        git_config,
    })
}

fn run_git(args: &[&str], workdir: Option<&Path>) -> anyhow::Result<()> {
    let output = git_command(args, workdir).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn git_stdout(args: &[&str], workdir: Option<&Path>) -> anyhow::Result<String> {
    let output = git_command(args, workdir).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn git_command(args: &[&str], workdir: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    command.args(args);
    command.stdin(Stdio::null());
    if let Some(dir) = workdir {
        command.current_dir(dir);
    }
    command
}
