use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[test]
fn branches_cli_lists_metadata_and_listed_branch_reads() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let cache_dir = temp.path().join("cache");
    let fixture = create_branch_metadata_fixture(temp.path())?;
    let git_config = write_url_rewrite_config(temp.path(), &fixture.remote)?;
    let wit_bin = env!("CARGO_BIN_EXE_wit");

    let output = run_wit(
        wit_bin,
        &cache_dir,
        &git_config,
        &["branches", "-r", "owner/repo"],
    )?;

    assert!(output.contains("BRANCH"));
    assert!(output.contains("DEFAULT"));
    assert!(output.contains("TIP"));
    assert!(output.contains("AHEAD"));
    assert!(output.contains("BEHIND"));
    assert!(output.contains("MERGED"));
    assert!(output.contains("CREATED"));
    assert!(output.contains("CREATED_SOURCE"));
    assert!(output.contains("AUTHOR"));

    for branch in ["main", "behind-only", "feature/active", "feature/merged"] {
        assert_eq!(
            branch_occurrences(&output, branch),
            1,
            "{branch} should appear exactly once in:\n{output}"
        );
    }

    assert_branch_columns(
        line_for_branch(&output, "main"),
        "main",
        "*",
        &fixture.main_sha,
        "0",
        "0",
        "yes",
    );
    assert!(line_for_branch(&output, "main").contains("Main Author <main@example.com>"));
    assert!(
        line_for_branch(&output, "main").contains("2024-01-05T00:00:00Z")
            || line_for_branch(&output, "main").contains("2024-01-05T00:00:00+00:00")
    );
    assert!(line_for_branch(&output, "main").contains("tip commit fallback"));

    assert_branch_columns(
        line_for_branch(&output, "feature/active"),
        "feature/active",
        "-",
        &fixture.active_sha,
        "1",
        "3",
        "no",
    );
    assert!(line_for_branch(&output, "feature/active").contains("first unique commit"));
    assert!(
        line_for_branch(&output, "feature/active").contains("Feature Author <feature@example.com>")
    );

    assert_branch_columns(
        line_for_branch(&output, "behind-only"),
        "behind-only",
        "-",
        &fixture.base_sha,
        "0",
        "3",
        "yes",
    );
    assert!(line_for_branch(&output, "behind-only").contains("tip commit fallback"));

    assert_branch_columns(
        line_for_branch(&output, "feature/merged"),
        "feature/merged",
        "-",
        &fixture.merged_sha,
        "0",
        "2",
        "yes",
    );
    assert!(
        line_for_branch(&output, "feature/merged").contains("Merged Author <merged@example.com>")
    );

    let active_file = run_wit(
        wit_bin,
        &cache_dir,
        &git_config,
        &[
            "cat",
            "--branch",
            "feature/active",
            "-r",
            "owner/repo",
            "active.txt",
        ],
    )?;
    assert!(active_file.contains("active branch marker"));

    Ok(())
}

fn assert_branch_columns(
    line: &str,
    name: &str,
    default_marker: &str,
    sha: &str,
    ahead: &str,
    behind: &str,
    merged: &str,
) {
    let columns: Vec<&str> = line.split_whitespace().collect();
    assert!(
        columns.len() >= 8,
        "branch line should include required columns: {line}"
    );
    assert_eq!(columns[0], name);
    assert_eq!(columns[1], default_marker);
    assert_eq!(columns[2], sha);
    assert_eq!(columns[3], ahead);
    assert_eq!(columns[4], behind);
    assert_eq!(columns[5], merged);
}

fn branch_occurrences(output: &str, branch: &str) -> usize {
    output
        .lines()
        .filter(|line| line.split_whitespace().next() == Some(branch))
        .count()
}

fn line_for_branch<'a>(output: &'a str, branch: &str) -> &'a str {
    output
        .lines()
        .find(|line| line.split_whitespace().next() == Some(branch))
        .unwrap_or_else(|| panic!("missing branch {branch} in:\n{output}"))
}

struct BranchMetadataFixture {
    remote: PathBuf,
    base_sha: String,
    active_sha: String,
    merged_sha: String,
    main_sha: String,
}

fn create_branch_metadata_fixture(root: &Path) -> anyhow::Result<BranchMetadataFixture> {
    let worktree = root.join("worktree");
    let remote = root.join("remote.git");

    run_git(&["init", worktree.to_str().unwrap()], None)?;
    run_git(&["checkout", "-b", "main"], Some(&worktree))?;
    std::fs::write(worktree.join("README.md"), "main base\n")?;
    let base_sha = commit_all_with_author(
        &worktree,
        "main base",
        "Main Author",
        "main@example.com",
        "2024-01-01T00:00:00 +0000",
    )?;

    run_git(&["init", "--bare", remote.to_str().unwrap()], None)?;
    run_git(
        &["remote", "add", "origin", remote.to_str().unwrap()],
        Some(&worktree),
    )?;
    run_git(&["push", "origin", "main"], Some(&worktree))?;
    run_git(&["symbolic-ref", "HEAD", "refs/heads/main"], Some(&remote))?;
    run_git(&["branch", "behind-only", &base_sha], Some(&worktree))?;
    run_git(&["push", "origin", "behind-only"], Some(&worktree))?;

    run_git(&["checkout", "-b", "feature/active"], Some(&worktree))?;
    std::fs::write(worktree.join("active.txt"), "active branch marker\n")?;
    let active_sha = commit_all_with_author(
        &worktree,
        "active branch",
        "Feature Author",
        "feature@example.com",
        "2024-01-02T00:00:00 +0000",
    )?;
    run_git(&["push", "origin", "feature/active"], Some(&worktree))?;

    run_git(&["checkout", "main"], Some(&worktree))?;
    run_git(&["checkout", "-b", "feature/merged"], Some(&worktree))?;
    std::fs::write(worktree.join("merged.txt"), "merged branch marker\n")?;
    let merged_sha = commit_all_with_author(
        &worktree,
        "merged branch",
        "Merged Author",
        "merged@example.com",
        "2024-01-03T00:00:00 +0000",
    )?;
    run_git(&["push", "origin", "feature/merged"], Some(&worktree))?;

    run_git(&["checkout", "main"], Some(&worktree))?;
    run_git_with_author(
        &[
            "-c",
            "user.name=Main Author",
            "-c",
            "user.email=main@example.com",
            "merge",
            "--no-ff",
            "feature/merged",
            "-m",
            "merge feature",
        ],
        Some(&worktree),
        "2024-01-04T00:00:00 +0000",
    )?;
    std::fs::write(worktree.join("README.md"), "main advanced\n")?;
    let main_sha = commit_all_with_author(
        &worktree,
        "main advance",
        "Main Author",
        "main@example.com",
        "2024-01-05T00:00:00 +0000",
    )?;
    run_git(&["push", "origin", "main"], Some(&worktree))?;

    Ok(BranchMetadataFixture {
        remote,
        base_sha,
        active_sha,
        merged_sha,
        main_sha,
    })
}

fn run_wit(
    wit_bin: &str,
    cache_dir: &Path,
    git_config: &Path,
    args: &[&str],
) -> anyhow::Result<String> {
    let output = Command::new(wit_bin)
        .args(args)
        .env("WIT_CACHE_DIR", cache_dir)
        .env("GIT_CONFIG_GLOBAL", git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "wit {:?} failed with status {} stdout: {} stderr: {}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn commit_all_with_author(
    worktree: &Path,
    message: &str,
    name: &str,
    email: &str,
    date: &str,
) -> anyhow::Result<String> {
    run_git(&["add", "."], Some(worktree))?;
    run_git_with_author(
        &[
            "-c",
            &format!("user.name={name}"),
            "-c",
            &format!("user.email={email}"),
            "commit",
            "-m",
            message,
        ],
        Some(worktree),
        date,
    )?;
    git_stdout(&["rev-parse", "HEAD"], Some(worktree))
}

fn write_url_rewrite_config(root: &Path, remote: &Path) -> anyhow::Result<PathBuf> {
    let git_config = root.join("gitconfig");
    let remote_url = format!("file://{}", remote.display());
    std::fs::write(
        &git_config,
        format!(
            "[url \"{remote_url}\"]\n\tinsteadOf = https://github.com/owner/repo\n\tinsteadOf = https://github.com/owner/repo.git\n"
        ),
    )?;
    Ok(git_config)
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

fn run_git_with_author(args: &[&str], workdir: Option<&Path>, date: &str) -> anyhow::Result<()> {
    let mut command = Command::new("git");
    command.args(args);
    command.env("GIT_AUTHOR_DATE", date);
    command.env("GIT_COMMITTER_DATE", date);
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

fn git_stdout(args: &[&str], workdir: Option<&Path>) -> anyhow::Result<String> {
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
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}
