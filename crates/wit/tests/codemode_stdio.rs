use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ClientRequest, Request},
    object,
    service::PeerRequestOptions,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const CODE_TOOL: &str = "code";

/// Parallel Code Mode stdio children deadlock intermittently (CI hung on
/// `main_wit_binary_serves_code_mode` for hours under default test threads).
fn stdio_child_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    &LOCK
}

async fn lock_stdio_child_tests() -> tokio::sync::MutexGuard<'static, ()> {
    stdio_child_lock().lock().await
}

type McpClient = rmcp::service::RunningService<rmcp::RoleClient, ()>;

async fn shutdown_client(mut client: McpClient) -> anyhow::Result<()> {
    let _ = client.close_with_timeout(Duration::from_secs(5)).await?;
    Ok(())
}

#[tokio::test]
async fn shipped_code_mode_lists_deterministic_typed_contract() -> anyhow::Result<()> {
    let _serial = lock_stdio_child_tests().await;
    let temp = tempfile::tempdir()?;
    let client = start_code_client(temp.path(), None).await?;

    let tools = client.list_all_tools().await?;
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, CODE_TOOL);
    let description = tools[0]
        .description
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("code tool should have a description"))?;
    let declarations = include_str!("../codemode.wit.d.ts");
    assert!(description.ends_with(declarations));
    assert!(description.contains("help():"));
    for method in [
        "findRepositories",
        "refs",
        "open",
        "list",
        "searchCode",
        "read",
        "context",
    ] {
        assert!(description.contains(&format!("{method}(arguments:")));
    }
    assert_eq!(
        serde_json::to_vec(&tools)?,
        serde_json::to_vec(&client.list_all_tools().await?)?
    );

    shutdown_client(client).await?;
    wait_for_empty_directory(temp.path()).await
}

#[tokio::test]
async fn shipped_code_mode_supports_progressive_help_and_method_suggestions() -> anyhow::Result<()>
{
    let _serial = lock_stdio_child_tests().await;
    let temp = tempfile::tempdir()?;
    let client = start_code_client(temp.path(), None).await?;

    let result = call_code_success(
        &client,
        r#"
        const overview = codemode.wit.help();
        const read = codemode.wit.help("read");
        let unknown;
        try {
          await codemode.wit.searchRepositories({ pattern: "ratzilla", max_items: 5 });
        } catch (error) {
          unknown = { code: error.code, operation: error.operation, message: error.message };
        }
        return { methods: Object.keys(codemode.wit), overview, read, unknown };
        "#,
    )
    .await?;

    assert_eq!(
        result["methods"],
        json!([
            "help",
            "findRepositories",
            "refs",
            "open",
            "list",
            "searchCode",
            "read",
            "context"
        ])
    );
    assert_eq!(
        result["overview"]["methods"]
            .as_array()
            .unwrap()
            .iter()
            .map(|method| method["name"].clone())
            .collect::<Vec<_>>(),
        json!([
            "findRepositories",
            "refs",
            "open",
            "list",
            "searchCode",
            "read",
            "context"
        ])
        .as_array()
        .unwrap()
        .clone()
    );
    assert_eq!(result["read"]["name"], "read");
    assert!(
        result["read"]["signature"]
            .as_str()
            .unwrap()
            .contains("format")
    );
    assert_eq!(result["unknown"]["code"], "unknown_method");
    assert_eq!(
        result["unknown"]["operation"],
        "codemode.wit.searchRepositories"
    );
    assert!(
        result["unknown"]["message"]
            .as_str()
            .unwrap()
            .contains("findRepositories")
    );

    shutdown_client(client).await?;
    wait_for_empty_directory(temp.path()).await
}

#[tokio::test]
async fn shipped_code_mode_compacts_reads_and_lists_and_filters_search_paths() -> anyhow::Result<()>
{
    let _serial = lock_stdio_child_tests().await;
    let temp = tempfile::tempdir()?;
    let cache = temp.path().join("cache");
    std::fs::create_dir(&cache)?;
    let fixture = seed_cached_repo(&cache)?;
    let client = start_code_client(temp.path(), Some((&cache, &fixture.git_config))).await?;

    let result = call_code_success(
        &client,
        r#"
        const opened = await codemode.wit.open({ repo: "owner/repo" });
        const text = await codemode.wit.read({
          snapshot_id: opened.snapshot_id, path: "README.md", start_line: 1, end_line: 2
        });
        const lines = await codemode.wit.read({
          snapshot_id: opened.snapshot_id, path: "README.md", start_line: 2, end_line: 3,
          format: "lines"
        });
        const structured = await codemode.wit.read({
          snapshot_id: opened.snapshot_id, path: "README.md", start_line: 1, end_line: 1,
          format: "structured"
        });
        const listed = await codemode.wit.list({
          snapshot_id: opened.snapshot_id, path: "src", depth: 1, format: "paths"
        });
        const prefixed = await codemode.wit.searchCode({
          snapshot_id: opened.snapshot_id, queries: ["pub fn"], path_prefix: "src"
        });
        const excluded = await codemode.wit.searchCode({
          snapshot_id: opened.snapshot_id, queries: ["alpha"], exclude: ["README.md"]
        });
        const globbed = await codemode.wit.searchCode({
          snapshot_id: opened.snapshot_id, queries: ["alpha"], glob: "README.md"
        });
        const nearLimit = await codemode.wit.read({
          snapshot_id: opened.snapshot_id, path: "medium.txt", max_bytes: 2048
        });
        return { opened, text, lines, structured, listed, prefixed, excluded, globbed, nearLimit };
        "#,
    )
    .await?;

    assert_eq!(result["text"]["format"], "text");
    assert_eq!(result["text"]["text"], "alpha\nbeta");
    assert!(result["text"].get("items").is_none());
    assert_eq!(result["text"]["path"], "README.md");
    assert_eq!(result["text"]["start_line"], 1);
    assert_eq!(result["text"]["end_line"], 2);
    assert_eq!(result["lines"]["format"], "lines");
    assert_eq!(
        result["lines"]["lines"],
        json!([
            { "line_number": 2, "text": "beta" },
            { "line_number": 3, "text": "gamma" }
        ])
    );
    assert_eq!(result["structured"]["items"][0]["text"], "alpha");
    assert_eq!(result["listed"]["format"], "paths");
    assert_eq!(result["listed"]["paths"], json!(["src/lib.rs"]));
    assert!(result["listed"].get("items").is_none());
    assert_eq!(result["prefixed"]["items"][0]["path"], "src/lib.rs");
    assert!(result["excluded"]["items"].as_array().unwrap().is_empty());
    assert_eq!(result["globbed"]["items"][0]["path"], "README.md");
    assert!(result["nearLimit"]["budget"]["remaining_bytes"].is_number());
    assert!(
        result["nearLimit"]["budget"]["warning"]
            .as_str()
            .unwrap()
            .contains("near max_bytes")
    );

    for key in ["text", "lines", "listed"] {
        assert_eq!(result[key]["repo"], result["opened"]["repo"]);
        assert_eq!(result[key]["commit_sha"], result["opened"]["commit_sha"]);
        assert_eq!(result[key]["snapshot_id"], result["opened"]["snapshot_id"]);
    }

    shutdown_client(client).await?;
    Ok(())
}

#[tokio::test]
async fn main_wit_binary_serves_code_mode() -> anyhow::Result<()> {
    let _serial = lock_stdio_child_tests().await;
    tokio::time::timeout(Duration::from_secs(60), async {
        let temp = tempfile::tempdir()?;
        let transport = TokioChildProcess::new(
            tokio::process::Command::new(env!("CARGO_BIN_EXE_wit")).configure(|command| {
                command
                    .kill_on_drop(true)
                    .args(["mcp", "--transport", "stdio", "--mode", "code"])
                    .env("TMPDIR", temp.path())
                    .env("TMP", temp.path())
                    .env("TEMP", temp.path());
            }),
        )?;
        let client = ().serve(transport).await?;

        let tools = client.list_all_tools().await?;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, CODE_TOOL);
        assert_eq!(
            call_code_success(&client, "return codemode.wit.help('open').name;").await?,
            "open"
        );

        shutdown_client(client).await?;
        wait_for_empty_directory(temp.path()).await
    })
    .await
    .map_err(|_| anyhow::anyhow!("main_wit_binary_serves_code_mode timed out"))?
}

#[tokio::test]
async fn shipped_code_mode_composes_snapshot_operations_and_pins_branch() -> anyhow::Result<()> {
    let _serial = lock_stdio_child_tests().await;
    let temp = tempfile::tempdir()?;
    let cache = temp.path().join("cache");
    std::fs::create_dir(&cache)?;
    let fixture = seed_cached_repo(&cache)?;
    let client = start_code_client(temp.path(), Some((&cache, &fixture.git_config))).await?;

    let result = call_code_success(
        &client,
        r#"
        const opened = await codemode.wit.open({ repo: "owner/repo" });
        const firstList = await codemode.wit.list({
          snapshot_id: opened.snapshot_id, depth: 3, max_items: 1, max_bytes: 4096
        });
        const secondList = await codemode.wit.list({
          snapshot_id: opened.snapshot_id, depth: 3, max_items: 1, max_bytes: 4096,
          cursor: firstList.next_cursor
        });
        let mismatch;
        try {
          await codemode.wit.list({
            snapshot_id: opened.snapshot_id, depth: 2, max_items: 1, max_bytes: 4096,
            cursor: firstList.next_cursor
          });
        } catch (error) {
          mismatch = { code: error.code, operation: error.operation, message: error.message };
        }
        const firstSearch = await codemode.wit.searchCode({
          snapshot_id: opened.snapshot_id, queries: ["alpha|beta|gamma"], globs: ["*.md"],
          max_results: 1, max_bytes: 4096
        });
        const secondSearch = await codemode.wit.searchCode({
          snapshot_id: opened.snapshot_id, queries: ["alpha|beta|gamma"], globs: ["*.md"],
          max_results: 1, max_bytes: 4096, cursor: firstSearch.next_cursor
        });
        const read = await codemode.wit.read({
          snapshot_id: opened.snapshot_id, path: firstSearch.items[0].path,
          start_line: firstSearch.items[0].match_line,
          end_line: firstSearch.items[0].match_line, format: "structured", max_bytes: 4096
        });
        const context = await codemode.wit.context({
          snapshot_id: opened.snapshot_id, queries: ["demo"], max_results: 2, max_bytes: 4096
        });
        return { opened, firstList, secondList, mismatch, firstSearch, secondSearch, read, context };
        "#,
    )
    .await?;

    let opened = &result["opened"];
    assert_eq!(opened["repo"], "owner/repo");
    assert_eq!(result["firstList"]["has_more"], true);
    assert_ne!(
        result["firstList"]["items"][0]["path"],
        result["secondList"]["items"][0]["path"]
    );
    assert_eq!(result["mismatch"]["code"], "operation_failed");
    assert_eq!(result["mismatch"]["operation"], "wit_list");
    assert_eq!(result["mismatch"]["message"], "wit operation failed");
    assert_ne!(
        result["firstSearch"]["items"][0]["match_line"],
        result["secondSearch"]["items"][0]["match_line"]
    );
    for item in [
        &result["firstSearch"]["items"][0],
        &result["secondSearch"]["items"][0],
        &result["read"]["items"][0],
        &result["context"]["items"][0],
    ] {
        assert_eq!(item["repo"], opened["repo"]);
        assert_eq!(item["commit_sha"], opened["commit_sha"]);
        assert_eq!(item["snapshot_id"], opened["snapshot_id"]);
        assert!(item["blob_sha"].as_str().is_some_and(|sha| sha.len() >= 40));
        assert!(item["path"].is_string());
        assert!(item["start_line"].as_u64().is_some_and(|line| line > 0));
        assert!(item["end_line"].as_u64().is_some_and(|line| line > 0));
    }

    let original_sha = opened["commit_sha"].as_str().unwrap().to_owned();
    let snapshot_id = opened["snapshot_id"].as_str().unwrap().to_owned();
    let moved_sha = move_main_branch(&fixture, "changed after snapshot\n")?;
    assert_ne!(moved_sha, original_sha);
    let replay = call_code_success(
        &client,
        &format!(
            r#"
            const oldRead = await codemode.wit.read({{
              snapshot_id: {}, path: "README.md", start_line: 1, end_line: 1, max_bytes: 4096
            }});
            const fresh = await codemode.wit.open({{
              repo: "owner/repo", ref: "main", freshness: "require_fresh"
            }});
            const freshRead = await codemode.wit.read({{
              snapshot_id: fresh.snapshot_id, path: "README.md", start_line: 1, end_line: 1,
              max_bytes: 4096
            }});
            return {{ oldRead, fresh, freshRead }};
            "#,
            serde_json::to_string(&snapshot_id)?
        ),
    )
    .await?;
    assert_eq!(replay["oldRead"]["text"], "alpha");
    assert_eq!(replay["oldRead"]["commit_sha"], original_sha);
    assert_eq!(replay["fresh"]["commit_sha"], moved_sha);
    assert_eq!(replay["freshRead"]["text"], "changed after snapshot");

    shutdown_client(client).await?;
    Ok(())
}

#[tokio::test]
async fn shipped_code_mode_exhausts_list_and_search_cursors_stably() -> anyhow::Result<()> {
    let _serial = lock_stdio_child_tests().await;
    let temp = tempfile::tempdir()?;
    let cache = temp.path().join("cache");
    std::fs::create_dir(&cache)?;
    let fixture = seed_cached_repo(&cache)?;
    let client = start_code_client(temp.path(), Some((&cache, &fixture.git_config))).await?;
    let source = r#"
        const opened = await codemode.wit.open({ repo: "owner/repo" });
        const listed = [];
        let listCursor;
        let listFinal;
        do {
          const page = await codemode.wit.list({
            snapshot_id: opened.snapshot_id, depth: 3, max_items: 1, max_bytes: 4096,
            ...(listCursor ? { cursor: listCursor } : {})
          });
          listed.push(...page.items.map(item => item.path));
          listFinal = { has_more: page.has_more, next_cursor: page.next_cursor ?? null };
          listCursor = page.next_cursor;
        } while (listCursor);
        const searched = [];
        let searchCursor;
        let searchFinal;
        do {
          const page = await codemode.wit.searchCode({
            snapshot_id: opened.snapshot_id, queries: ["alpha|beta|gamma"], globs: ["README.md"],
            max_results: 1, max_bytes: 4096,
            ...(searchCursor ? { cursor: searchCursor } : {})
          });
          searched.push(...page.items.map(item => `${item.path}:${item.match_line}:${item.query}`));
          searchFinal = { has_more: page.has_more, next_cursor: page.next_cursor ?? null };
          searchCursor = page.next_cursor;
        } while (searchCursor);
        return { listed, listFinal, searched, searchFinal };
    "#;
    let first = call_code_success(&client, source).await?;
    let second = call_code_success(&client, source).await?;
    assert_eq!(first, second);
    assert_eq!(
        first["listFinal"],
        json!({ "has_more": false, "next_cursor": null })
    );
    assert_eq!(
        first["searchFinal"],
        json!({ "has_more": false, "next_cursor": null })
    );
    assert_eq!(
        first["searched"],
        json!([
            "README.md:1:alpha|beta|gamma",
            "README.md:2:alpha|beta|gamma",
            "README.md:3:alpha|beta|gamma"
        ])
    );
    for key in ["listed", "searched"] {
        let values = first[key].as_array().unwrap();
        let unique = values.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(values.len(), unique.len(), "{key} contains duplicates");
    }
    shutdown_client(client).await?;
    Ok(())
}

#[tokio::test]
async fn shipped_code_mode_enforces_host_concurrency_and_byte_budgets() -> anyhow::Result<()> {
    let _serial = lock_stdio_child_tests().await;
    let temp = tempfile::tempdir()?;
    let cache = temp.path().join("cache");
    std::fs::create_dir(&cache)?;
    let fixture = seed_cached_repo(&cache)?;
    let client = start_code_client(temp.path(), Some((&cache, &fixture.git_config))).await?;

    let concurrency = call_code_success(
        &client,
        r#"
        const opened = await codemode.wit.open({ repo: "owner/repo" });
        const results = await Promise.all(Array.from({ length: 5 }, async () => {
          try {
            await codemode.wit.read({ snapshot_id: opened.snapshot_id, path: "README.md", start_line: 1, end_line: 1 });
            return "ok";
          } catch (error) { return error.code; }
        }));
        return results;
        "#,
    )
    .await?;
    assert!(
        concurrency
            .as_array()
            .unwrap()
            .iter()
            .any(|code| code == "host_concurrency_limit")
    );

    let per_call = call_code_success(
        &client,
        r#"/* wit-adversarial:per-call-bytes */
        const opened = await codemode.wit.open({ repo: "owner/repo" });
        try {
          await codemode.wit.read({ snapshot_id: opened.snapshot_id, path: "medium.txt", start_line: 1, end_line: 1, max_bytes: 4096 });
        } catch (error) { return error.code; }
        return "missing-error";
        "#,
    )
    .await?;
    assert_eq!(per_call, "host_result_bytes_limit");

    let cumulative = call_code_success(
        &client,
        r#"/* wit-adversarial:cumulative-bytes */
        const opened = await codemode.wit.open({ repo: "owner/repo" });
        const codes = [];
        for (let index = 0; index < 3; index += 1) {
          try {
            await codemode.wit.read({ snapshot_id: opened.snapshot_id, path: "medium.txt", start_line: 1, end_line: 1, max_bytes: 4096 });
          } catch (error) { codes.push(error.code); }
        }
        return codes;
        "#,
    )
    .await?;
    assert!(
        cumulative
            .as_array()
            .unwrap()
            .iter()
            .any(|code| code == "cumulative_host_bytes_limit")
    );
    assert_eq!(
        call_code_success(&client, "return 'after-host-limits';").await?,
        "after-host-limits"
    );
    shutdown_client(client).await?;
    Ok(())
}

#[tokio::test]
async fn shipped_code_mode_contains_failures_and_recovers_without_temp_leaks() -> anyhow::Result<()>
{
    let _serial = lock_stdio_child_tests().await;
    let temp = tempfile::tempdir()?;
    let scratch = temp.path().join("scratch");
    std::fs::create_dir(&scratch)?;
    let pid_log = temp.path().join("worker-pids");
    let client = start_code_client_with_pid_log(&scratch, None, &pid_log).await?;

    for (source, accepted_codes) in [
        ("throw new Error('boom');", &["code_rejected"][..]),
        (
            "function recurse() { return recurse(); } return recurse();",
            &["code_rejected", "worker_exited", "deadline_exceeded"][..],
        ),
        (
            "const held = []; while (true) held.push(new ArrayBuffer(1024 * 1024));",
            &["code_rejected", "worker_exited", "deadline_exceeded"][..],
        ),
        (
            "return 'x'.repeat(60 * 1024);",
            &["final_result_bytes_limit"][..],
        ),
    ] {
        let result = call_code(&client, source).await?;
        assert_eq!(
            result.is_error,
            Some(true),
            "source unexpectedly succeeded: {source}"
        );
        let code = result
            .structured_content
            .as_ref()
            .and_then(|value| value["code"].as_str());
        assert!(
            code.is_some_and(|code| accepted_codes.contains(&code)),
            "unexpected failure for {source}: {:?}",
            result.structured_content
        );
        if code == Some("final_result_bytes_limit") {
            let message = result.structured_content.as_ref().unwrap()["message"]
                .as_str()
                .unwrap();
            assert!(message.contains("bytes"));
            assert!(message.contains("compact read/list formats"));
        }
        assert_eq!(
            call_code_success(&client, "return 'recovered';").await?,
            "recovered"
        );
        wait_for_empty_directory(&scratch).await?;
    }

    let exhausted = call_code_success(
        &client,
        r#"
        const codes = [];
        for (let i = 0; i < 17; i += 1) {
          try { await codemode.wit.read({}); }
          catch (error) { codes.push(error.code); }
        }
        return codes;
        "#,
    )
    .await?;
    let exhausted = exhausted.as_array().unwrap();
    assert!(exhausted.iter().any(|code| code == "pages_limit"));
    assert!(exhausted.iter().any(|code| code == "host_calls_limit"));
    let uncaught_limit = call_code(
        &client,
        "for (let i = 0; i < 17; i += 1) { try { await codemode.wit.read({}); } catch (error) { if (error.code === 'host_calls_limit') throw error; } } return null;",
    )
    .await?;
    assert_eq!(uncaught_limit.is_error, Some(true));
    assert_eq!(
        uncaught_limit.structured_content.unwrap()["code"],
        "host_calls_limit"
    );
    assert_eq!(
        call_code_success(&client, "return 'after-limits';").await?,
        "after-limits"
    );
    wait_for_empty_directory(&scratch).await?;

    let oversized_source = " ".repeat(wit_quickjs_spike::MAX_SCRIPT_BYTES + 1);
    assert_eq!(
        call_code(&client, &oversized_source)
            .await?
            .structured_content
            .unwrap()["code"],
        "source_bytes_limit"
    );
    assert_eq!(
        call_code_success(&client, "return 'after-source-limit';").await?,
        "after-source-limit"
    );

    let timeout = call_code(&client, "while (true) {}").await?;
    assert_eq!(timeout.is_error, Some(true));
    let timeout_content = timeout.structured_content.unwrap();
    assert_eq!(
        timeout_content["code"], "deadline_exceeded",
        "unexpected timeout result: {timeout_content}"
    );
    let timeout_pid = last_pid(&pid_log)?;
    wait_for_pid_exit(timeout_pid).await?;
    assert_eq!(
        call_code_success(&client, "return 'after-timeout';").await?,
        "after-timeout"
    );
    wait_for_empty_directory(&scratch).await?;

    for (source, expected_code) in [
        ("/* wit-adversarial:crash */ return null;", "worker_exited"),
        (
            "/* wit-adversarial:malformed */ return null;",
            "worker_protocol_error",
        ),
    ] {
        let result = call_code(&client, source).await?;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content.unwrap()["code"], expected_code);
        wait_for_pid_exit(last_pid(&pid_log)?).await?;
        assert_eq!(
            call_code_success(&client, "return 'restarted';").await?,
            "restarted"
        );
        wait_for_empty_directory(&scratch).await?;
    }

    let request = ClientRequest::CallToolRequest(Request::new(
        CallToolRequestParams::new(CODE_TOOL).with_arguments(object!({
            "code": "while (true) {}"
        })),
    ));
    let running = client
        .send_cancellable_request(request, PeerRequestOptions::no_options())
        .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    running
        .cancel(Some("integration cancellation".to_string()))
        .await?;
    wait_for_pid_exit(last_pid(&pid_log)?).await?;
    assert_eq!(
        call_code_success(&client, "return 'after-cancel';").await?,
        "after-cancel"
    );
    wait_for_empty_directory(&scratch).await?;

    shutdown_client(client).await?;
    wait_for_empty_directory(&scratch).await
}

#[cfg(unix)]
#[tokio::test]
async fn shipped_code_cancellation_reaps_worker_and_git_processes() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let _serial = lock_stdio_child_tests().await;
    let temp = tempfile::tempdir()?;
    let cache = temp.path().join("cache");
    let scratch = temp.path().join("scratch");
    let fake_bin = temp.path().join("bin");
    std::fs::create_dir(&cache)?;
    std::fs::create_dir(&scratch)?;
    std::fs::create_dir(&fake_bin)?;
    let fixture = seed_cached_repo(&cache)?;
    let git_started = temp.path().join("git-started");
    let git_pid_log = temp.path().join("git-pid");
    let worker_pid_log = temp.path().join("worker-pids");
    let fake_git = fake_bin.join("git");
    std::fs::write(
        &fake_git,
        "#!/bin/sh\necho $$ > \"$WIT_TEST_GIT_PID\"\ntouch \"$WIT_TEST_GIT_STARTED\"\ntrap 'exit 143' TERM INT\nwhile :; do sleep 1; done\n",
    )?;
    std::fs::set_permissions(&fake_git, std::fs::Permissions::from_mode(0o755))?;
    let mut paths = vec![fake_bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(paths)?;
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(env!("CARGO_BIN_EXE_wit-mcp")).configure(|command| {
            command
                .args(["--mode", "code"])
                .env("PATH", path)
                .env("TMPDIR", &scratch)
                .env("WIT_CACHE_DIR", &cache)
                .env("GIT_CONFIG_GLOBAL", &fixture.git_config)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("WIT_CODEMODE_TEST_PID_LOG", &worker_pid_log)
                .env("WIT_TEST_GIT_STARTED", &git_started)
                .env("WIT_TEST_GIT_PID", &git_pid_log);
        }),
    )?;
    let client = ().serve(transport).await?;
    let request = ClientRequest::CallToolRequest(Request::new(
        CallToolRequestParams::new(CODE_TOOL).with_arguments(object!({
            "code": "return await codemode.wit.refs({ repo: 'owner/repo' });"
        })),
    ));
    let running = client
        .send_cancellable_request(request, PeerRequestOptions::no_options())
        .await?;
    wait_for_path(&git_started).await?;
    let worker_pid = last_pid(&worker_pid_log)?;
    let git_pid: u32 = std::fs::read_to_string(&git_pid_log)?.trim().parse()?;
    running.cancel(Some("cancel blocked git".into())).await?;
    wait_for_pid_exit(worker_pid).await?;
    wait_for_pid_exit(git_pid).await?;
    assert_eq!(
        call_code_success(&client, "return 'after-git-cancel';").await?,
        "after-git-cancel"
    );
    shutdown_client(client).await?;
    wait_for_empty_directory(&scratch).await
}

async fn wait_for_path(path: &Path) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if path.exists() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for {}", path.display());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn start_code_client(
    temp_root: &Path,
    fixture: Option<(&Path, &Path)>,
) -> anyhow::Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
    start_code_client_inner(temp_root, fixture, None).await
}

async fn start_code_client_with_pid_log(
    temp_root: &Path,
    fixture: Option<(&Path, &Path)>,
    pid_log: &Path,
) -> anyhow::Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
    start_code_client_inner(temp_root, fixture, Some(pid_log)).await
}

async fn start_code_client_inner(
    temp_root: &Path,
    fixture: Option<(&Path, &Path)>,
    pid_log: Option<&Path>,
) -> anyhow::Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
    let bin = env!("CARGO_BIN_EXE_wit-mcp");
    let transport =
        TokioChildProcess::new(tokio::process::Command::new(bin).configure(|command| {
            command
                .kill_on_drop(true)
                .args(["--mode", "code"])
                .env("TMPDIR", temp_root)
                .env("TMP", temp_root)
                .env("TEMP", temp_root)
                // If an insteadOf rewrite ever misses, git must fail fast,
                // not block on a credential prompt (hangs Windows CI).
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GCM_INTERACTIVE", "never");
            if let Some((cache, git_config)) = fixture {
                command
                    .env("WIT_CACHE_DIR", cache)
                    .env("GIT_CONFIG_GLOBAL", git_config)
                    .env("GIT_CONFIG_NOSYSTEM", "1");
            }
            if let Some(pid_log) = pid_log {
                command.env("WIT_CODEMODE_TEST_PID_LOG", pid_log);
            }
        }))?;
    Ok(().serve(transport).await?)
}

fn last_pid(path: &Path) -> anyhow::Result<u32> {
    std::fs::read_to_string(path)?
        .lines()
        .next_back()
        .ok_or_else(|| anyhow::anyhow!("worker PID log is empty"))?
        .parse()
        .map_err(Into::into)
}

async fn wait_for_pid_exit(pid: u32) -> anyhow::Result<()> {
    for _ in 0..80 {
        if !pid_is_alive(pid).await? {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    anyhow::bail!("Code Mode child PID {pid} is still alive")
}

#[cfg(unix)]
async fn pid_is_alive(pid: u32) -> anyhow::Result<bool> {
    Ok(tokio::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?
        .success())
}

#[cfg(windows)]
async fn pid_is_alive(pid: u32) -> anyhow::Result<bool> {
    let output = tokio::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")))
}

async fn call_code(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    source: &str,
) -> anyhow::Result<rmcp::model::CallToolResult> {
    Ok(client
        .call_tool(
            CallToolRequestParams::new(CODE_TOOL).with_arguments(object!({ "code": source })),
        )
        .await?)
}

async fn call_code_success(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    source: &str,
) -> anyhow::Result<Value> {
    let result = call_code(client, source).await?;
    if result.is_error == Some(true) {
        anyhow::bail!("code tool failed: {:?}", result.structured_content);
    }
    result
        .structured_content
        .ok_or_else(|| anyhow::anyhow!("code tool should return structured content"))
}

async fn wait_for_empty_directory(path: &Path) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if std::fs::read_dir(path)?.next().is_none() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            let entries = std::fs::read_dir(path)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<_>, _>>()?;
            anyhow::bail!("temporary Code Mode state leaked: {entries:?}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

struct CachedRepoFixture {
    _temp: tempfile::TempDir,
    git_config: PathBuf,
    worktree: PathBuf,
}

fn seed_cached_repo(cache_dir: &Path) -> anyhow::Result<CachedRepoFixture> {
    let temp = tempfile::tempdir()?;
    let worktree = temp.path().join("worktree");
    let remote = temp.path().join("remote.git");
    let branch_dir = cache_dir.join("owner/repo/branches/b-main");
    let repo_path = branch_dir.join("repo.git");

    run_git(&["init", worktree.to_str().unwrap()], None)?;
    run_git(&["checkout", "-b", "main"], Some(&worktree))?;
    std::fs::create_dir_all(worktree.join("src"))?;
    std::fs::write(worktree.join("README.md"), "alpha\nbeta\ngamma\n")?;
    std::fs::write(worktree.join("src/lib.rs"), "pub fn demo() {}\n")?;
    std::fs::write(
        worktree.join("medium.txt"),
        format!("{}\n", "x".repeat(1500)),
    )?;
    run_git(&["add", "."], Some(&worktree))?;
    commit(&worktree, "fixture")?;
    let sha = git_stdout(&["rev-parse", "HEAD"], Some(&worktree))?;

    run_git(&["init", "--bare", remote.to_str().unwrap()], None)?;
    run_git(
        &["remote", "add", "origin", remote.to_str().unwrap()],
        Some(&worktree),
    )?;
    run_git(&["push", "origin", "main"], Some(&worktree))?;
    run_git(&["symbolic-ref", "HEAD", "refs/heads/main"], Some(&remote))?;
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

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    std::fs::write(
        branch_dir.join("metadata.json"),
        serde_json::to_vec_pretty(&json!({
            "cache_schema_version": 1,
            "owner_repo": "owner/repo",
            "branch": "main",
            "remote_url": "https://github.com/owner/repo",
            "current_sha": sha,
            "last_checked_at": now,
            "last_updated_at": now
        }))?,
    )?;
    let git_config = temp.path().join("gitconfig");
    std::fs::write(
        &git_config,
        format!(
            "[url \"{}\"]\n\tinsteadOf = https://github.com/owner/repo\n\tinsteadOf = https://github.com/owner/repo.git\n",
            git_file_url(&remote)
        ),
    )?;
    Ok(CachedRepoFixture {
        _temp: temp,
        git_config,
        worktree,
    })
}

fn git_file_url(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        format!("file:///{path}")
    } else {
        format!("file://{path}")
    }
}

fn move_main_branch(fixture: &CachedRepoFixture, content: &str) -> anyhow::Result<String> {
    std::fs::write(fixture.worktree.join("README.md"), content)?;
    run_git(&["add", "README.md"], Some(&fixture.worktree))?;
    commit(&fixture.worktree, "move main")?;
    run_git(&["push", "origin", "main"], Some(&fixture.worktree))?;
    git_stdout(&["rev-parse", "HEAD"], Some(&fixture.worktree))
}

fn commit(worktree: &Path, message: &str) -> anyhow::Result<()> {
    run_git(
        &[
            "-c",
            "user.name=wit-test",
            "-c",
            "user.email=wit-test@example.com",
            "commit",
            "-m",
            message,
        ],
        Some(worktree),
    )
}

fn run_git(args: &[&str], workdir: Option<&Path>) -> anyhow::Result<()> {
    let output = git_command(args, workdir).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn git_stdout(args: &[&str], workdir: Option<&Path>) -> anyhow::Result<String> {
    let output = git_command(args, workdir).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn git_command(args: &[&str], workdir: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    command.args(args).stdin(Stdio::null());
    if let Some(workdir) = workdir {
        command.current_dir(workdir);
    }
    command
}
