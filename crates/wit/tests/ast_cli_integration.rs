//! `wit ast symbols|query` against a seeded disk cache (no network).

use serde_json::json;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn wit_bin() -> &'static str {
    env!("CARGO_BIN_EXE_wit")
}

fn run_git(args: &[&str], workdir: Option<&Path>) -> anyhow::Result<String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", std::env::temp_dir());
    if let Some(dir) = workdir {
        command.current_dir(dir);
    }
    let output = command.output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

struct Fixture {
    _temp: tempfile::TempDir,
    cache_dir: PathBuf,
    git_config: PathBuf,
}

/// Seed `WIT_CACHE_DIR` with owner/repo@main so `wit` never touches GitHub.
fn seed() -> anyhow::Result<Fixture> {
    let temp = tempfile::tempdir()?;
    let worktree = temp.path().join("worktree");
    let remote = temp.path().join("remote.git");
    let cache_dir = temp.path().join("cache");
    let branch_dir = cache_dir
        .join("owner")
        .join("repo")
        .join("branches")
        .join("b-main");

    run_git(&["init", worktree.to_str().unwrap()], None)?;
    run_git(&["checkout", "-b", "main"], Some(&worktree))?;
    std::fs::create_dir_all(worktree.join("src"))?;
    std::fs::write(
        worktree.join("src").join("lib.rs"),
        "pub struct Widget {\n    name: String,\n}\n\nimpl Widget {\n    pub fn new(name: &str) -> Self {\n        Self { name: name.into() }\n    }\n\n    fn render(&self) -> String {\n        helper(&self.name)\n    }\n}\n\nfn helper(s: &str) -> String {\n    s.to_string()\n}\n",
    )?;
    std::fs::write(
        worktree.join("app.py"),
        "class Runner:\n    def run(self):\n        return helper()\n\n\ndef helper():\n    return 1\n",
    )?;
    std::fs::write(worktree.join("README.md"), "# fixture\n")?;
    std::fs::write(worktree.join("logo.png"), [0x89u8, 0x50, 0x00, 0x47])?;
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
    let sha = run_git(&["rev-parse", "HEAD"], Some(&worktree))?;
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
            branch_dir.join("repo.git").to_str().unwrap(),
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
    let remote_url = remote.display().to_string().replace('\\', "/");
    let remote_url = if remote_url.starts_with('/') {
        format!("file://{remote_url}")
    } else {
        format!("file:///{remote_url}")
    };
    let git_config = temp.path().join("gitconfig");
    std::fs::write(
        &git_config,
        format!(
            "[url \"{remote_url}\"]\n\tinsteadOf = https://github.com/owner/repo\n\tinsteadOf = https://github.com/owner/repo.git\n"
        ),
    )?;
    Ok(Fixture {
        _temp: temp,
        cache_dir,
        git_config,
    })
}

fn wit(fixture: &Fixture, args: &[&str]) -> anyhow::Result<Output> {
    Ok(Command::new(wit_bin())
        .env("WIT_CACHE_DIR", &fixture.cache_dir)
        .env("GIT_CONFIG_GLOBAL", &fixture.git_config)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("WIT_SNAPSHOT_BACKEND")
        .args(args)
        .output()?)
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "wit failed (status {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn ast_symbols_lists_definitions_with_ranges_and_nesting() -> anyhow::Result<()> {
    let fixture = seed()?;
    let out = stdout(&wit(
        &fixture,
        &["ast", "symbols", "owner/repo", "src/lib.rs"],
    )?);
    assert_eq!(
        out,
        "src/lib.rs (rust, 17 lines)\n   1-3   struct Widget\n   5-13  impl Widget\n   6-8     fn new\n  10-12    fn render\n  15-17  fn helper\n"
    );

    let whole_repo = stdout(&wit(&fixture, &["ast", "symbols", "-r", "owner/repo"])?);
    assert!(whole_repo.contains("app.py (python, 7 lines)"));
    assert!(whole_repo.contains("  1-3  class Runner\n  2-3    def run\n  6-7  def helper"));
    assert!(whole_repo.contains("src/lib.rs (rust, 17 lines)"));
    assert!(
        !whole_repo.contains("README"),
        "unsupported files are skipped silently"
    );
    assert!(!whole_repo.contains("logo.png"), "binary files are skipped");

    let filtered = stdout(&wit(
        &fixture,
        &[
            "ast",
            "symbols",
            "-r",
            "owner/repo",
            "--kind",
            "fn",
            "--name",
            "^helper$",
            "--glob",
            "*.rs",
        ],
    )?);
    assert_eq!(
        filtered,
        "src/lib.rs (rust, 17 lines)\n  15-17  fn helper\n"
    );

    let by_lang = stdout(&wit(
        &fixture,
        &["ast", "symbols", "owner/repo", "--lang", "python"],
    )?);
    assert!(by_lang.starts_with("app.py (python, 7 lines)"));
    assert!(!by_lang.contains("lib.rs"));
    Ok(())
}

#[test]
fn ast_symbols_json_carries_positions_and_parents() -> anyhow::Result<()> {
    let fixture = seed()?;
    let out = stdout(&wit(
        &fixture,
        &["ast", "symbols", "owner/repo", "src/lib.rs", "--json"],
    )?);
    let reports: serde_json::Value = serde_json::from_str(&out)?;
    let file = &reports[0];
    assert_eq!(file["path"], "src/lib.rs");
    assert_eq!(file["language"], "rust");
    assert_eq!(file["total_lines"], 17);
    let symbols = file["symbols"].as_array().unwrap();
    assert_eq!(symbols.len(), 5);
    let render = symbols.iter().find(|s| s["name"] == "render").unwrap();
    assert_eq!(render["kind"], "fn");
    assert_eq!(render["parent"], "Widget");
    assert_eq!(render["depth"], 1);
    assert_eq!(render["start_line"], 10);
    assert_eq!(render["end_line"], 12);
    assert_eq!(render["signature"], "fn render(&self) -> String {");
    Ok(())
}

#[test]
fn ast_query_runs_tree_sitter_queries_across_files() -> anyhow::Result<()> {
    let fixture = seed()?;
    let query = "(call_expression function: (identifier) @callee (#eq? @callee \"helper\"))";
    let out = stdout(&wit(
        &fixture,
        &["ast", "query", query, "owner/repo", "src", "--lang", "rust"],
    )?);
    assert_eq!(out, "src/lib.rs:11:9: @callee (identifier) helper\n");

    // A single-file PATH infers the language.
    let py = stdout(&wit(
        &fixture,
        &[
            "ast",
            "query",
            "(function_definition name: (identifier) @name)",
            "owner/repo",
            "app.py",
            "--json",
        ],
    )?);
    let reports: serde_json::Value = serde_json::from_str(&py)?;
    let captures = reports[0]["captures"].as_array().unwrap();
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0]["text"], "run");
    assert_eq!(captures[1]["text"], "helper");
    assert_eq!(captures[1]["start_line"], 6);

    let missing_lang = wit(
        &fixture,
        &["ast", "query", "(function_item) @f", "owner/repo", "src"],
    )?;
    assert!(!missing_lang.status.success());
    assert!(String::from_utf8_lossy(&missing_lang.stderr).contains("--lang"));

    let bad_query = wit(
        &fixture,
        &["ast", "query", "(nope) @f", "owner/repo", "--lang", "rust"],
    )?;
    assert!(!bad_query.status.success());
    assert!(String::from_utf8_lossy(&bad_query.stderr).contains("invalid tree-sitter query"));
    Ok(())
}
