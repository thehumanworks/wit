use std::{fs, path::Path, process::Command};

fn dir_size(path: &Path) -> u64 {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                dir_size(&entry.path())
            } else {
                meta.len()
            }
        })
        .sum()
}

#[test]
#[ignore = "requires network access"]
fn tree_openai_codex_caches_only_the_branch_tip() {
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    let output = Command::new(env!("CARGO_BIN_EXE_wit"))
        .args(["tree", "openai/codex"])
        .env("WIT_CACHE_DIR", &cache_dir)
        .output()
        .expect("failed to run wit tree");
    assert!(
        output.status.success(),
        "wit tree openai/codex failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("codex-rs"));

    let repo = cache_dir.join("openai/codex/branches/b-main/repo.git");
    let shallow = fs::read_to_string(repo.join("shallow")).unwrap();
    assert_eq!(shallow.lines().count(), 1, "expected a single shallow tip");
    let packed_refs = fs::read_to_string(repo.join("packed-refs")).unwrap_or_default();
    assert!(
        !packed_refs.contains("refs/tags/"),
        "cache fetched release tags"
    );
    let size = dir_size(&repo);
    assert!(
        size < 100 * 1024 * 1024,
        "depth-1 cache unexpectedly large: {size} bytes"
    );
}
