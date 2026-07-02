use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[test]
fn branch_selected_cli_commands_read_named_branch() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let cache_dir = temp.path().join("cache");
    let remote = create_remote_with_feature_branch(temp.path())?;
    let git_config = write_url_rewrite_config(temp.path(), &remote)?;
    let wit_bin = env!("CARGO_BIN_EXE_wit");

    let cache = run_wit(
        wit_bin,
        &cache_dir,
        &git_config,
        &["cache", "-r", "owner/repo", "--branch", "feature/cli"],
    )?;
    assert!(cache.contains("b-feature%2Fcli"));

    let tree = run_wit(
        wit_bin,
        &cache_dir,
        &git_config,
        &["tree", "--branch", "feature/cli", "-r", "owner/repo"],
    )?;
    assert!(tree.contains("feature.txt"));

    let ls = run_wit(
        wit_bin,
        &cache_dir,
        &git_config,
        &["ls", "--branch", "feature/cli", "-r", "owner/repo"],
    )?;
    assert!(ls.contains("feature.txt"));

    let cat = run_wit(
        wit_bin,
        &cache_dir,
        &git_config,
        &[
            "cat",
            "--branch",
            "feature/cli",
            "-r",
            "owner/repo",
            "feature.txt",
        ],
    )?;
    assert!(cat.contains("feature marker"));

    let rg = run_wit(
        wit_bin,
        &cache_dir,
        &git_config,
        &[
            "rg",
            "--branch",
            "feature/cli",
            "feature_marker",
            "-r",
            "owner/repo",
        ],
    )?;
    assert!(rg.contains("feature_marker"));

    let sed = run_wit(
        wit_bin,
        &cache_dir,
        &git_config,
        &[
            "sed",
            "--branch",
            "feature/cli",
            "-n",
            "-r",
            "owner/repo",
            "1p",
            "feature.txt",
        ],
    )?;
    assert_eq!(sed.trim(), "feature marker");

    let head = run_wit(
        wit_bin,
        &cache_dir,
        &git_config,
        &[
            "head",
            "--branch",
            "feature/cli",
            "-r",
            "owner/repo",
            "-n",
            "1",
            "feature.txt",
        ],
    )?;
    assert_eq!(head.trim(), "feature marker");

    let tail = run_wit(
        wit_bin,
        &cache_dir,
        &git_config,
        &[
            "tail",
            "--branch",
            "feature/cli",
            "-r",
            "owner/repo",
            "-n",
            "1",
            "feature.txt",
        ],
    )?;
    assert_eq!(tail.trim(), "feature_marker();");

    let default_readme = run_wit(
        wit_bin,
        &cache_dir,
        &git_config,
        &["cat", "-r", "owner/repo", "README.md"],
    )?;
    assert!(default_readme.contains("main branch"));
    assert!(!default_readme.contains("feature branch"));

    std::fs::rename(&remote, temp.path().join("remote-offline.git"))?;
    let offline_default_readme = run_wit(
        wit_bin,
        &cache_dir,
        &git_config,
        &["cat", "-r", "owner/repo", "README.md"],
    )?;
    assert!(offline_default_readme.contains("main branch"));
    assert!(!offline_default_readme.contains("feature branch"));

    Ok(())
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

fn create_remote_with_feature_branch(root: &Path) -> anyhow::Result<PathBuf> {
    let worktree = root.join("worktree");
    let remote = root.join("remote.git");

    run_git(&["init", worktree.to_str().unwrap()], None)?;
    run_git(&["checkout", "-b", "main"], Some(&worktree))?;
    std::fs::create_dir_all(worktree.join("src"))?;
    std::fs::write(worktree.join("README.md"), "main branch\n")?;
    std::fs::write(
        worktree.join("src").join("lib.rs"),
        "pub fn main_branch() {}\n",
    )?;
    commit_all(&worktree, "main")?;

    run_git(&["init", "--bare", remote.to_str().unwrap()], None)?;
    run_git(
        &["remote", "add", "origin", remote.to_str().unwrap()],
        Some(&worktree),
    )?;
    run_git(&["push", "origin", "main"], Some(&worktree))?;
    run_git(&["symbolic-ref", "HEAD", "refs/heads/main"], Some(&remote))?;

    run_git(&["checkout", "-b", "feature/cli"], Some(&worktree))?;
    std::fs::write(worktree.join("README.md"), "feature branch\n")?;
    std::fs::write(
        worktree.join("feature.txt"),
        "feature marker\nfeature_marker();\n",
    )?;
    commit_all(&worktree, "feature")?;
    run_git(&["push", "origin", "feature/cli"], Some(&worktree))?;

    Ok(remote)
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
