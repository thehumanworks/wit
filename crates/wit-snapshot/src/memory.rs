//! In-memory GitHub snapshot backend.
//!
//! Open fetches repository metadata + a recursive git tree over HTTP and holds
//! the path index in RAM. Blobs are fetched on demand (and optionally cached
//! under a hard memory budget). No files are written to disk.

use crate::{
    DirEntry, EntryKind, FileContent, RepoSnapshot, SnapshotBackend, SnapshotError,
    SnapshotProvenance, SnapshotResult, TreeEntry, TreeView, normalize_repo_path, split_owner_repo,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// Default max entries accepted from a recursive GitHub tree response.
pub const DEFAULT_MAX_TREE_ENTRIES: usize = 50_000;
/// Default max blob size (bytes) the memory path will fetch/decode.
pub const DEFAULT_MAX_BLOB_BYTES: u64 = 1_048_576;
/// Default total decoded blob cache budget.
pub const DEFAULT_MEMORY_BUDGET_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct MemoryBackendLimits {
    pub max_tree_entries: usize,
    pub max_blob_bytes: u64,
    pub memory_budget_bytes: usize,
}

impl Default for MemoryBackendLimits {
    fn default() -> Self {
        Self {
            max_tree_entries: DEFAULT_MAX_TREE_ENTRIES,
            max_blob_bytes: DEFAULT_MAX_BLOB_BYTES,
            memory_budget_bytes: DEFAULT_MEMORY_BUDGET_BYTES,
        }
    }
}

/// Minimal HTTP surface so tests can inject wiremock without touching disk.
pub trait GitHubHttpClient: Send + Sync {
    fn get_json(
        &self,
        path: &str,
    ) -> impl std::future::Future<Output = SnapshotResult<(u16, String)>> + Send;
}

/// Production client backed by reqwest against `https://api.github.com`.
#[cfg(feature = "http")]
#[derive(Clone)]
pub struct ReqwestGitHubClient {
    client: reqwest::Client,
    base_url: String,
    token: Option<String>,
}

#[cfg(feature = "http")]
impl ReqwestGitHubClient {
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> SnapshotResult<Self> {
        let client = reqwest::Client::builder()
            .user_agent("wit-snapshot/0.1 (+https://github.com/thehumanworks/wit)")
            .build()
            .map_err(|err| SnapshotError::Other(format!("failed to build HTTP client: {err}")))?;
        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
        })
    }

    pub fn from_env() -> SnapshotResult<Self> {
        crate_tls_note();
        let token = std::env::var("GITHUB_TOKEN")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let base = std::env::var("WIT_GITHUB_API_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "https://api.github.com".to_string());
        Self::new(base, token)
    }
}

#[cfg(feature = "http")]
impl GitHubHttpClient for ReqwestGitHubClient {
    async fn get_json(&self, path: &str) -> SnapshotResult<(u16, String)> {
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}/{}", self.base_url, path.trim_start_matches('/'))
        };
        let mut request = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.github+json");
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|err| SnapshotError::Other(format!("GitHub request failed: {err}")))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|err| SnapshotError::Other(format!("failed to read GitHub body: {err}")))?;
        Ok((status, body))
    }
}

#[cfg(feature = "http")]
fn crate_tls_note() {
    // rustls provider install lives in the wit crate for the full CLI; the demo
    // binary installs its own. No-op here keeps this crate free of rustls.
}

#[derive(Clone)]
pub struct MemoryBackend<C> {
    client: Arc<C>,
    limits: MemoryBackendLimits,
}

impl<C> MemoryBackend<C> {
    pub fn new(client: C, limits: MemoryBackendLimits) -> Self {
        Self {
            client: Arc::new(client),
            limits,
        }
    }
}

#[cfg(feature = "http")]
impl MemoryBackend<ReqwestGitHubClient> {
    pub fn from_env() -> SnapshotResult<Self> {
        Ok(Self::new(
            ReqwestGitHubClient::from_env()?,
            MemoryBackendLimits::default(),
        ))
    }
}

#[derive(Debug, Clone)]
struct TreeNode {
    kind: EntryKind,
    sha: String,
    size: Option<u64>,
}

/// One path from an in-memory recursive tree (files and directories).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkEntry {
    pub path: String,
    pub kind: EntryKind,
    pub sha: String,
    pub size: Option<u64>,
}

struct BlobCache {
    budget: usize,
    used: usize,
    entries: BTreeMap<String, Arc<Vec<u8>>>,
}

impl BlobCache {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            used: 0,
            entries: BTreeMap::new(),
        }
    }

    fn get(&self, sha: &str) -> Option<Arc<Vec<u8>>> {
        self.entries.get(sha).cloned()
    }

    fn insert(&mut self, sha: String, bytes: Arc<Vec<u8>>) -> SnapshotResult<()> {
        if self.entries.contains_key(&sha) {
            return Ok(());
        }
        let size = bytes.len();
        if size > self.budget {
            return Err(SnapshotError::MemoryPressure(format!(
                "blob {sha} is {size} bytes but memory budget is {} bytes",
                self.budget
            )));
        }
        while self.used + size > self.budget && !self.entries.is_empty() {
            if let Some((evicted_sha, evicted)) = self.entries.pop_first() {
                self.used = self.used.saturating_sub(evicted.len());
                let _ = evicted_sha;
            } else {
                break;
            }
        }
        if self.used + size > self.budget {
            return Err(SnapshotError::MemoryPressure(format!(
                "cannot cache blob {sha}: would exceed budget ({} used, {} budget)",
                self.used, self.budget
            )));
        }
        self.used += size;
        self.entries.insert(sha, bytes);
        Ok(())
    }
}

pub struct MemorySnapshot<C> {
    client: Arc<C>,
    limits: MemoryBackendLimits,
    provenance: SnapshotProvenance,
    /// Full repository-relative paths → node metadata (files and directories).
    nodes: BTreeMap<String, TreeNode>,
    blobs: Mutex<BlobCache>,
}

impl<C> MemorySnapshot<C> {
    fn from_fixture(
        provenance: SnapshotProvenance,
        nodes: BTreeMap<String, TreeNode>,
        limits: MemoryBackendLimits,
        client: Arc<C>,
    ) -> Self {
        let budget = limits.memory_budget_bytes;
        Self {
            client,
            limits,
            provenance,
            nodes,
            blobs: Mutex::new(BlobCache::new(budget)),
        }
    }

    fn ensure_path_exists(&self, path: &str) -> SnapshotResult<()> {
        if path.is_empty() {
            return Ok(());
        }
        if self.nodes.contains_key(path) {
            return Ok(());
        }
        // Directory may only exist implicitly via children.
        let prefix = format!("{path}/");
        if self.nodes.keys().any(|key| key.starts_with(&prefix)) {
            return Ok(());
        }
        Err(SnapshotError::MissingPath(path.to_string()))
    }

    /// Sorted walk of every file and directory node in the pinned tree.
    pub fn walk_entries(&self) -> Vec<WalkEntry> {
        self.nodes
            .iter()
            .map(|(path, node)| WalkEntry {
                path: path.clone(),
                kind: node.kind,
                sha: node.sha.clone(),
                size: node.size,
            })
            .collect()
    }

    /// Look up a single path's tree metadata.
    pub fn entry(&self, path: &str) -> Option<WalkEntry> {
        let path = normalize_repo_path(path);
        self.nodes.get(&path).map(|node| WalkEntry {
            path,
            kind: node.kind,
            sha: node.sha.clone(),
            size: node.size,
        })
    }

    /// Insert decoded blob bytes into the in-process cache (tests / offline fixtures).
    pub fn preload_blob(&self, sha: impl Into<String>, bytes: Vec<u8>) -> SnapshotResult<()> {
        let mut cache = self
            .blobs
            .lock()
            .map_err(|_| SnapshotError::Other("blob cache lock poisoned".to_string()))?;
        cache.insert(sha.into(), Arc::new(bytes))
    }
}

impl<C> MemorySnapshot<C>
where
    C: GitHubHttpClient + 'static,
{
    /// Fetch and decode a blob by SHA (used by MCP list metadata / read-by-oid).
    pub async fn blob_text_by_sha(&self, sha: &str, max_bytes: u64) -> SnapshotResult<String> {
        if let Some(size) = self
            .nodes
            .values()
            .find(|node| node.sha == sha && node.kind == EntryKind::File)
            .and_then(|node| node.size)
            && size > max_bytes
        {
            return Err(SnapshotError::OversizedBlob(format!(
                "blob {sha} is {size} bytes (max {max_bytes})"
            )));
        }

        let bytes = {
            let cache = self
                .blobs
                .lock()
                .map_err(|_| SnapshotError::Other("blob cache lock poisoned".to_string()))?;
            cache.get(sha)
        };

        let bytes = if let Some(hit) = bytes {
            hit
        } else {
            let (status, body) = self
                .client
                .get_json(&format!(
                    "/repos/{}/git/blobs/{}",
                    self.provenance.repo, sha
                ))
                .await?;
            if status != 200 {
                return Err(SnapshotError::from_status(
                    status,
                    &body,
                    &self.provenance.repo,
                ));
            }
            let blob: GitBlob = serde_json::from_str(&body)
                .map_err(|err| SnapshotError::Other(format!("decode blob: {err}")))?;
            if blob.size > max_bytes {
                return Err(SnapshotError::OversizedBlob(format!(
                    "blob {sha} is {} bytes (max {max_bytes})",
                    blob.size
                )));
            }
            let decoded = match blob.encoding.as_str() {
                "base64" => {
                    let cleaned: String = blob
                        .content
                        .chars()
                        .filter(|ch| !ch.is_whitespace())
                        .collect();
                    STANDARD.decode(cleaned).map_err(|err| {
                        SnapshotError::Other(format!("base64 decode failed for {sha}: {err}"))
                    })?
                }
                "utf-8" => blob.content.into_bytes(),
                other => {
                    return Err(SnapshotError::Other(format!(
                        "unsupported blob encoding '{other}' for {sha}"
                    )));
                }
            };
            let arc = Arc::new(decoded);
            let mut cache = self
                .blobs
                .lock()
                .map_err(|_| SnapshotError::Other("blob cache lock poisoned".to_string()))?;
            cache.insert(sha.to_string(), Arc::clone(&arc))?;
            arc
        };

        if bytes.contains(&0) {
            return Err(SnapshotError::BinaryFile(format!("blob {sha}")));
        }
        String::from_utf8(bytes.as_ref().clone())
            .map_err(|_| SnapshotError::BinaryFile(format!("blob {sha} (not valid UTF-8)")))
    }
}

impl<C> SnapshotBackend for MemoryBackend<C>
where
    C: GitHubHttpClient + 'static,
{
    type Snapshot = MemorySnapshot<C>;

    async fn open(&self, repo: &str, branch: Option<&str>) -> SnapshotResult<Self::Snapshot> {
        let (owner, name) = split_owner_repo(repo)?;
        let owner_repo = format!("{owner}/{name}");

        let (status, repo_body) = self
            .client
            .get_json(&format!("/repos/{owner_repo}"))
            .await?;
        if status != 200 {
            return Err(SnapshotError::from_status(status, &repo_body, &owner_repo));
        }
        let repo_meta: RepoMeta = serde_json::from_str(&repo_body)
            .map_err(|err| SnapshotError::Other(format!("decode repo metadata: {err}")))?;
        if repo_meta.private {
            return Err(SnapshotError::PrivateRepo(format!(
                "{owner_repo} is marked private; memory backend supports public repos only"
            )));
        }

        let requested = branch
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&repo_meta.default_branch)
            .to_string();

        let (status, commit_body) = self
            .client
            .get_json(&format!("/repos/{owner_repo}/commits/{requested}"))
            .await?;
        if status != 200 {
            return Err(SnapshotError::from_status(
                status,
                &commit_body,
                &owner_repo,
            ));
        }
        let commit: CommitMeta = serde_json::from_str(&commit_body)
            .map_err(|err| SnapshotError::Other(format!("decode commit: {err}")))?;
        let tree_sha = commit.commit.tree.sha.clone();
        let commit_sha = commit.sha;

        let (status, tree_body) = self
            .client
            .get_json(&format!(
                "/repos/{owner_repo}/git/trees/{tree_sha}?recursive=1"
            ))
            .await?;
        if status != 200 {
            return Err(SnapshotError::from_status(status, &tree_body, &owner_repo));
        }
        let tree: GitTree = serde_json::from_str(&tree_body)
            .map_err(|err| SnapshotError::Other(format!("decode tree: {err}")))?;
        if tree.truncated {
            return Err(SnapshotError::OversizedTree(format!(
                "{owner_repo}@{commit_sha} recursive tree is truncated by GitHub; narrow the repo or raise limits out of band"
            )));
        }
        if tree.tree.len() > self.limits.max_tree_entries {
            return Err(SnapshotError::OversizedTree(format!(
                "{owner_repo}@{commit_sha} has {} tree entries (max {})",
                tree.tree.len(),
                self.limits.max_tree_entries
            )));
        }

        let mut nodes = BTreeMap::new();
        for entry in tree.tree {
            let path = normalize_repo_path(&entry.path);
            if path.is_empty() {
                continue;
            }
            let kind = match entry.type_.as_str() {
                "blob" => EntryKind::File,
                "tree" => EntryKind::Dir,
                // Skip commits/submodules for this slice.
                _ => continue,
            };
            // Ensure ancestor directories exist as explicit nodes for list().
            let mut ancestor = String::new();
            for part in path.split('/') {
                if !ancestor.is_empty() {
                    ancestor.push('/');
                }
                ancestor.push_str(part);
                if ancestor == path {
                    break;
                }
                nodes.entry(ancestor.clone()).or_insert_with(|| TreeNode {
                    kind: EntryKind::Dir,
                    sha: String::new(),
                    size: None,
                });
            }
            nodes.insert(
                path,
                TreeNode {
                    kind,
                    sha: entry.sha,
                    size: entry.size,
                },
            );
        }

        let resolved_ref = if is_full_sha(&requested) || requested.starts_with("refs/") {
            requested.clone()
        } else {
            format!("refs/heads/{requested}")
        };

        Ok(MemorySnapshot {
            client: Arc::clone(&self.client),
            limits: self.limits.clone(),
            provenance: SnapshotProvenance {
                repo: owner_repo,
                requested_ref: requested,
                resolved_ref,
                commit_sha,
                tree_sha,
                backend: "memory".to_string(),
                cache_state: "memory".to_string(),
            },
            nodes,
            blobs: Mutex::new(BlobCache::new(self.limits.memory_budget_bytes)),
        })
    }
}

fn is_full_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl<C> RepoSnapshot for MemorySnapshot<C>
where
    C: GitHubHttpClient + 'static,
{
    fn provenance(&self) -> &SnapshotProvenance {
        &self.provenance
    }

    fn list(&self, path: Option<&str>) -> SnapshotResult<Vec<DirEntry>> {
        let base = normalize_repo_path(path.unwrap_or(""));
        self.ensure_path_exists(&base)?;
        if !base.is_empty()
            && let Some(node) = self.nodes.get(&base)
            && node.kind == EntryKind::File
        {
            return Err(SnapshotError::NotAFile(format!(
                "{base} is a file; list expects a directory"
            )));
        }

        let mut dirs: BTreeMap<String, DirEntry> = BTreeMap::new();
        let mut files: BTreeMap<String, DirEntry> = BTreeMap::new();

        for (full_path, node) in &self.nodes {
            let relative = if base.is_empty() {
                full_path.as_str()
            } else if let Some(rest) = full_path.strip_prefix(&base) {
                rest.strip_prefix('/').unwrap_or("")
            } else {
                continue;
            };
            if relative.is_empty() {
                continue;
            }
            if let Some((name, rest)) = relative.split_once('/') {
                dirs.entry(name.to_string()).or_insert_with(|| DirEntry {
                    name: name.to_string(),
                    kind: EntryKind::Dir,
                    path: if base.is_empty() {
                        name.to_string()
                    } else {
                        format!("{base}/{name}")
                    },
                    blob_sha: None,
                    size_bytes: None,
                    is_binary: false,
                });
                let _ = rest;
            } else {
                match node.kind {
                    EntryKind::Dir => {
                        dirs.insert(
                            relative.to_string(),
                            DirEntry {
                                name: relative.to_string(),
                                kind: EntryKind::Dir,
                                path: full_path.clone(),
                                blob_sha: None,
                                size_bytes: None,
                                is_binary: false,
                            },
                        );
                    }
                    EntryKind::File => {
                        files.insert(
                            relative.to_string(),
                            DirEntry {
                                name: relative.to_string(),
                                kind: EntryKind::File,
                                path: full_path.clone(),
                                blob_sha: Some(node.sha.clone()),
                                size_bytes: node.size,
                                is_binary: false,
                            },
                        );
                    }
                }
            }
        }

        let mut out: Vec<DirEntry> = dirs.into_values().collect();
        out.extend(files.into_values());
        Ok(out)
    }

    fn tree(&self, path: Option<&str>) -> SnapshotResult<TreeView> {
        let base = normalize_repo_path(path.unwrap_or(""));
        self.ensure_path_exists(&base)?;
        let root = if base.is_empty() {
            ".".to_string()
        } else {
            base.split('/').next_back().unwrap_or(&base).to_string()
        };

        let mut entries = Vec::new();
        for (full_path, node) in &self.nodes {
            if node.kind != EntryKind::File {
                continue;
            }
            let include = if base.is_empty() {
                true
            } else {
                full_path == &base
                    || full_path
                        .strip_prefix(&base)
                        .is_some_and(|rest| rest.starts_with('/'))
            };
            if !include {
                continue;
            }
            entries.push(TreeEntry {
                path: full_path.clone(),
                kind: EntryKind::File,
                blob_sha: Some(node.sha.clone()),
                size_bytes: node.size,
            });
        }
        Ok(TreeView {
            root,
            entries,
            truncated: false,
        })
    }

    async fn read(&self, path: &str) -> SnapshotResult<FileContent> {
        let path = normalize_repo_path(path);
        if path.is_empty() {
            return Err(SnapshotError::NotAFile(
                "path must identify a file".to_string(),
            ));
        }
        let node = self
            .nodes
            .get(&path)
            .ok_or_else(|| SnapshotError::MissingPath(path.clone()))?;
        if node.kind != EntryKind::File {
            return Err(SnapshotError::NotAFile(path));
        }
        if let Some(size) = node.size
            && size > self.limits.max_blob_bytes
        {
            return Err(SnapshotError::OversizedBlob(format!(
                "{path} is {size} bytes (max {})",
                self.limits.max_blob_bytes
            )));
        }

        let bytes = {
            let cache = self
                .blobs
                .lock()
                .map_err(|_| SnapshotError::Other("blob cache lock poisoned".to_string()))?;
            cache.get(&node.sha)
        };

        let bytes = if let Some(hit) = bytes {
            hit
        } else {
            let (status, body) = self
                .client
                .get_json(&format!(
                    "/repos/{}/git/blobs/{}",
                    self.provenance.repo, node.sha
                ))
                .await?;
            if status != 200 {
                return Err(SnapshotError::from_status(
                    status,
                    &body,
                    &self.provenance.repo,
                ));
            }
            let blob: GitBlob = serde_json::from_str(&body)
                .map_err(|err| SnapshotError::Other(format!("decode blob: {err}")))?;
            if blob.size > self.limits.max_blob_bytes {
                return Err(SnapshotError::OversizedBlob(format!(
                    "{path} is {} bytes (max {})",
                    blob.size, self.limits.max_blob_bytes
                )));
            }
            let decoded = match blob.encoding.as_str() {
                "base64" => {
                    let cleaned: String = blob
                        .content
                        .chars()
                        .filter(|ch| !ch.is_whitespace())
                        .collect();
                    STANDARD.decode(cleaned).map_err(|err| {
                        SnapshotError::Other(format!("base64 decode failed for {path}: {err}"))
                    })?
                }
                "utf-8" => blob.content.into_bytes(),
                other => {
                    return Err(SnapshotError::Other(format!(
                        "unsupported blob encoding '{other}' for {path}"
                    )));
                }
            };
            let arc = Arc::new(decoded);
            let mut cache = self
                .blobs
                .lock()
                .map_err(|_| SnapshotError::Other("blob cache lock poisoned".to_string()))?;
            cache.insert(node.sha.clone(), Arc::clone(&arc))?;
            arc
        };

        let is_binary = bytes.contains(&0);
        if is_binary {
            return Err(SnapshotError::BinaryFile(path));
        }
        let text = String::from_utf8(bytes.as_ref().clone())
            .map_err(|_| SnapshotError::BinaryFile(format!("{path} (not valid UTF-8)")))?;
        Ok(FileContent {
            path,
            blob_sha: node.sha.clone(),
            size_bytes: bytes.len() as u64,
            text,
            is_binary: false,
        })
    }
}

#[derive(Debug, Deserialize)]
struct RepoMeta {
    private: bool,
    default_branch: String,
}

#[derive(Debug, Deserialize)]
struct CommitMeta {
    sha: String,
    commit: CommitInner,
}

#[derive(Debug, Deserialize)]
struct CommitInner {
    tree: ShaOnly,
}

#[derive(Debug, Deserialize)]
struct ShaOnly {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct GitTree {
    #[serde(default)]
    truncated: bool,
    tree: Vec<GitTreeEntry>,
}

#[derive(Debug, Deserialize)]
struct GitTreeEntry {
    path: String,
    #[serde(rename = "type")]
    type_: String,
    sha: String,
    size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GitBlob {
    content: String,
    encoding: String,
    size: u64,
}

/// Build a snapshot purely from already-fetched JSON (wasmtime fixture demos).
pub fn snapshot_from_tree_json<C: GitHubHttpClient + 'static>(
    client: Arc<C>,
    repo: &str,
    requested_ref: &str,
    commit_sha: &str,
    tree_sha: &str,
    tree_json: &str,
    limits: MemoryBackendLimits,
) -> SnapshotResult<MemorySnapshot<C>> {
    let tree: GitTree = serde_json::from_str(tree_json)
        .map_err(|err| SnapshotError::Other(format!("decode tree fixture: {err}")))?;
    if tree.truncated {
        return Err(SnapshotError::OversizedTree(
            "fixture tree is truncated".to_string(),
        ));
    }
    if tree.tree.len() > limits.max_tree_entries {
        return Err(SnapshotError::OversizedTree(format!(
            "fixture has {} entries (max {})",
            tree.tree.len(),
            limits.max_tree_entries
        )));
    }
    let mut nodes = BTreeMap::new();
    for entry in tree.tree {
        let path = normalize_repo_path(&entry.path);
        if path.is_empty() {
            continue;
        }
        let kind = match entry.type_.as_str() {
            "blob" => EntryKind::File,
            "tree" => EntryKind::Dir,
            _ => continue,
        };
        let mut ancestor = String::new();
        for part in path.split('/') {
            if !ancestor.is_empty() {
                ancestor.push('/');
            }
            ancestor.push_str(part);
            if ancestor == path {
                break;
            }
            nodes.entry(ancestor.clone()).or_insert_with(|| TreeNode {
                kind: EntryKind::Dir,
                sha: String::new(),
                size: None,
            });
        }
        nodes.insert(
            path,
            TreeNode {
                kind,
                sha: entry.sha,
                size: entry.size,
            },
        );
    }
    Ok(MemorySnapshot::from_fixture(
        SnapshotProvenance {
            repo: repo.to_string(),
            requested_ref: requested_ref.to_string(),
            resolved_ref: format!("refs/heads/{requested_ref}"),
            commit_sha: commit_sha.to_string(),
            tree_sha: tree_sha.to_string(),
            backend: "memory".to_string(),
            cache_state: "memory".to_string(),
        },
        nodes,
        limits,
        client,
    ))
}

#[cfg(test)]
mod blob_cache_tests {
    use super::*;

    #[test]
    fn blob_cache_enforces_budget() {
        let mut cache = BlobCache::new(8);
        let ok = Arc::new(vec![1, 2, 3, 4]);
        cache.insert("a".into(), ok).unwrap();
        let too_big = Arc::new(vec![0; 16]);
        let err = cache.insert("b".into(), too_big).unwrap_err();
        assert!(matches!(err, SnapshotError::MemoryPressure(_)));
    }
}
