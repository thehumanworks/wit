use fs2::FileExt;
use std::{
    fs::{self, OpenOptions},
    path::Path,
    process::{Child, Command, ExitStatus},
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
        .spawn()
        .expect("failed to spawn wit rg process")
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

#[test]
#[ignore = "requires network access"]
fn test_cache_lock_serializes_parallel_cache_processes() {
    let wit_bin = env!("CARGO_BIN_EXE_wit");
    let repo = "octocat/Hello-World";

    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let cache_dir = temp.path().join("cache");
    fs::create_dir_all(&cache_dir).expect("failed to create cache dir");

    let lock_path = cache_dir.join(".cache.lock");
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
    assert!(
        cache_dir.join(repo).exists(),
        "cache directory should exist after successful runs"
    );
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

    let lock_path = cache_dir.join(".cache.lock");
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
    assert!(
        cache_dir.join(repo).exists(),
        "cache directory should exist after successful runs"
    );
}
