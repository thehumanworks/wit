//! Disk-backed [`RepoSnapshot`] adapter over the existing bare-repo cache.
//!
//! This keeps the production CLI path working while sharing the same open /
//! list / tree / read contract as the memory backend.

mod memory_ops;

pub use memory_ops::{
    filter_ignored_paths, grep_memory_snapshot, head_from_text, list_remote_branches_api,
    read_memory_text, tail_from_text,
};

use crate::gitops::ops::{
    CacheAcquisitionMode, CacheBranchSelection, FileMetadata, cache_github_repo, list_dir,
    read_file, tree_text_with_ignore,
};
use gix::Repository;
use gix::bstr::ByteSlice;
use wit_snapshot::{
    DirEntry, EntryKind, FileContent, RepoSnapshot, SnapshotBackend, SnapshotError,
    SnapshotProvenance, SnapshotResult, TreeEntry, TreeView, normalize_repo_path, split_owner_repo,
};

#[derive(Debug, Clone, Copy)]
pub struct DiskBackend {
    pub mode: CacheAcquisitionMode,
}

impl DiskBackend {
    pub fn new(mode: CacheAcquisitionMode) -> Self {
        Self { mode }
    }
}

impl Default for DiskBackend {
    fn default() -> Self {
        Self {
            mode: CacheAcquisitionMode::ServeStaleAndRevalidate,
        }
    }
}

pub struct DiskSnapshot {
    repo: Repository,
    provenance: SnapshotProvenance,
}

impl SnapshotBackend for DiskBackend {
    type Snapshot = DiskSnapshot;

    async fn open(&self, repo: &str, branch: Option<&str>) -> SnapshotResult<Self::Snapshot> {
        let (owner, name) = split_owner_repo(repo)?;
        let owner_repo = format!("{owner}/{name}");
        let selection = match branch {
            Some(name) if !name.trim().is_empty() => CacheBranchSelection::named(name.trim()),
            _ => CacheBranchSelection::Default,
        };
        let repository = cache_github_repo(&owner_repo, selection, self.mode)
            .await
            .map_err(|err| SnapshotError::Other(err.to_string()))?;

        let (commit_sha, tree_sha) = {
            let commit = repository
                .head_commit()
                .map_err(|err| SnapshotError::Other(err.to_string()))?;
            let commit_sha = commit.id().to_string();
            let tree_sha = commit
                .tree_id()
                .map_err(|err| SnapshotError::Other(err.to_string()))?
                .to_string();
            (commit_sha, tree_sha)
        };
        let requested = branch
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("HEAD")
            .to_string();
        let resolved_ref = if requested == "HEAD" {
            "HEAD".to_string()
        } else if requested.starts_with("refs/") {
            requested.clone()
        } else {
            format!("refs/heads/{requested}")
        };
        let cache_state = match self.mode {
            CacheAcquisitionMode::ForceInvalidate => "explicitly_refreshed",
            CacheAcquisitionMode::ServeStaleAndRevalidate => "stale_served_revalidating",
        }
        .to_string();

        Ok(DiskSnapshot {
            repo: repository,
            provenance: SnapshotProvenance {
                repo: owner_repo,
                requested_ref: requested,
                resolved_ref,
                commit_sha,
                tree_sha,
                backend: "disk".to_string(),
                cache_state,
            },
        })
    }
}

impl RepoSnapshot for DiskSnapshot {
    fn provenance(&self) -> &SnapshotProvenance {
        &self.provenance
    }

    fn list(&self, path: Option<&str>) -> SnapshotResult<Vec<DirEntry>> {
        let base = normalize_repo_path(path.unwrap_or(""));
        let entries = list_dir(
            &self.repo,
            if base.is_empty() { None } else { Some(&base) },
            false,
        )
        .map_err(|err| map_path_err(&base, err))?;
        if entries.is_empty() && !base.is_empty() {
            return match read_file(&self.repo, &base) {
                Ok(_) => Err(SnapshotError::NotAFile(base)),
                Err(_) => Err(SnapshotError::MissingPath(base)),
            };
        }
        Ok(entries
            .into_iter()
            .map(|entry| map_file_metadata(&base, entry))
            .collect())
    }

    fn tree(&self, path: Option<&str>) -> SnapshotResult<TreeView> {
        let base = normalize_repo_path(path.unwrap_or(""));
        let text = tree_text_with_ignore(
            &self.repo,
            if base.is_empty() {
                None
            } else {
                Some(base.as_str())
            },
            false,
            &[],
            None,
        )
        .map_err(|err| map_path_err(&base, err))?;
        let root = if base.is_empty() {
            ".".to_string()
        } else {
            base.split('/').next_back().unwrap_or(&base).to_string()
        };
        let mut entries = Vec::new();
        collect_file_paths(&self.repo, &base, &mut entries)?;
        Ok(TreeView {
            root,
            entries,
            truncated: text.truncated,
        })
    }

    fn read(
        &self,
        path: &str,
    ) -> impl std::future::Future<Output = SnapshotResult<FileContent>> + Send {
        // Compute synchronously so the future does not capture `&self`.
        // `gix::Repository` is `!Sync`, so an `async fn` body would not be `Send`.
        std::future::ready(self.read_sync(path))
    }
}

impl DiskSnapshot {
    fn read_sync(&self, path: &str) -> SnapshotResult<FileContent> {
        let path = normalize_repo_path(path);
        if path.is_empty() {
            return Err(SnapshotError::NotAFile(
                "path must identify a file".to_string(),
            ));
        }
        let content = read_file(&self.repo, &path).map_err(|err| {
            let message = err.to_string();
            if message.contains("File not found") || message.contains("not found") {
                SnapshotError::MissingPath(path.clone())
            } else if message.to_ascii_lowercase().contains("utf") || message.contains("not valid")
            {
                SnapshotError::BinaryFile(path.clone())
            } else {
                SnapshotError::Other(message)
            }
        })?;
        if content.as_bytes().contains(&0) {
            return Err(SnapshotError::BinaryFile(path));
        }
        Ok(FileContent {
            path: path.clone(),
            blob_sha: lookup_blob_sha(&self.repo, &path).unwrap_or_default(),
            size_bytes: content.len() as u64,
            text: content,
            is_binary: false,
        })
    }
}

fn collect_file_paths(
    repo: &Repository,
    base: &str,
    out: &mut Vec<TreeEntry>,
) -> SnapshotResult<()> {
    let commit = repo
        .head_commit()
        .map_err(|err| SnapshotError::Other(err.to_string()))?;
    let tree = commit
        .tree()
        .map_err(|err| SnapshotError::Other(err.to_string()))?;
    let mut recorder = gix::traverse::tree::Recorder::default();
    tree.traverse()
        .breadthfirst(&mut recorder)
        .map_err(|err| SnapshotError::Other(err.to_string()))?;
    for entry in recorder.records.iter().filter(|e| e.mode.is_blob()) {
        let full_path = entry
            .filepath
            .to_str()
            .map_err(|err| SnapshotError::Other(err.to_string()))?
            .to_string();
        let include = if base.is_empty() {
            true
        } else {
            full_path == base
                || full_path
                    .strip_prefix(base)
                    .is_some_and(|rest| rest.starts_with('/'))
        };
        if include {
            out.push(TreeEntry {
                path: full_path,
                kind: EntryKind::File,
                blob_sha: Some(entry.oid.to_string()),
                size_bytes: None,
            });
        }
    }
    Ok(())
}

fn lookup_blob_sha(repo: &Repository, path: &str) -> Option<String> {
    let tree = repo.head_commit().ok()?.tree().ok()?;
    let entry = tree.lookup_entry_by_path(path).ok()??;
    Some(entry.oid().to_string())
}

fn map_file_metadata(base: &str, entry: FileMetadata) -> DirEntry {
    let path = if base.is_empty() {
        entry.name.clone()
    } else {
        format!("{base}/{}", entry.name)
    };
    DirEntry {
        name: entry.name,
        kind: if entry.is_dir {
            EntryKind::Dir
        } else {
            EntryKind::File
        },
        path,
        blob_sha: None,
        size_bytes: entry.size_bytes,
        is_binary: entry.is_binary,
    }
}

fn map_path_err(path: &str, err: anyhow::Error) -> SnapshotError {
    let message = err.to_string();
    if message.contains("not found") || message.contains("does not exist") {
        SnapshotError::MissingPath(path.to_string())
    } else {
        SnapshotError::Other(message)
    }
}

/// Which snapshot backend the human CLI should use for repo-reading commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliSnapshotBackend {
    Disk,
    Memory,
}

impl CliSnapshotBackend {
    pub fn from_env_or_flag(flag: Option<&str>) -> SnapshotResult<Self> {
        if let Some(value) = flag {
            return Self::parse(value);
        }
        if let Ok(value) = std::env::var("WIT_SNAPSHOT_BACKEND")
            && !value.trim().is_empty()
        {
            return Self::parse(value.trim());
        }
        Ok(Self::Disk)
    }

    fn parse(value: &str) -> SnapshotResult<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "disk" | "cache" => Ok(Self::Disk),
            "memory" | "mem" | "no-fs" | "nofs" => Ok(Self::Memory),
            other => Err(SnapshotError::Other(format!(
                "unknown snapshot backend '{other}' (expected disk|memory)"
            ))),
        }
    }
}
