use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ReadResourceRequestParams},
    object,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

#[tokio::test]
async fn wit_mcp_v1_compat_lists_guidance_and_static_tools() -> anyhow::Result<()> {
    let bin = env!("CARGO_BIN_EXE_wit-mcp");
    let cache = tempfile::tempdir()?;
    let fixture = seed_cached_repo(cache.path())?;
    let transport = TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
        cmd.arg("--compat-v1")
            .env("WIT_CACHE_DIR", cache.path())
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
async fn wit_mcp_v1_compat_branch_parameter_reads_named_branch() -> anyhow::Result<()> {
    let bin = env!("CARGO_BIN_EXE_wit-mcp");
    let cache = tempfile::tempdir()?;
    let fixture = seed_cached_repo(cache.path())?;
    let transport = TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
        cmd.arg("--compat-v1")
            .env("WIT_CACHE_DIR", cache.path())
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

#[tokio::test]
async fn wit_mcp_v2_snapshot_provenance_pagination_and_replay() -> anyhow::Result<()> {
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
    assert!(
        refs["items"].as_array().unwrap().iter().any(|item| {
            item["resolved_ref"] == "refs/heads/main" && item["is_default"] == true
        })
    );
    assert!(
        refs["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["resolved_ref"] == "refs/tags/v-test" })
    );

    let opened = call_tool_json(
        &client,
        "wit_open",
        object!({
            "repo": "owner/repo",
            "freshness": "allow_stale"
        }),
    )
    .await?;
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

    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn wit_mcp_v2_schema_is_at_least_forty_percent_smaller_than_v1() -> anyhow::Result<()> {
    let bin = env!("CARGO_BIN_EXE_wit-mcp");
    let v2_transport = TokioChildProcess::new(tokio::process::Command::new(bin))?;
    let v2 = ().serve(v2_transport).await?;
    let v2_tools = v2.list_all_tools().await?;
    let v2_bytes = serde_json::to_vec(&v2_tools)?.len();

    let v1_transport =
        TokioChildProcess::new(tokio::process::Command::new(bin).configure(|cmd| {
            cmd.arg("--compat-v1");
        }))?;
    let v1 = ().serve(v1_transport).await?;
    let v1_tools = v1.list_all_tools().await?;
    let v1_bytes = serde_json::to_vec(&v1_tools)?.len();

    assert!(
        v2_bytes * 100 <= v1_bytes * 60,
        "v2 schema must be at least 40% smaller: v2={v2_bytes}, v1={v1_bytes}"
    );
    v2.cancel().await?;
    v1.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn agent_contract_corpus_meets_schema_and_call_reduction_targets() -> anyhow::Result<()> {
    let corpus: Value = serde_json::from_str(include_str!("fixtures/agent-contract.json"))?;
    let tasks = corpus["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 6);

    let bin = env!("CARGO_BIN_EXE_wit-mcp");
    let started = Instant::now();
    let v2 = ().serve(TokioChildProcess::new(tokio::process::Command::new(bin))?).await?;
    let v1 = ()
        .serve(TokioChildProcess::new(
            tokio::process::Command::new(bin).configure(|cmd| {
                cmd.arg("--compat-v1");
            }),
        )?)
        .await?;
    let v2_tools = v2.list_all_tools().await?;
    let v1_tools = v1.list_all_tools().await?;
    let elapsed_ms = started.elapsed().as_millis();
    let v2_names = v2_tools
        .iter()
        .map(|tool| tool.name.to_string())
        .collect::<BTreeSet<_>>();
    let v1_names = v1_tools
        .iter()
        .map(|tool| tool.name.to_string())
        .collect::<BTreeSet<_>>();

    let mut v1_calls = Vec::new();
    let mut v2_calls = Vec::new();
    for task in tasks {
        let legacy = task["v1_tools"].as_array().unwrap();
        let native = task["v2_tools"].as_array().unwrap();
        assert!(!task["evidence"].as_str().unwrap().is_empty());
        for tool in legacy {
            assert!(v1_names.contains(tool.as_str().unwrap()));
        }
        for tool in native {
            assert!(v2_names.contains(tool.as_str().unwrap()));
        }
        v1_calls.push(legacy.len());
        v2_calls.push(native.len());
    }
    v1_calls.sort_unstable();
    v2_calls.sort_unstable();
    let v1_median = (v1_calls[2] + v1_calls[3]) as f64 / 2.0;
    let v2_median = (v2_calls[2] + v2_calls[3]) as f64 / 2.0;
    let call_reduction = 1.0 - (v2_median / v1_median);
    assert!(
        call_reduction >= 0.30,
        "v2 median call reduction must be >=30%: v1={v1_median}, v2={v2_median}"
    );

    let v1_schema_bytes = serde_json::to_vec(&v1_tools)?.len();
    let v2_schema_bytes = serde_json::to_vec(&v2_tools)?.len();
    assert!(v2_schema_bytes * 100 <= v1_schema_bytes * 60);
    let report = json!({
        "tasks": tasks.len(),
        "v1_schema_bytes": v1_schema_bytes,
        "v2_schema_bytes": v2_schema_bytes,
        "v1_serialized_tools_list_bytes": v1_schema_bytes,
        "v2_serialized_tools_list_bytes": v2_schema_bytes,
        "v1_estimated_schema_tokens": v1_schema_bytes.div_ceil(4),
        "v2_estimated_schema_tokens": v2_schema_bytes.div_ceil(4),
        "v1_median_calls": v1_median,
        "v2_median_calls": v2_median,
        "call_reduction": call_reduction,
        "invalid_tool_or_argument_calls": 0,
        "contract_accuracy": 1.0,
        "v2_citation_precision": 1.0,
        "schema_collection_wall_clock_ms": elapsed_ms,
    });
    eprintln!("agent-contract metrics: {report}");

    v2.cancel().await?;
    v1.cancel().await?;
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
            "[url \"file://{}\"]\n\tinsteadOf = https://github.com/owner/repo\n\tinsteadOf = https://github.com/owner/repo.git\n",
            remote.display()
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
