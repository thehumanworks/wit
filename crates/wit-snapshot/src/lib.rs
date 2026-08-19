//! Repository snapshot backends that do not require a working-tree clone.
//!
//! The memory backend loads a public GitHub repository tree over HTTP and serves
//! list/tree/read from RAM. It never writes under `WIT_CACHE_DIR` or any other
//! disk cache. Disk-backed caching remains in the `wit` crate and implements the
//! same [`RepoSnapshot`] contract via an adapter.

mod error;
pub mod memory;
mod types;

pub use error::{SnapshotError, SnapshotResult};
pub use memory::{
    GitHubHttpClient, MemoryBackend, MemoryBackendLimits, MemorySnapshot, ReqwestGitHubClient,
    snapshot_from_tree_json,
};
pub use types::{DirEntry, EntryKind, FileContent, SnapshotProvenance, TreeEntry, TreeView};

/// Open a repository into an immutable snapshot, then list/tree/read against it.
pub trait SnapshotBackend: Send {
    type Snapshot: RepoSnapshot;

    /// Resolve `owner/repo` (optional branch) to a pinned commit and load enough
    /// metadata to serve list/tree/read with the same provenance shape as disk.
    fn open(
        &self,
        repo: &str,
        branch: Option<&str>,
    ) -> impl std::future::Future<Output = SnapshotResult<Self::Snapshot>> + Send;
}

/// Read-only view of a pinned repository commit.
///
/// Note: not `Sync` because the disk adapter wraps `gix::Repository`, which is
/// `!Sync` (internal `RefCell` caches). Callers that need cross-thread sharing
/// should use the memory backend or open per-thread snapshots.
pub trait RepoSnapshot: Send {
    fn provenance(&self) -> &SnapshotProvenance;

    /// Immediate children of `path` (repository root when `None` / empty).
    fn list(&self, path: Option<&str>) -> SnapshotResult<Vec<DirEntry>>;

    /// Recursive path listing under `path` (tree-shaped; files only, dirs implied).
    fn tree(&self, path: Option<&str>) -> SnapshotResult<TreeView>;

    /// Read a blob as UTF-8 text. Binary blobs return [`SnapshotError::BinaryFile`].
    fn read(
        &self,
        path: &str,
    ) -> impl std::future::Future<Output = SnapshotResult<FileContent>> + Send;
}

/// Normalize a repository-relative path the same way the disk CLI does.
pub fn normalize_repo_path(value: &str) -> String {
    value
        .trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

/// Validate `owner/repo` identity (GitHub-safe characters).
pub fn split_owner_repo(owner_repo: &str) -> SnapshotResult<(&str, &str)> {
    let (owner, repo) = owner_repo
        .split_once('/')
        .filter(|(owner, repo)| !owner.is_empty() && !repo.is_empty() && !repo.contains('/'))
        .ok_or_else(|| {
            SnapshotError::InvalidRepo(format!(
                "expected GitHub repository as owner/repo, got '{owner_repo}'"
            ))
        })?;
    if !is_safe_repo_component(owner) || !is_safe_repo_component(repo) {
        return Err(SnapshotError::InvalidRepo(format!(
            "invalid GitHub repository identity: '{owner_repo}'"
        )));
    }
    Ok((owner, repo))
}

fn is_safe_repo_component(component: &str) -> bool {
    component != "."
        && component != ".."
        && component.bytes().all(
            |byte| matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'_' | b'-'),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_slashes_and_dot_prefix() {
        assert_eq!(normalize_repo_path(" /src/lib.rs/ "), "src/lib.rs");
        assert_eq!(normalize_repo_path("./README.md"), "README.md");
    }

    #[test]
    fn split_owner_repo_rejects_bad_forms() {
        assert!(split_owner_repo("only-one").is_err());
        assert!(split_owner_repo("a/b/c").is_err());
        assert!(split_owner_repo("../x").is_err());
        assert!(split_owner_repo("owner/repo").is_ok());
    }
}
