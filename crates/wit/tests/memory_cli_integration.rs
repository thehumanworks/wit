//! Memory-backend CLI parity: newly mapped commands must not write WIT_CACHE_DIR.
//!
//! Live against a public GitHub repo (octocat/Hello-World). Marked `ignore` so
//! default CI stays offline-friendly; run with `--ignored` when network is available.

use std::{
    path::Path,
    process::{Command, Output},
};

fn wit_bin() -> &'static str {
    env!("CARGO_BIN_EXE_wit")
}

fn run_wit(cache_dir: &Path, args: &[&str]) -> anyhow::Result<Output> {
    let output = Command::new(wit_bin())
        .env("WIT_CACHE_DIR", cache_dir)
        .env_remove("WIT_SNAPSHOT_BACKEND")
        .args(args)
        .output()?;
    Ok(output)
}

fn assert_success(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed (status {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_cache_empty(cache_dir: &Path) {
    let mut entries = std::fs::read_dir(cache_dir)
        .unwrap_or_else(|err| panic!("read cache probe {}: {err}", cache_dir.display()));
    assert!(
        entries.next().is_none(),
        "memory backend must leave WIT_CACHE_DIR empty; found entries under {}",
        cache_dir.display()
    );
}

#[test]
#[ignore = "live GitHub; run with --ignored"]
fn memory_backend_cli_commands_leave_cache_empty() -> anyhow::Result<()> {
    let probe = tempfile::tempdir()?;
    let cache = probe.path();
    let repo = "octocat/Hello-World";

    let tree = run_wit(cache, &["tree", "-r", repo, "--backend", "memory"])?;
    assert_success(&tree, "tree");
    assert!(String::from_utf8_lossy(&tree.stdout).contains("README"));

    let ls = run_wit(cache, &["ls", "-r", repo, "--backend", "memory"])?;
    assert_success(&ls, "ls");

    let cat = run_wit(cache, &["cat", "-r", repo, "README", "--backend", "memory"])?;
    assert_success(&cat, "cat");
    assert!(String::from_utf8_lossy(&cat.stdout).contains("Hello"));

    let rg = run_wit(cache, &["rg", "Hello", "-r", repo, "--backend", "memory"])?;
    assert_success(&rg, "rg");
    assert!(String::from_utf8_lossy(&rg.stdout).contains("Hello"));

    let head = run_wit(
        cache,
        &[
            "head",
            "-n",
            "3",
            "-r",
            repo,
            "README",
            "--backend",
            "memory",
        ],
    )?;
    assert_success(&head, "head");

    let tail = run_wit(
        cache,
        &[
            "tail",
            "-n",
            "3",
            "-r",
            repo,
            "README",
            "--backend",
            "memory",
        ],
    )?;
    assert_success(&tail, "tail");

    let sed = run_wit(
        cache,
        &["sed", "-n", "1,5p", repo, "README", "--backend", "memory"],
    )?;
    assert_success(&sed, "sed trailing --backend");
    assert!(
        !String::from_utf8_lossy(&sed.stdout).is_empty(),
        "sed trailing --backend should print lines"
    );

    // Flag-first form remains supported
    let sed_flag_first = run_wit(
        cache,
        &[
            "sed",
            "--backend",
            "memory",
            "-e",
            "s/Hello/Hi/",
            repo,
            "README",
        ],
    )?;
    assert_success(&sed_flag_first, "sed flag-first --backend");
    assert!(String::from_utf8_lossy(&sed_flag_first.stdout).contains("Hi"));

    let cache_cmd = run_wit(cache, &["cache", "-r", repo, "--backend", "memory"])?;
    assert_success(&cache_cmd, "cache");
    assert!(
        String::from_utf8_lossy(&cache_cmd.stdout).contains("Pinned memory snapshot"),
        "cache memory path should pin, not clone"
    );

    let branches = run_wit(cache, &["branches", "-r", repo, "--backend", "memory"])?;
    assert_success(&branches, "branches");
    assert!(
        String::from_utf8_lossy(&branches.stdout).contains("master")
            || String::from_utf8_lossy(&branches.stdout).contains("main")
    );

    // search is GitHub API only — still must not touch the memory probe dir
    let search = run_wit(cache, &["search", "-p", "Hello-World", "--limit", "1"])?;
    assert_success(&search, "search");

    assert_cache_empty(cache);
    Ok(())
}

#[test]
fn tree_positional_before_repo_flag_conflicts_before_backend() {
    let probe = tempfile::tempdir().unwrap();
    let output = run_wit(
        probe.path(),
        &[
            "tree",
            "other/repo",
            "-r",
            "octocat/Hello-World",
            "--backend",
            "memory",
        ],
    )
    .expect("wit tree should spawn");
    assert!(
        !output.status.success(),
        "disagreeing positional/-r must fail before backend"
    );
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        err.contains("conflicting repository arguments"),
        "expected conflict error, got:\n{err}"
    );
    assert!(
        !err.to_lowercase().contains("path not found"),
        "must not treat positional repo as a path:\n{err}"
    );
}

#[test]
fn help_documents_memory_rg_sed_head_tail() {
    let output = Command::new(wit_bin())
        .args(["--help"])
        .output()
        .expect("wit --help");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.contains("Memory covers tree/ls/cat/rg/sed/head/tail")
            || help.contains("tree/ls/cat/rg/sed/head/tail"),
        "wit --help should document memory rg/sed/head/tail coverage"
    );
    assert!(
        !help.to_lowercase().contains("does not cover")
            && !help.to_lowercase().contains("lacks rg"),
        "wit --help must not claim memory lacks rg/sed/head/tail"
    );
}
