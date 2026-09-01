use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ReadResourceRequestParams, ResourceContents},
    object,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

/// Parallel `TokioChildProcess` MCP clients deadlock intermittently under cargo's
/// default test concurrency (CI hung for hours on guidance-resources / cancel).
fn stdio_child_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    &LOCK
}

async fn lock_stdio_child_tests() -> tokio::sync::MutexGuard<'static, ()> {
    stdio_child_lock().lock().await
}

fn lock_stdio_child_tests_blocking() -> tokio::sync::MutexGuard<'static, ()> {
    stdio_child_lock().blocking_lock()
}

type McpClient = rmcp::service::RunningService<rmcp::RoleClient, ()>;

/// `RunningService::cancel` can wait forever if the serve task never exits; bound it.
async fn shutdown_client(mut client: McpClient) -> anyhow::Result<()> {
    let _ = client.close_with_timeout(Duration::from_secs(5)).await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn stdio_cancel_notification_stops_dynamic_route_git_work() -> anyhow::Result<()> {
    use rmcp::{
        model::{ClientRequest, Request},
        service::PeerRequestOptions,
    };
    use std::os::unix::fs::PermissionsExt;

    let _serial = lock_stdio_child_tests().await;
    tokio::time::timeout(Duration::from_secs(30), async {
    let bin = env!("CARGO_BIN_EXE_wit-mcp");
    let temp = tempfile::tempdir()?;
    let fake_bin = temp.path().join("bin");
    std::fs::create_dir(&fake_bin)?;
    let fake_git = fake_bin.join("git");
    std::fs::write(
        &fake_git,
        "#!/bin/sh\ntouch \"$WIT_FAKE_GIT_STARTED\"\ntrap 'touch \"$WIT_FAKE_GIT_TERMINATED\"; exit 143' TERM\nwhile :; do sleep 1; done\n",
    )?;
    std::fs::set_permissions(&fake_git, std::fs::Permissions::from_mode(0o755))?;
    let started = temp.path().join("started");
    let terminated = temp.path().join("terminated");
    let mut paths = vec![fake_bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(paths)?;
    let transport = TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
        cmd.kill_on_drop(true)
            .env("PATH", path)
            .env("WIT_FAKE_GIT_STARTED", &started)
            .env("WIT_FAKE_GIT_TERMINATED", &terminated);
    }))?;
    let client = ().serve(transport).await?;
    let request = ClientRequest::CallToolRequest(Request::new(
        CallToolRequestParams::new("wit_refs").with_arguments(object!({
            "repo": "owner/repo"
        })),
    ));
    let handle = client
        .send_cancellable_request(request, PeerRequestOptions::no_options())
        .await?;

    wait_for_path(&started, std::time::Duration::from_secs(2)).await?;
    handle.cancel(Some("test cancellation".to_string())).await?;
    wait_for_path(&terminated, std::time::Duration::from_secs(2)).await?;
    assert_eq!(client.list_all_tools().await?.len(), 7);

    shutdown_client(client).await?;
    Ok(())
    })
    .await
    .map_err(|_| anyhow::anyhow!("stdio_cancel_notification_stops_dynamic_route_git_work timed out"))?
}

#[cfg(unix)]
async fn wait_for_path(path: &Path, timeout: std::time::Duration) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if path.exists() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for {}", path.display());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
#[tokio::test]
async fn wit_mcp_exposes_snapshot_first_guidance_resources() -> anyhow::Result<()> {
    // In-process duplex avoids TokioChildProcess hangs that flake under CI
    // (the child-process path left orphan wit-mcp and blocked cargo test for hours).
    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        wit::mcp::WitMcpServer::new()
            .serve(server_transport)
            .await?
            .waiting()
            .await?;
        anyhow::Ok(())
    });
    let client = ().serve(client_transport).await?;

    let resources = client.list_all_resources().await?;
    let uris = resources
        .iter()
        .map(|resource| resource.uri.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        uris,
        vec![
            "wit://skill/SKILL.md",
            "wit://guide/workflow",
            "wit://guide/tools",
        ]
    );
    for uri in uris {
        let resource = client
            .read_resource(ReadResourceRequestParams::new(uri))
            .await?;
        assert_eq!(resource.contents.len(), 1);
        let ResourceContents::TextResourceContents { text, .. } = &resource.contents[0] else {
            anyhow::bail!("{uri} should be a text resource");
        };
        assert!(text.contains("snapshot"));
        assert!(!text.contains("compat-v1"));
        assert!(!text.contains("MCP v1"));
    }

    shutdown_client(client).await?;
    server.await??;
    Ok(())
}

#[test]
fn wit_mcp_rejects_unknown_arguments_and_preserves_help_and_version() -> anyhow::Result<()> {
    let _serial = lock_stdio_child_tests_blocking();
    let bin = env!("CARGO_BIN_EXE_wit-mcp");
    let unknown = Command::new(bin).arg("--unknown").output()?;
    assert!(!unknown.status.success());
    let stderr = String::from_utf8(unknown.stderr)?;
    assert!(stderr.contains("unsupported argument --unknown"));
    assert!(stderr.contains("wit-mcp --help"));

    let help = Command::new(bin).arg("--help").output()?;
    assert!(help.status.success());
    let help_stdout = String::from_utf8(help.stdout)?;
    assert!(help_stdout.contains("USAGE:"));
    assert!(help_stdout.contains("wit-mcp --version"));

    let version = Command::new(bin).arg("--version").output()?;
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout)?.trim(),
        format!("wit-mcp {}", env!("CARGO_PKG_VERSION"))
    );
    Ok(())
}

#[tokio::test]
async fn wit_mcp_v2_snapshot_provenance_pagination_and_replay() -> anyhow::Result<()> {
    let _serial = lock_stdio_child_tests().await;
    // Name the step in flight so a timeout reports where the test blocked
    // instead of a bare "timed out" (the only signal CI gives on a hang).
    let step = std::sync::Arc::new(std::sync::Mutex::new("startup"));
    let mark = {
        let step = std::sync::Arc::clone(&step);
        move |name: &'static str| *step.lock().unwrap() = name
    };
    tokio::time::timeout(Duration::from_secs(120), async {
        let bin = env!("CARGO_BIN_EXE_wit-mcp");
        let cache = tempfile::tempdir()?;
        let fixture = seed_cached_repo(cache.path())?;
        let transport =
            TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
                cmd.kill_on_drop(true)
                    .env("WIT_CACHE_DIR", cache.path())
                    .env("GIT_CONFIG_GLOBAL", &fixture.git_config)
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    // If an insteadOf rewrite ever misses, git must fail fast,
                    // not block on a credential prompt (hangs Windows CI).
                    .env("GIT_TERMINAL_PROMPT", "0")
                    .env("GCM_INTERACTIVE", "never");
            }))?;
        mark("serve handshake");
        let client = ().serve(transport).await?;

        mark("list_all_tools");
        let tools = client.list_all_tools().await?;
        let names = tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "wit_context",
                "wit_find_repositories",
                "wit_list",
                "wit_open",
                "wit_read",
                "wit_refs",
                "wit_search_code",
            ]
        );
        assert!(!names.contains(&"wit_cat"));

        mark("wit_refs");
        let refs = call_tool_json(
            &client,
            "wit_refs",
            object!({
                "repo": "owner/repo",
                "max_items": 10,
                "max_bytes": 8192
            }),
        )
        .await?;
        assert!(refs["items"].as_array().unwrap().iter().any(|item| {
            item["resolved_ref"] == "refs/heads/main" && item["is_default"] == true
        }));
        assert!(
            refs["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| { item["resolved_ref"] == "refs/tags/v-test" })
        );

        mark("wit_open allow_stale");
        let opened = call_tool_json(
            &client,
            "wit_open",
            object!({
                "repo": "owner/repo",
                "freshness": "allow_stale"
            }),
        )
        .await?;
        mark("pagination and search");
        let snapshot_id = opened["snapshot_id"].as_str().unwrap().to_string();
        let original_sha = opened["commit_sha"].as_str().unwrap().to_string();
        assert_eq!(opened["api_version"], "2");
        assert_eq!(opened["resolved_ref"], "refs/heads/main");
        assert_eq!(opened["cache"]["state"], "stale_served_revalidating");
        assert!(
            opened["capabilities"]["pull_request_heads"]
                .as_str()
                .unwrap()
                .contains("not_supported")
        );

        let first_list = call_tool_json(
            &client,
            "wit_list",
            object!({
                "snapshot_id": snapshot_id,
                "depth": 3,
                "max_items": 1,
                "max_bytes": 4096
            }),
        )
        .await?;
        assert_eq!(first_list["returned_items"], 1);
        assert_eq!(first_list["has_more"], true);
        assert!(first_list["next_cursor"].is_string());
        assert!(first_list["budget"]["serialized_bytes"].as_u64().unwrap() <= 4096);
        assert_eq!(
            serde_json::to_vec(&first_list)?.len() as u64,
            first_list["budget"]["serialized_bytes"].as_u64().unwrap()
        );
        let first_path = first_list["items"][0]["path"].as_str().unwrap().to_string();
        assert_eq!(first_list["items"][0]["commit_sha"], original_sha);

        let second_list = call_tool_json(
            &client,
            "wit_list",
            object!({
                "snapshot_id": snapshot_id,
                "depth": 3,
                "max_items": 1,
                "max_bytes": 4096,
                "cursor": first_list["next_cursor"]
            }),
        )
        .await?;
        assert_ne!(second_list["items"][0]["path"], first_path);

        let mismatched_cursor = client
            .call_tool(
                CallToolRequestParams::new("wit_list").with_arguments(object!({
                    "snapshot_id": snapshot_id,
                    "depth": 2,
                    "max_items": 1,
                    "max_bytes": 4096,
                    "cursor": first_list["next_cursor"]
                })),
            )
            .await?;
        assert_eq!(mismatched_cursor.is_error, Some(true));
        assert!(format!("{:?}", mismatched_cursor.content).contains("does not match"));

        let search = call_tool_json(
            &client,
            "wit_search_code",
            object!({
                "snapshot_id": snapshot_id,
                "queries": ["beta", "demo"],
                "globs": ["*.md", "**/*.rs"],
                "context_lines": 1,
                "max_results": 10,
                "max_bytes": 8192
            }),
        )
        .await?;
        let beta = search["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["query"] == "beta")
            .unwrap();
        assert_eq!(beta["path"], "README.md");
        assert_eq!(beta["match_line"], 2);
        assert_eq!(beta["start_line"], 1);
        assert_eq!(beta["end_line"], 3);
        assert_eq!(beta["commit_sha"], original_sha);
        assert!(beta["blob_sha"].as_str().unwrap().len() >= 40);

        let read = call_tool_json(
            &client,
            "wit_read",
            object!({
                "snapshot_id": snapshot_id,
                "path": "README.md",
                "start_line": 2,
                "end_line": 2,
                "max_bytes": 4096
            }),
        )
        .await?;
        assert_eq!(read["items"][0]["text"], "beta");
        assert_eq!(read["items"][0]["start_line"], 2);
        assert_eq!(read["items"][0]["end_line"], 2);
        assert!(read.get("rendered_text").is_none());

        let mut paged_lines = Vec::new();
        let mut read_cursor: Option<String> = None;
        loop {
            let mut arguments = object!({
                "snapshot_id": snapshot_id,
                "path": "README.md",
                "start_line": 1,
                "end_line": 3,
                "max_lines": 1,
                "max_bytes": 4096
            });
            if let Some(cursor) = &read_cursor {
                arguments.insert("cursor".to_string(), Value::String(cursor.clone()));
            }
            let page = call_tool_json(&client, "wit_read", arguments).await?;
            paged_lines.push(page["items"][0]["text"].as_str().unwrap().to_string());
            if page["has_more"] == false {
                break;
            }
            read_cursor = Some(page["next_cursor"].as_str().unwrap().to_string());
        }
        assert_eq!(paged_lines, vec!["alpha", "beta", "gamma"]);

        let first_search_page = call_tool_json(
            &client,
            "wit_search_code",
            object!({
                "snapshot_id": snapshot_id,
                "queries": ["alpha|beta|gamma"],
                "globs": ["*.md"],
                "context_lines": 1,
                "max_results": 1,
                "max_bytes": 4096
            }),
        )
        .await?;
        assert_eq!(first_search_page["has_more"], true);
        let second_search_page = call_tool_json(
            &client,
            "wit_search_code",
            object!({
                "snapshot_id": snapshot_id,
                "queries": ["alpha|beta|gamma"],
                "globs": ["*.md"],
                "context_lines": 1,
                "max_results": 1,
                "max_bytes": 4096,
                "cursor": first_search_page["next_cursor"]
            }),
        )
        .await?;
        assert_ne!(
            first_search_page["items"][0]["match_line"],
            second_search_page["items"][0]["match_line"]
        );

        let high_match = call_tool_json(
            &client,
            "wit_search_code",
            object!({
                "snapshot_id": snapshot_id,
                "queries": ["high match"],
                "globs": ["high_matches.txt"],
                "context_lines": 1,
                "max_results": 5,
                "max_bytes": 8192
            }),
        )
        .await?;
        assert_eq!(high_match["returned_items"], 5);
        assert_eq!(high_match["has_more"], true);
        assert!(high_match["next_cursor"].is_string());

        let context = call_tool_json(
            &client,
            "wit_context",
            object!({
                "snapshot_id": snapshot_id,
                "queries": ["alpha", "beta"],
                "context_lines": 1,
                "max_results": 10,
                "max_bytes": 8192
            }),
        )
        .await?;
        assert_eq!(context["items"][0]["path"], "README.md");
        assert!(context["items"][0]["score"].as_i64().unwrap() > 0);
        assert!(
            context["items"][0]["ranking_reasons"]
                .as_array()
                .unwrap()
                .len()
                >= 2
        );

        mark("wit_open feature/mcp");
        let named = call_tool_json(
            &client,
            "wit_open",
            object!({ "repo": "owner/repo", "ref": "feature/mcp" }),
        )
        .await?;
        assert_eq!(named["resolved_ref"], "refs/heads/feature/mcp");
        let named_read = call_tool_json(
            &client,
            "wit_read",
            object!({
                "snapshot_id": named["snapshot_id"],
                "path": "README.md",
                "start_line": 1,
                "end_line": 1,
                "max_bytes": 4096
            }),
        )
        .await?;
        assert_eq!(named_read["items"][0]["text"], "feature alpha");

        mark("move_main_branch and replay");
        let moved_sha = move_main_branch(&fixture, "changed after snapshot\n")?;
        assert_ne!(moved_sha, original_sha);
        let exact_replay = call_tool_json(
            &client,
            "wit_read",
            object!({
                "snapshot_id": snapshot_id,
                "path": "README.md",
                "start_line": 2,
                "end_line": 2,
                "max_bytes": 4096
            }),
        )
        .await?;
        assert_eq!(exact_replay, read);
        let replay = call_tool_json(
            &client,
            "wit_read",
            object!({
                "snapshot_id": snapshot_id,
                "path": "README.md",
                "start_line": 1,
                "end_line": 3,
                "max_bytes": 4096
            }),
        )
        .await?;
        assert_eq!(replay["items"][0]["text"], "alpha");
        assert_eq!(replay["items"][0]["commit_sha"], original_sha);

        mark("wit_open require_fresh");
        let fresh = call_tool_json(
            &client,
            "wit_open",
            object!({
                "repo": "owner/repo",
                "ref": "main",
                "freshness": "require_fresh"
            }),
        )
        .await?;
        assert_eq!(fresh["commit_sha"], moved_sha);
        assert_eq!(fresh["cache"]["state"], "explicitly_refreshed");

        mark("wit_open tag and sha");
        let tag = call_tool_json(
            &client,
            "wit_open",
            object!({ "repo": "owner/repo", "ref": "v-test" }),
        )
        .await?;
        assert_eq!(tag["resolved_ref"], "refs/tags/v-test");
        let by_sha = call_tool_json(
            &client,
            "wit_open",
            object!({ "repo": "owner/repo", "ref": tag["commit_sha"] }),
        )
        .await?;
        assert_eq!(by_sha["commit_sha"], tag["commit_sha"]);

        mark("shutdown");
        shutdown_client(client).await?;
        Ok(())
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "wit_mcp_v2_snapshot_provenance_pagination_and_replay timed out at step '{}'",
            step.lock().unwrap()
        )
    })?
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
    temp: tempfile::TempDir,
    git_config: PathBuf,
    worktree: PathBuf,
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
    std::fs::write(
        worktree.join("high_matches.txt"),
        "high match\n".repeat(10_000),
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
    run_git(&["tag", "v-test"], Some(&worktree))?;
    run_git(&["push", "origin", "refs/tags/v-test"], Some(&worktree))?;

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
            "[url \"{}\"]\n\tinsteadOf = https://github.com/owner/repo\n\tinsteadOf = https://github.com/owner/repo.git\n",
            file_url(&remote)
        ),
    )?;

    Ok(CachedRepoFixture {
        temp,
        git_config,
        worktree,
    })
}

fn move_main_branch(fixture: &CachedRepoFixture, content: &str) -> anyhow::Result<String> {
    let _keep_alive = fixture.temp.path();
    run_git(&["checkout", "main"], Some(&fixture.worktree))?;
    std::fs::write(fixture.worktree.join("README.md"), content)?;
    run_git(&["add", "README.md"], Some(&fixture.worktree))?;
    run_git(
        &[
            "-c",
            "user.name=wit-test",
            "-c",
            "user.email=wit-test@example.com",
            "commit",
            "-m",
            "move main",
        ],
        Some(&fixture.worktree),
    )?;
    run_git(&["push", "origin", "main"], Some(&fixture.worktree))?;
    git_stdout(&["rev-parse", "HEAD"], Some(&fixture.worktree))
}

/// A file URL git parses identically on every platform. `Path::display` on
/// Windows yields backslashes, which git's config parser silently strips
/// inside a quoted subsection (`\U` -> `U`), corrupting the insteadOf target
/// so remote access escapes the fixture.
fn file_url(path: &Path) -> String {
    let path = path.display().to_string().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
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
