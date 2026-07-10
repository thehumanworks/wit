use fs2::FileExt;
use std::{
    fs::{self, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

fn spawn_cache_process(wit_bin: &str, cache_dir: &Path, repo: &str) -> Child {
    Command::new(wit_bin)
        .arg("cache")
        .arg("-r")
        .arg(repo)
        .env("WIT_CACHE_DIR", cache_dir)
        .spawn()
        .expect("failed to spawn wit cache process")
}

fn spawn_rg_process(wit_bin: &str, cache_dir: &Path, pattern: &str, repo: &str) -> Child {
    Command::new(wit_bin)
        .arg("rg")
        .arg(pattern)
        .arg("-r")
        .arg(repo)
        .env("WIT_CACHE_DIR", cache_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn wit rg process")
}

fn spawn_configured_rg_process(
    wit_bin: &str,
    cache_dir: &Path,
    git_config: &Path,
    pattern: &str,
    repo: &str,
) -> Child {
    Command::new(wit_bin)
        .arg("rg")
        .arg(pattern)
        .arg("-r")
        .arg(repo)
        .env("WIT_CACHE_DIR", cache_dir)
        .env("GIT_CONFIG_GLOBAL", git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn configured wit rg process")
}

fn wait_for_exit(child: &mut Child, timeout: Duration, label: &str) -> ExitStatus {
    let deadline = Instant::now() + timeout;

    loop {
        if let Some(status) = child.try_wait().expect("failed to poll child status") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("timed out waiting for {label} process");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_output(
    mut child: Child,
    timeout: Duration,
    label: &str,
) -> (ExitStatus, String, String) {
    let status = wait_for_exit(&mut child, timeout, label);
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .expect("child stdout should be piped")
        .read_to_string(&mut stdout)
        .expect("failed to read child stdout");
    child
        .stderr
        .take()
        .expect("child stderr should be piped")
        .read_to_string(&mut stderr)
        .expect("failed to read child stderr");
    (status, stdout, stderr)
}

fn assert_branch_cache_exists(cache_dir: &Path, repo: &str) {
    let (owner, name) = repo
        .split_once('/')
        .expect("test repo should be formatted as owner/repo");
    assert!(
        cache_dir.join(owner).join(name).join("branches").exists(),
        "branch cache directory should exist after successful runs"
    );
}

fn default_branch_lock_path(cache_dir: &Path, repo: &str) -> std::path::PathBuf {
    branch_lock_path(cache_dir, repo, "master")
}

fn branch_lock_path(cache_dir: &Path, repo: &str, branch: &str) -> PathBuf {
    let (owner, name) = repo
        .split_once('/')
        .expect("test repo should be formatted as owner/repo");
    cache_dir
        .join(owner)
        .join(name)
        .join("branches")
        .join(format!("b-{branch}"))
        .join(".cache.lock")
}

#[cfg(unix)]
#[test]
fn warm_cache_reads_do_not_wait_for_branch_revalidation_lock() -> anyhow::Result<()> {
    let wit_bin = env!("CARGO_BIN_EXE_wit");
    let repo = "owner/repo";
    let pattern = "warm_cache_marker";
    let temp = tempfile::tempdir()?;
    let cache_dir = temp.path().join("cache");
    let remote = create_remote(temp.path())?;
    let populate_git_config = write_url_rewrite_config(temp.path(), &remote)?;

    let populate = Command::new(wit_bin)
        .args(["cache", "-r", repo])
        .env("WIT_CACHE_DIR", &cache_dir)
        .env("GIT_CONFIG_GLOBAL", &populate_git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()?;
    anyhow::ensure!(
        populate.status.success(),
        "failed to populate cache: {}",
        String::from_utf8_lossy(&populate.stderr)
    );

    let (git_config, revalidation_started, release_revalidation) =
        write_blocked_revalidation_config(temp.path(), &remote)?;
    let revalidation = Command::new(wit_bin)
        .args(["__cache-revalidate", "--repo", repo, "--branch", "main"])
        .env("WIT_CACHE_DIR", &cache_dir)
        .env("GIT_CONFIG_GLOBAL", &git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ALLOW_PROTOCOL", "ext")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let marker_deadline = Instant::now() + Duration::from_secs(2);
    while !revalidation_started.exists() {
        anyhow::ensure!(
            Instant::now() < marker_deadline,
            "timed out waiting for revalidation to reach the remote check"
        );
        thread::sleep(Duration::from_millis(20));
    }

    let mut children: Vec<Child> = (0..4)
        .map(|_| spawn_configured_rg_process(wit_bin, &cache_dir, &git_config, pattern, repo))
        .collect();

    let reads_deadline = Instant::now() + Duration::from_secs(2);
    let completed_during_revalidation = loop {
        let completed: Vec<bool> = children
            .iter_mut()
            .map(|child| child.try_wait().expect("failed to poll warm rg").is_some())
            .collect();
        if completed.iter().all(|done| *done) || Instant::now() >= reads_deadline {
            break completed;
        }
        thread::sleep(Duration::from_millis(20));
    };

    std::fs::write(release_revalidation, "release\n")?;

    let (revalidation_status, _, revalidation_stderr) =
        wait_for_output(revalidation, Duration::from_secs(10), "revalidation");
    assert!(
        revalidation_status.success(),
        "revalidation failed: {revalidation_stderr}"
    );

    for (index, child) in children.into_iter().enumerate() {
        let (status, stdout, stderr) = wait_for_output(child, Duration::from_secs(5), "warm rg");
        assert!(
            status.success(),
            "warm rg process {index} exited with {status}: {stderr}"
        );
        assert!(
            stdout.contains(pattern),
            "warm rg process {index} did not return cached content: {stdout}"
        );
    }

    assert!(
        completed_during_revalidation
            .into_iter()
            .all(|completed| completed),
        "warm reads should finish while the remote freshness check is still running"
    );
    Ok(())
}

#[test]
#[ignore = "requires network access"]
fn test_cache_lock_serializes_parallel_cache_processes() {
    let wit_bin = env!("CARGO_BIN_EXE_wit");
    let repo = "octocat/Hello-World";

    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let cache_dir = temp.path().join("cache");
    fs::create_dir_all(&cache_dir).expect("failed to create cache dir");

    let lock_path = default_branch_lock_path(&cache_dir, repo);
    fs::create_dir_all(lock_path.parent().unwrap()).expect("failed to create lock parent");
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .expect("failed to open lock file");
    lock_file
        .lock_exclusive()
        .expect("failed to acquire pre-test lock");

    let mut first = spawn_cache_process(wit_bin, &cache_dir, repo);
    let mut second = spawn_cache_process(wit_bin, &cache_dir, repo);

    thread::sleep(Duration::from_millis(250));
    assert!(
        first
            .try_wait()
            .expect("failed to poll first process")
            .is_none(),
        "first process should be blocked by cache lock"
    );
    assert!(
        second
            .try_wait()
            .expect("failed to poll second process")
            .is_none(),
        "second process should be blocked by cache lock"
    );

    lock_file.unlock().expect("failed to unlock pre-test lock");
    drop(lock_file);

    let first_status = wait_for_exit(&mut first, Duration::from_secs(180), "first");
    let second_status = wait_for_exit(&mut second, Duration::from_secs(180), "second");

    assert!(
        first_status.success(),
        "first process exited with status {first_status}"
    );
    assert!(
        second_status.success(),
        "second process exited with status {second_status}"
    );
    assert_branch_cache_exists(&cache_dir, repo);
}

#[test]
#[ignore = "requires network access"]
fn test_cache_lock_serializes_parallel_rg_processes() {
    let wit_bin = env!("CARGO_BIN_EXE_wit");
    let repo = "octocat/Hello-World";
    let pattern = "Hello";

    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let cache_dir = temp.path().join("cache");
    fs::create_dir_all(&cache_dir).expect("failed to create cache dir");

    let lock_path = default_branch_lock_path(&cache_dir, repo);
    fs::create_dir_all(lock_path.parent().unwrap()).expect("failed to create lock parent");
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .expect("failed to open lock file");
    lock_file
        .lock_exclusive()
        .expect("failed to acquire pre-test lock");

    let mut children: Vec<Child> = (0..4)
        .map(|_| spawn_rg_process(wit_bin, &cache_dir, pattern, repo))
        .collect();

    thread::sleep(Duration::from_millis(250));
    for (index, child) in children.iter_mut().enumerate() {
        assert!(
            child
                .try_wait()
                .expect("failed to poll rg process")
                .is_none(),
            "rg process {index} should be blocked by cache lock"
        );
    }

    lock_file.unlock().expect("failed to unlock pre-test lock");
    drop(lock_file);

    for (index, child) in children.iter_mut().enumerate() {
        let status = wait_for_exit(child, Duration::from_secs(180), "rg");
        assert!(status.success(), "rg process {index} exited with {status}");
    }
    assert_branch_cache_exists(&cache_dir, repo);
}

fn create_remote(root: &Path) -> anyhow::Result<PathBuf> {
    let worktree = root.join("worktree");
    let remote = root.join("remote.git");

    run_git(&["init", worktree.to_str().unwrap()], None)?;
    run_git(&["checkout", "-b", "main"], Some(&worktree))?;
    std::fs::write(
        worktree.join("README.md"),
        "warm_cache_marker\ncached repository content\n",
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
            "initial",
        ],
        Some(&worktree),
    )?;

    run_git(&["init", "--bare", remote.to_str().unwrap()], None)?;
    run_git(
        &["remote", "add", "origin", remote.to_str().unwrap()],
        Some(&worktree),
    )?;
    run_git(&["push", "origin", "main"], Some(&worktree))?;
    run_git(&["symbolic-ref", "HEAD", "refs/heads/main"], Some(&remote))?;
    Ok(remote)
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

#[cfg(unix)]
fn write_blocked_revalidation_config(
    root: &Path,
    remote: &Path,
) -> anyhow::Result<(PathBuf, PathBuf, PathBuf)> {
    use std::os::unix::fs::PermissionsExt;

    let marker = root.join("revalidation-started");
    let release = root.join("release-revalidation");
    let upload_pack = root.join("slow-upload-pack");
    std::fs::write(
        &upload_pack,
        format!(
            "#!/bin/sh\n: > '{}'\nwhile [ ! -e '{}' ]; do\n  sleep 0.05\ndone\nexec git-upload-pack '{}'\n",
            marker.display(),
            release.display(),
            remote.display()
        ),
    )?;
    let mut permissions = std::fs::metadata(&upload_pack)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&upload_pack, permissions)?;

    let git_config = root.join("slow-gitconfig");
    std::fs::write(
        &git_config,
        format!(
            "[url \"ext::{}\"]\n\tinsteadOf = https://github.com/owner/repo\n\tinsteadOf = https://github.com/owner/repo.git\n",
            upload_pack.display()
        ),
    )?;
    Ok((git_config, marker, release))
}

fn run_git(args: &[&str], workdir: Option<&Path>) -> anyhow::Result<()> {
    let mut command = Command::new("git");
    command.args(args);
    if let Some(dir) = workdir {
        command.current_dir(dir);
    }
    let output = command.output()?;
    anyhow::ensure!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
