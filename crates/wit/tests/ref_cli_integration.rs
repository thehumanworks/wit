//! `--ref` on CLI repo reads: branches, lightweight and annotated tags, full commit SHAs, and the
//! commit provenance line every disk read prints on stderr.

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    _temp: tempfile::TempDir,
    cache_dir: PathBuf,
    git_config: PathBuf,
    /// Commit tagged `v1.0` (annotated) and `light` (lightweight).
    v1_sha: String,
    /// Current tip of `main`.
    main_sha: String,
}

#[test]
fn ref_reads_default_branch_tags_commits_and_branches() -> anyhow::Result<()> {
    let fx = fixture()?;

    // No --ref keeps the default branch and reports which commit it served.
    let default = run_wit(&fx, &["cat", "owner/repo", "VERSION"])?;
    assert_eq!(stdout(&default), "2\n");
    assert_provenance(&default, "refs/heads/main", &fx.main_sha, "branch");

    // Annotated tags resolve to the commit they name, not the tag object.
    let annotated = run_wit(&fx, &["cat", "--ref", "v1.0", "owner/repo", "VERSION"])?;
    assert_eq!(stdout(&annotated), "1\n");
    assert_provenance(&annotated, "refs/tags/v1.0", &fx.v1_sha, "pinned");

    let qualified = run_wit(
        &fx,
        &["cat", "--ref", "refs/tags/v1.0", "owner/repo", "VERSION"],
    )?;
    assert_eq!(stdout(&qualified), "1\n");

    let lightweight = run_wit(&fx, &["cat", "--ref", "light", "owner/repo", "VERSION"])?;
    assert_eq!(stdout(&lightweight), "1\n");
    assert_provenance(&lightweight, "refs/tags/light", &fx.v1_sha, "pinned");

    // A full commit SHA reads exactly that commit, even after the branch moved past it.
    let pinned = run_wit(&fx, &["cat", "--ref", &fx.v1_sha, "owner/repo", "VERSION"])?;
    assert_eq!(stdout(&pinned), "1\n");
    assert_provenance(&pinned, &fx.v1_sha, &fx.v1_sha, "pinned");

    // Branches still go through the branch cache, and --branch stays an alias of --ref.
    for flag in ["--ref", "--branch"] {
        let branch = run_wit(&fx, &["cat", flag, "feature/x", "owner/repo", "VERSION"])?;
        assert_eq!(stdout(&branch), "feature\n");
        assert_provenance(&branch, "refs/heads/feature/x", "", "branch");
    }

    // Every repo-reading command accepts --ref.
    for args in [
        vec!["tree", "--ref", "v1.0", "owner/repo"],
        vec!["ls", "--ref", "v1.0", "owner/repo"],
        vec!["rg", "--ref", "v1.0", "1", "owner/repo"],
        vec!["head", "--ref", "v1.0", "owner/repo", "VERSION"],
        vec!["tail", "--ref", "v1.0", "owner/repo", "VERSION"],
        vec![
            "sed",
            "--ref",
            "v1.0",
            "-n",
            "-r",
            "owner/repo",
            "1p",
            "VERSION",
        ],
        vec!["ast", "symbols", "--ref", "v1.0", "owner/repo"],
        vec!["cache", "--ref", "v1.0", "owner/repo"],
    ] {
        let output = run_wit(&fx, &args)?;
        assert_provenance(&output, "refs/tags/v1.0", &fx.v1_sha, "pinned");
    }
    let tree = run_wit(&fx, &["tree", "--ref", "v1.0", "owner/repo"])?;
    assert!(stdout(&tree).contains("VERSION"));
    assert!(
        !stdout(&tree).contains("NEW_IN_2"),
        "tag read must not see later files"
    );
    Ok(())
}

#[test]
fn pinned_reads_are_served_from_cache_without_the_remote() -> anyhow::Result<()> {
    let fx = fixture()?;
    let first = run_wit(&fx, &["cat", "--ref", &fx.v1_sha, "owner/repo", "VERSION"])?;
    assert_eq!(stdout(&first), "1\n");

    // Point the remote somewhere that does not exist: a warm commit entry needs no network.
    let broken = fx.git_config.with_file_name("broken-gitconfig");
    std::fs::write(
        &broken,
        "[url \"file:///nonexistent/remote.git\"]\n\tinsteadOf = https://github.com/owner/repo\n",
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_wit"))
        .args(["cat", "--ref", &fx.v1_sha, "owner/repo", "VERSION"])
        .env("WIT_CACHE_DIR", &fx.cache_dir)
        .env("WIT_CACHE_URL", "off")
        .env("GIT_CONFIG_GLOBAL", &broken)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(stdout(&output), "1\n");
    Ok(())
}

#[test]
fn unknown_ref_fails_with_guidance() -> anyhow::Result<()> {
    let fx = fixture()?;
    let output = wit_command(&fx, &["cat", "--ref", "v9.9", "owner/repo", "VERSION"]).output()?;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ref 'v9.9' was not found as a branch or tag"),
        "{stderr}"
    );
    assert!(stderr.contains("full 40-character commit SHA"), "{stderr}");
    Ok(())
}

/// Agents pass `owner/repo/PATH` as one argument, add `-n` to rg, and read long files with cat.
#[test]
fn agent_shaped_reads_work() -> anyhow::Result<()> {
    let fx = fixture()?;

    let combined = run_wit(&fx, &["cat", "owner/repo/src/lines.txt"])?;
    assert_eq!(stdout(&combined), "line1\nline2\nline3\nline4\nline5\n");

    let ranged = run_wit(
        &fx,
        &[
            "sed",
            "-n",
            "2,3p",
            "owner/repo/src/lines.txt",
            "--ref",
            "main",
        ],
    )?;
    assert_eq!(stdout(&ranged), "line2\nline3\n");

    let scoped = run_wit(&fx, &["rg", "-n", "line3", "owner/repo/src"])?;
    assert!(
        stdout(&scoped).contains("src/lines.txt:3:line3"),
        "{}",
        stdout(&scoped)
    );

    let flagged = run_wit(
        &fx,
        &[
            "head",
            "-n",
            "1",
            "-r",
            "owner/repo",
            "owner/repo/src/lines.txt",
        ],
    )?;
    assert_eq!(stdout(&flagged), "line1\n");

    let capped = wit_command(&fx, &["cat", "owner/repo", "src/lines.txt"])
        .env("WIT_CAT_MAX_LINES", "2")
        .output()?;
    assert!(capped.status.success());
    let text = stdout(&capped);
    assert!(
        text.starts_with("line1\nline2\n[wit cat: printed lines 1-2 of 5."),
        "{text}"
    );
    assert!(
        text.contains("`wit sed -n '3,4p' owner/repo src/lines.txt`"),
        "{text}"
    );
    assert!(!text.contains("line3"), "{text}");

    let full = wit_command(&fx, &["cat", "--full", "owner/repo", "src/lines.txt"])
        .env("WIT_CAT_MAX_LINES", "2")
        .output()?;
    assert_eq!(stdout(&full), "line1\nline2\nline3\nline4\nline5\n");
    Ok(())
}

/// Errors stay one readable message even when the environment asks Rust for backtraces.
#[test]
fn errors_never_print_backtraces() -> anyhow::Result<()> {
    let fx = fixture()?;
    let output = wit_command(&fx, &["cat", "--ref", "v9.9", "owner/repo", "VERSION"])
        .env("RUST_BACKTRACE", "1")
        .env("RUST_LIB_BACKTRACE", "1")
        .output()?;
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("Error: ref 'v9.9' was not found"),
        "{stderr}"
    );
    assert!(!stderr.to_lowercase().contains("backtrace"), "{stderr}");
    Ok(())
}

/// A wrong path names the real ones, a missing directory fails instead of listing nothing,
/// and ripgrep-style `-t`, `wit --ref REF CMD`, and `wit grep` work or explain themselves.
#[test]
fn wrong_paths_and_flag_shapes_get_actionable_answers() -> anyhow::Result<()> {
    let fx = fixture()?;

    let missing = wit_command(&fx, &["cat", "owner/repo", "lines.txt"]).output()?;
    assert!(!missing.status.success());
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(
        stderr.contains("File not found: lines.txt. Did you mean: src/lines.txt?"),
        "{stderr}"
    );

    let dir = wit_command(&fx, &["tree", "owner/repo", "lib"]).output()?;
    assert!(!dir.status.success());
    let stderr = String::from_utf8_lossy(&dir.stderr);
    assert!(stderr.contains("Directory not found: lib."), "{stderr}");

    let typed = run_wit(&fx, &["rg", "-t", "txt", "line2", "owner/repo"]);
    assert!(typed.is_err(), "txt is not a known type");
    let typed = run_wit(&fx, &["rg", "-g", "*.txt", "line2", "owner/repo"])?;
    assert!(stdout(&typed).contains("src/lines.txt:2:line2"));

    let global_ref = run_wit(&fx, &["--ref", "v1.0", "cat", "owner/repo", "VERSION"])?;
    assert_eq!(stdout(&global_ref), "1\n");

    let grep = wit_command(&fx, &["grep", "-rn", "line2", "owner/repo"]).output()?;
    assert!(!grep.status.success());
    assert!(String::from_utf8_lossy(&grep.stderr).contains("wit rg PATTERN"));
    Ok(())
}

fn assert_provenance(output: &Output, resolved_ref: &str, commit: &str, cache: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stderr
        .lines()
        .find(|line| line.starts_with("snapshot: backend=disk "))
        .unwrap_or_else(|| panic!("missing provenance line in stderr: {stderr}"));
    assert!(line.contains("repo=owner/repo"), "{line}");
    assert!(line.contains(&format!(" ref={resolved_ref} ")), "{line}");
    assert!(line.ends_with(&format!(" cache={cache}")), "{line}");
    if !commit.is_empty() {
        assert!(line.contains(&format!(" commit={commit} ")), "{line}");
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn run_wit(fx: &Fixture, args: &[&str]) -> anyhow::Result<Output> {
    let output = wit_command(fx, args).output()?;
    if !output.status.success() {
        anyhow::bail!(
            "wit {:?} failed with status {} stdout: {} stderr: {}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output)
}

fn wit_command(fx: &Fixture, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wit"));
    command
        .args(args)
        .env("WIT_CACHE_DIR", &fx.cache_dir)
        .env("WIT_CACHE_URL", "off")
        .env("GIT_CONFIG_GLOBAL", &fx.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1");
    command
}

fn fixture() -> anyhow::Result<Fixture> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let worktree = root.join("worktree");
    let remote = root.join("remote.git");

    run_git(&["init", worktree.to_str().unwrap()], None)?;
    run_git(&["checkout", "-b", "main"], Some(&worktree))?;
    std::fs::write(worktree.join("VERSION"), "1\n")?;
    commit_all(&worktree, "v1")?;
    let v1_sha = git_stdout(&["rev-parse", "HEAD"], &worktree)?;
    run_git(
        &[
            "-c",
            "user.name=wit-test",
            "-c",
            "user.email=wit-test@example.com",
            "tag",
            "-a",
            "v1.0",
            "-m",
            "release 1.0",
        ],
        Some(&worktree),
    )?;
    run_git(&["tag", "light"], Some(&worktree))?;

    std::fs::write(worktree.join("VERSION"), "2\n")?;
    std::fs::write(worktree.join("NEW_IN_2"), "later\n")?;
    std::fs::create_dir_all(worktree.join("src"))?;
    std::fs::write(
        worktree.join("src/lines.txt"),
        "line1\nline2\nline3\nline4\nline5\n",
    )?;
    commit_all(&worktree, "v2")?;
    let main_sha = git_stdout(&["rev-parse", "HEAD"], &worktree)?;

    run_git(&["checkout", "-b", "feature/x"], Some(&worktree))?;
    std::fs::write(worktree.join("VERSION"), "feature\n")?;
    commit_all(&worktree, "feature")?;

    run_git(&["init", "--bare", remote.to_str().unwrap()], None)?;
    run_git(
        &[
            "push",
            remote.to_str().unwrap(),
            "main",
            "feature/x",
            "--tags",
        ],
        Some(&worktree),
    )?;
    run_git(&["symbolic-ref", "HEAD", "refs/heads/main"], Some(&remote))?;

    let git_config = root.join("gitconfig");
    std::fs::write(
        &git_config,
        format!(
            "[url \"file://{}\"]\n\tinsteadOf = https://github.com/owner/repo\n",
            remote.display()
        ),
    )?;

    Ok(Fixture {
        cache_dir: root.join("cache"),
        git_config,
        v1_sha,
        main_sha,
        _temp: temp,
    })
}

fn commit_all(worktree: &Path, message: &str) -> anyhow::Result<()> {
    run_git(&["add", "."], Some(worktree))?;
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

fn git_stdout(args: &[&str], workdir: &Path) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workdir)
        .output()?;
    anyhow::ensure!(output.status.success(), "git {args:?} failed");
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn run_git(args: &[&str], workdir: Option<&Path>) -> anyhow::Result<()> {
    let mut command = Command::new("git");
    command.args(args);
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
    Ok(())
}
