use crate::ast::{self, AstCapture, AstLanguage, AstSymbol, SymbolFilter};
use crate::{
    gitops::ops::{CacheAcquisitionMode, CacheBranchSelection, cache_github_repo_with_context},
    operation_context::command_output,
    search::{GitHubSearchClient, MAX_GITHUB_REPOS},
    snapshot::CliSnapshotBackend,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};
use tempfile::TempDir;
use wit_snapshot::{
    EntryKind, MemoryBackend, MemorySnapshot, RepoSnapshot, ReqwestGitHubClient, SnapshotBackend,
    SnapshotError,
};
#[cfg(test)]
use wit_snapshot::{MemoryBackendLimits, snapshot_from_tree_json};

pub use crate::operation_context::{OperationCancellation, OperationContext};

const DEFAULT_BUDGET_BYTES: usize = 64 * 1024;
const MIN_BUDGET_BYTES: usize = 1024;
const MAX_BUDGET_BYTES: usize = 256 * 1024;
const DEFAULT_PAGE_ITEMS: usize = 100;
const MAX_PAGE_ITEMS: usize = 1000;
const DEFAULT_LIST_DEPTH: usize = 2;
const MAX_LIST_DEPTH: usize = 32;
const DEFAULT_AST_MAX_FILES: usize = 200;
const MAX_AST_MAX_FILES: usize = 1000;
const DEFAULT_CONTEXT_LINES: usize = 4;
const MAX_CONTEXT_LINES: usize = 100;
const DEFAULT_CONTEXT_RESULTS: usize = 20;
const MAX_CONTEXT_RESULTS: usize = 100;
const MAX_CONTEXT_CANDIDATES: usize = 5000;

#[derive(Clone)]
pub struct WitOperations {
    snapshots: Arc<Mutex<HashMap<String, SnapshotRecord>>>,
    snapshot_backend: CliSnapshotBackend,
    /// Optional injected GitHub HTTP client for the memory backend (tests / custom base URL).
    memory_github: Option<ReqwestGitHubClient>,
}

impl WitOperations {
    pub fn new() -> Self {
        Self {
            snapshots: Arc::new(Mutex::new(HashMap::new())),
            snapshot_backend: CliSnapshotBackend::from_env_or_flag(None)
                .unwrap_or(CliSnapshotBackend::Disk),
            memory_github: None,
        }
    }

    /// Construct with an explicit snapshot backend (disk remains the production default).
    pub fn with_backend(snapshot_backend: CliSnapshotBackend) -> Self {
        Self {
            snapshots: Arc::new(Mutex::new(HashMap::new())),
            snapshot_backend,
            memory_github: None,
        }
    }

    /// Memory backend with an injected GitHub HTTP client (wiremock / custom API base).
    pub fn with_memory_github_client(client: ReqwestGitHubClient) -> Self {
        Self {
            snapshots: Arc::new(Mutex::new(HashMap::new())),
            snapshot_backend: CliSnapshotBackend::Memory,
            memory_github: Some(client),
        }
    }

    fn memory_backend(&self) -> Result<MemoryBackend<ReqwestGitHubClient>, String> {
        match &self.memory_github {
            Some(client) => Ok(MemoryBackend::new(
                client.clone(),
                wit_snapshot::MemoryBackendLimits::default(),
            )),
            None => MemoryBackend::from_env().map_err(snapshot_error),
        }
    }

    fn snapshot(&self, snapshot_id: &str) -> Result<SnapshotHandle, String> {
        let snapshots = self
            .snapshots
            .lock()
            .map_err(|_| "snapshot registry lock was poisoned".to_string())?;
        snapshots
            .get(snapshot_id)
            .map(SnapshotRecord::handle)
            .ok_or_else(|| {
                format!(
                    "unknown or expired snapshot_id '{snapshot_id}'; call wit_open in this server session"
                )
            })
    }

    fn publish_snapshot(
        &self,
        context: &OperationContext,
        snapshot_id: String,
        record: SnapshotRecord,
    ) -> Result<(), String> {
        context.check()?;
        let mut snapshots = self
            .snapshots
            .lock()
            .map_err(|_| "snapshot registry lock was poisoned".to_string())?;
        context.check()?;
        snapshots.entry(snapshot_id).or_insert(record);
        // Publication is immutable and complete. Never roll it back after releasing the lock: a
        // concurrent open may already have claimed the same snapshot. The adapter still checks
        // the context and discards this invocation's late result.
        context.check()
    }
}

#[derive(Debug)]
struct ResolvedRef {
    kind: ResolvedRefKind,
    resolved_ref: String,
    fetch_ref: String,
    commit_sha: Option<String>,
}

#[derive(Debug)]
enum ResolvedRefKind {
    DefaultBranch(String),
    Branch(String),
    Tag,
    Commit,
}

impl ResolvedRefKind {
    fn label(&self) -> &'static str {
        match self {
            Self::DefaultBranch(_) | Self::Branch(_) => "branch",
            Self::Tag => "tag",
            Self::Commit => "commit",
        }
    }
}

#[derive(Debug)]
struct RemoteRefs {
    default_branch: String,
    branches: BTreeMap<String, String>,
    tags: BTreeMap<String, String>,
}

#[derive(Debug)]
struct TreeEntry {
    kind: String,
    oid: String,
    size: Option<u64>,
    path: String,
}

fn validate_repo(repo: &str) -> Result<(), String> {
    let mut parts = repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if owner.is_empty()
        || name.is_empty()
        || parts.next().is_some()
        || !owner
            .chars()
            .chain(name.chars())
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err("repo must be in owner/repo form using GitHub-safe characters".to_string());
    }
    Ok(())
}

fn github_remote_url(repo: &str) -> String {
    format!("https://github.com/{repo}")
}

fn snapshot_id(repo: &str, commit_sha: &str) -> String {
    format!("{repo}@{commit_sha}")
}

fn resolve_requested_ref(
    context: &OperationContext,
    remote: &str,
    requested: &str,
) -> Result<ResolvedRef, String> {
    let requested = requested.trim();
    if requested.is_empty() || requested == "HEAD" {
        let refs = query_remote_refs(context, remote)?;
        let sha = refs
            .branches
            .get(&refs.default_branch)
            .cloned()
            .ok_or_else(|| "remote default branch was not listed".to_string())?;
        return Ok(ResolvedRef {
            kind: ResolvedRefKind::DefaultBranch(refs.default_branch.clone()),
            resolved_ref: format!("refs/heads/{}", refs.default_branch),
            fetch_ref: "HEAD".to_string(),
            commit_sha: Some(sha),
        });
    }
    if is_full_sha(requested) {
        return Ok(ResolvedRef {
            kind: ResolvedRefKind::Commit,
            resolved_ref: requested.to_ascii_lowercase(),
            fetch_ref: requested.to_ascii_lowercase(),
            commit_sha: Some(requested.to_ascii_lowercase()),
        });
    }

    let refs = query_remote_refs(context, remote)?;
    let branch_name = requested.strip_prefix("refs/heads/").unwrap_or(requested);
    if let Some(sha) = refs.branches.get(branch_name) {
        return Ok(ResolvedRef {
            kind: ResolvedRefKind::Branch(branch_name.to_string()),
            resolved_ref: format!("refs/heads/{branch_name}"),
            fetch_ref: format!("refs/heads/{branch_name}"),
            commit_sha: Some(sha.clone()),
        });
    }
    let tag_name = requested.strip_prefix("refs/tags/").unwrap_or(requested);
    if let Some(sha) = refs.tags.get(tag_name) {
        return Ok(ResolvedRef {
            kind: ResolvedRefKind::Tag,
            resolved_ref: format!("refs/tags/{tag_name}"),
            fetch_ref: format!("refs/tags/{tag_name}"),
            commit_sha: Some(sha.clone()),
        });
    }
    Err(format!(
        "ref '{requested}' was not found as a branch or tag; full 40-character commit SHAs are also accepted"
    ))
}

fn is_full_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn query_remote_refs(context: &OperationContext, remote: &str) -> Result<RemoteRefs, String> {
    let mut command = Command::new("git");
    command.args([
        "ls-remote",
        "--symref",
        remote,
        "HEAD",
        "refs/heads/*",
        "refs/tags/*",
    ]);
    let output = command_output(context, &mut command, "run git ls-remote")
        .map_err(|err| contextual_error(context, "failed to run git ls-remote", err))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-remote failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| "git ls-remote returned non-UTF-8 output".to_string())?;
    let mut default_branch = None;
    let mut branches = BTreeMap::new();
    let mut raw_tags = BTreeMap::new();
    let mut peeled_tags = BTreeMap::new();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("ref: refs/heads/") {
            if let Some((branch, target)) = rest.split_once('\t')
                && target == "HEAD"
            {
                default_branch = Some(branch.to_string());
            }
            continue;
        }
        let Some((sha, reference)) = line.split_once('\t') else {
            continue;
        };
        if let Some(branch) = reference.strip_prefix("refs/heads/") {
            branches.insert(branch.to_string(), sha.to_string());
        } else if let Some(tag) = reference.strip_prefix("refs/tags/") {
            if let Some(tag) = tag.strip_suffix("^{}") {
                peeled_tags.insert(tag.to_string(), sha.to_string());
            } else {
                raw_tags.insert(tag.to_string(), sha.to_string());
            }
        }
    }
    for (tag, sha) in peeled_tags {
        raw_tags.insert(tag, sha);
    }
    let default_branch = default_branch
        .ok_or_else(|| "remote HEAD did not resolve to refs/heads/<branch>".to_string())?;
    Ok(RemoteRefs {
        default_branch,
        branches,
        tags: raw_tags,
    })
}

fn list_remote_refs(
    context: &OperationContext,
    repo: &str,
    remote: &str,
) -> Result<Vec<RefItem>, String> {
    let refs = query_remote_refs(context, remote)?;
    let mut items = Vec::with_capacity(refs.branches.len() + refs.tags.len());
    for (name, commit_sha) in refs.branches {
        items.push(RefItem {
            repo: repo.to_string(),
            resolved_ref: format!("refs/heads/{name}"),
            is_default: name == refs.default_branch,
            name,
            kind: "branch".to_string(),
            commit_sha,
        });
    }
    for (name, commit_sha) in refs.tags {
        items.push(RefItem {
            repo: repo.to_string(),
            resolved_ref: format!("refs/tags/{name}"),
            is_default: false,
            name,
            kind: "tag".to_string(),
            commit_sha,
        });
    }
    Ok(items)
}

fn clone_snapshot(
    context: &OperationContext,
    repo: &str,
    source: &Path,
    expected_sha: &str,
) -> Result<SnapshotRecord, String> {
    let temp_dir = tempfile::Builder::new()
        .prefix("wit-snapshot-")
        .tempdir()
        .map_err(|err| format!("failed to create snapshot directory: {err}"))?;
    let repo_path = temp_dir.path().join("repo.git");
    let mut command = Command::new("git");
    command
        .args(["clone", "--bare", "--no-hardlinks"])
        .arg(source)
        .arg(&repo_path);
    let output = command_output(context, &mut command, "clone immutable snapshot")
        .map_err(|err| contextual_error(context, "failed to clone immutable snapshot", err))?;
    if !output.status.success() {
        return Err(format!(
            "failed to clone immutable snapshot: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let actual = git_stdout_with_context(
        context,
        &repo_path,
        &["rev-parse", "HEAD^{commit}"],
        "verify cloned snapshot",
    )?;
    if actual != expected_sha {
        return Err(format!(
            "snapshot changed while being pinned: expected {expected_sha}, cloned {actual}; retry wit_open"
        ));
    }
    Ok(SnapshotRecord::Disk {
        _temp_dir: temp_dir,
        repo_path,
        repo: repo.to_string(),
        commit_sha: actual,
    })
}

fn fetch_snapshot(
    context: &OperationContext,
    repo: &str,
    remote: &str,
    fetch_ref: &str,
    resolved_ref: &str,
) -> Result<(SnapshotRecord, String), String> {
    let temp_dir = tempfile::Builder::new()
        .prefix("wit-snapshot-")
        .tempdir()
        .map_err(|err| format!("failed to create snapshot directory: {err}"))?;
    let repo_path = temp_dir.path().join("repo.git");
    run_git_with_context(
        context,
        None,
        &["init", "--bare", repo_path.to_string_lossy().as_ref()],
        "initialize snapshot",
    )?;
    run_git_with_context(
        context,
        Some(&repo_path),
        &["fetch", "--depth", "1", remote, fetch_ref],
        "fetch snapshot ref",
    )?;
    let commit_sha = git_stdout_with_context(
        context,
        &repo_path,
        &["rev-parse", "FETCH_HEAD^{commit}"],
        "resolve fetched snapshot commit",
    )?;
    if is_full_sha(resolved_ref) && commit_sha != resolved_ref {
        return Err(format!(
            "remote resolved commit {commit_sha}, not requested commit {resolved_ref}"
        ));
    }
    run_git_with_context(
        context,
        Some(&repo_path),
        &["update-ref", "refs/heads/snapshot", &commit_sha],
        "pin snapshot ref",
    )?;
    run_git_with_context(
        context,
        Some(&repo_path),
        &["symbolic-ref", "HEAD", "refs/heads/snapshot"],
        "set snapshot HEAD",
    )?;
    Ok((
        SnapshotRecord::Disk {
            _temp_dir: temp_dir,
            repo_path,
            repo: repo.to_string(),
            commit_sha: commit_sha.clone(),
        },
        commit_sha,
    ))
}

#[derive(Deserialize)]
struct CacheMetadataView {
    last_checked_at: Option<u64>,
    last_updated_at: Option<u64>,
    last_error: Option<String>,
}

fn cache_provenance(repo_path: &Path, freshness: Freshness) -> CacheProvenance {
    let metadata = repo_path
        .parent()
        .and_then(|parent| std::fs::read(parent.join("metadata.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<CacheMetadataView>(&bytes).ok());
    let state = match freshness {
        Freshness::RequireFresh => "explicitly_refreshed",
        Freshness::AllowStale
            if metadata
                .as_ref()
                .and_then(|value| value.last_error.as_ref())
                .is_some() =>
        {
            "stale_with_error"
        }
        Freshness::AllowStale => "stale_served_revalidating",
    };
    CacheProvenance {
        state: state.to_string(),
        last_checked_at: metadata.as_ref().and_then(|value| value.last_checked_at),
        last_updated_at: metadata.as_ref().and_then(|value| value.last_updated_at),
        last_error: metadata.and_then(|value| value.last_error),
    }
}

#[cfg(test)]
fn run_git(repo_path: Option<&Path>, args: &[&str], action: &str) -> Result<(), String> {
    let mut command = Command::new("git");
    if let Some(repo_path) = repo_path {
        command.arg("-C").arg(repo_path);
    }
    let output = command
        .args(args)
        .output()
        .map_err(|err| format!("failed to {action}: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to {action}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
fn git_stdout(repo_path: &Path, args: &[&str], action: &str) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .output()
        .map_err(|err| format!("failed to {action}: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to {action}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|_| format!("failed to {action}: git returned non-UTF-8 output"))
}

fn run_git_with_context(
    context: &OperationContext,
    repo_path: Option<&Path>,
    args: &[&str],
    action: &str,
) -> Result<(), String> {
    let mut command = Command::new("git");
    if let Some(repo_path) = repo_path {
        command.arg("-C").arg(repo_path);
    }
    command.args(args);
    let output = command_output(context, &mut command, action).map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "failed to {action}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn git_stdout_with_context(
    context: &OperationContext,
    repo_path: &Path,
    args: &[&str],
    action: &str,
) -> Result<String, String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo_path).args(args);
    let output = command_output(context, &mut command, action).map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "failed to {action}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|_| format!("failed to {action}: git returned non-UTF-8 output"))
}

fn normalize_repo_path(path: &str) -> Result<String, String> {
    let normalized = path.trim_matches('/');
    if normalized
        .split('/')
        .any(|part| part == ".." || part == ".")
        || path.starts_with('/')
        || path.contains('\0')
    {
        return Err(
            "path must be repository-relative and cannot contain . or .. components".to_string(),
        );
    }
    Ok(normalized.to_string())
}

fn walk_tree(
    context: &OperationContext,
    snapshot: &SnapshotHandle,
    mut visit: impl FnMut(&TreeEntry) -> Result<bool, String>,
) -> Result<(), String> {
    if let Some(memory) = snapshot.memory() {
        for entry in memory.walk_entries() {
            context.check()?;
            let kind = match entry.kind {
                wit_snapshot::EntryKind::File => "blob",
                wit_snapshot::EntryKind::Dir => "tree",
            };
            let tree_entry = TreeEntry {
                kind: kind.to_string(),
                oid: entry.sha,
                size: entry.size,
                path: entry.path,
            };
            if !visit(&tree_entry)? {
                break;
            }
        }
        return Ok(());
    }
    let SnapshotHandle::Disk { repo_path, .. } = snapshot else {
        return Err("walk_tree requires a disk or memory snapshot".to_string());
    };
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo_path)
        .args(["ls-tree", "-r", "-t", "-z", "-l", "HEAD"]);
    let output = command_output(context, &mut command, "list snapshot tree")
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "failed to list snapshot tree: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    for record in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        context.check()?;
        let text = std::str::from_utf8(record)
            .map_err(|_| "snapshot tree contains a non-UTF-8 path".to_string())?;
        let (metadata, path) = text
            .split_once('\t')
            .ok_or_else(|| format!("unexpected git ls-tree record: {text}"))?;
        let fields = metadata.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 4 {
            return Err(format!("unexpected git ls-tree metadata: {metadata}"));
        }
        let entry = TreeEntry {
            kind: fields[1].to_string(),
            oid: fields[2].to_string(),
            size: fields[3].parse().ok(),
            path: path.to_string(),
        };
        if !visit(&entry)? {
            break;
        }
    }
    Ok(())
}

fn relative_depth(path: &str, base_path: &str) -> Option<usize> {
    let relative = if base_path.is_empty() {
        path
    } else if path == base_path {
        return Some(0);
    } else {
        path.strip_prefix(&format!("{base_path}/"))?
    };
    Some(relative.split('/').count())
}

async fn list_memory_snapshot_window(
    context: &OperationContext,
    snapshot: &SnapshotHandle,
    base_path: &str,
    depth: usize,
    include_metadata: bool,
    offset: usize,
    limit: usize,
) -> Result<Vec<ListItem>, String> {
    let memory = snapshot
        .memory()
        .ok_or_else(|| "list_memory_snapshot_window requires a memory snapshot".to_string())?;
    let mut seen = 0usize;
    let mut items = Vec::with_capacity(limit.min(MAX_PAGE_ITEMS + 1));
    for entry in memory.walk_entries() {
        context.check()?;
        let Some(entry_depth) = relative_depth(&entry.path, base_path) else {
            continue;
        };
        if entry_depth == 0 || entry_depth > depth {
            continue;
        }
        if seen < offset {
            seen += 1;
            continue;
        }
        if items.len() >= limit {
            break;
        }
        let kind = match entry.kind {
            EntryKind::Dir => "directory",
            EntryKind::File => "file",
        };
        let lines = if include_metadata && entry.kind == EntryKind::File && !entry.sha.is_empty() {
            match memory.blob_text_by_sha(&entry.sha, 4 * 1024 * 1024).await {
                Ok(text) => Some(text.lines().count()),
                Err(_) => None,
            }
        } else {
            None
        };
        items.push(ListItem {
            snapshot_id: snapshot.snapshot_id().to_string(),
            repo: snapshot.repo().to_string(),
            commit_sha: snapshot.commit_sha().to_string(),
            path: entry.path,
            kind: kind.to_string(),
            blob_sha: (entry.kind == EntryKind::File).then_some(entry.sha),
            size_bytes: include_metadata.then_some(entry.size).flatten(),
            lines,
        });
        seen += 1;
    }
    Ok(items)
}

async fn read_memory_snapshot_window(
    context: &OperationContext,
    snapshot: &SnapshotHandle,
    path: &str,
    requested_start: usize,
    requested_end: Option<usize>,
    offset: usize,
    limit: usize,
) -> Result<Vec<ReadLineItem>, String> {
    let memory = snapshot
        .memory()
        .ok_or_else(|| "read_memory_snapshot_window requires a memory snapshot".to_string())?;
    let entry = memory
        .entry(path)
        .ok_or_else(|| format!("path not found: {path}"))?;
    if entry.kind != EntryKind::File {
        return Err(format!("path is a directory, not a file: {path}"));
    }
    context.check()?;
    let text = context
        .wait(memory.blob_text_by_sha(&entry.sha, 16 * 1024 * 1024))
        .await?
        .map_err(snapshot_error)?;
    context.check()?;
    let lines = text.lines().collect::<Vec<_>>();
    let end = requested_end.unwrap_or(lines.len()).min(lines.len());
    if requested_start > lines.len().saturating_add(1) {
        return Err(format!(
            "start_line {requested_start} exceeds file length {}",
            lines.len()
        ));
    }
    let page_start = requested_start.saturating_add(offset);
    if page_start > end {
        return Ok(Vec::new());
    }
    Ok((page_start..=end)
        .take(limit)
        .map(|line_number| ReadLineItem {
            snapshot_id: snapshot.snapshot_id().to_string(),
            repo: snapshot.repo().to_string(),
            commit_sha: snapshot.commit_sha().to_string(),
            path: path.to_string(),
            blob_sha: entry.sha.clone(),
            start_line: line_number,
            end_line: line_number,
            text: lines[line_number - 1].to_string(),
        })
        .collect())
}

fn list_snapshot_window(
    context: &OperationContext,
    snapshot: &SnapshotHandle,
    base_path: &str,
    depth: usize,
    include_metadata: bool,
    offset: usize,
    limit: usize,
) -> Result<Vec<ListItem>, String> {
    let mut seen = 0usize;
    let mut items = Vec::with_capacity(limit.min(MAX_PAGE_ITEMS + 1));
    walk_tree(context, snapshot, |entry| {
        let Some(entry_depth) = relative_depth(&entry.path, base_path) else {
            return Ok(true);
        };
        if entry_depth == 0 || entry_depth > depth {
            return Ok(true);
        }
        if seen < offset {
            seen += 1;
            return Ok(true);
        }
        if items.len() >= limit {
            return Ok(false);
        }
        let lines = if include_metadata && entry.kind == "blob" {
            blob_text(context, snapshot, &entry.oid, 4 * 1024 * 1024)
                .ok()
                .map(|text| text.lines().count())
        } else {
            None
        };
        items.push(ListItem {
            snapshot_id: snapshot.snapshot_id().to_string(),
            repo: snapshot.repo().to_string(),
            commit_sha: snapshot.commit_sha().to_string(),
            path: entry.path.clone(),
            kind: if entry.kind == "tree" {
                "directory".to_string()
            } else {
                "file".to_string()
            },
            blob_sha: (entry.kind == "blob").then(|| entry.oid.clone()),
            size_bytes: include_metadata.then_some(entry.size).flatten(),
            lines,
        });
        seen += 1;
        Ok(true)
    })?;
    Ok(items)
}

fn compile_queries(queries: &[String]) -> Result<Vec<(String, Regex)>, String> {
    if queries.is_empty() {
        return Err("queries must contain at least one regular expression".to_string());
    }
    if queries.len() > 20 {
        return Err("queries must contain at most 20 regular expressions".to_string());
    }
    queries
        .iter()
        .map(|query| {
            if query.is_empty() {
                return Err("queries cannot contain an empty expression".to_string());
            }
            Regex::new(query)
                .map(|regex| (query.clone(), regex))
                .map_err(|err| format!("invalid query regex '{query}': {err}"))
        })
        .collect()
}

fn compile_globs(globs: &[String]) -> Result<Option<GlobSet>, String> {
    if globs.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for glob in globs {
        builder.add(Glob::new(glob).map_err(|err| format!("invalid glob '{glob}': {err}"))?);
    }
    builder
        .build()
        .map(Some)
        .map_err(|err| format!("failed to compile globs: {err}"))
}

fn path_has_prefix(path: &str, prefix: &str) -> bool {
    prefix.is_empty() || path == prefix || path.starts_with(&format!("{prefix}/"))
}

struct SearchPathFilters {
    includes: Option<GlobSet>,
    prefix: String,
    excludes: Option<GlobSet>,
}

fn search_snapshot_window(
    context: &OperationContext,
    snapshot: &SnapshotHandle,
    queries: &[(String, Regex)],
    filters: &SearchPathFilters,
    context_lines: usize,
    offset: usize,
    limit: usize,
) -> Result<Vec<SearchItem>, String> {
    let mut seen = 0usize;
    let mut items = Vec::with_capacity(limit.min(MAX_CONTEXT_CANDIDATES));
    walk_tree(context, snapshot, |entry| {
        if entry.kind != "blob"
            || !path_has_prefix(&entry.path, &filters.prefix)
            || filters
                .includes
                .as_ref()
                .is_some_and(|set| !set.is_match(&entry.path))
            || filters
                .excludes
                .as_ref()
                .is_some_and(|set| set.is_match(&entry.path))
        {
            return Ok(true);
        }
        let Some(size) = entry.size else {
            return Ok(true);
        };
        if size > 4 * 1024 * 1024 {
            return Ok(true);
        }
        let Ok(text) = blob_text(context, snapshot, &entry.oid, 4 * 1024 * 1024) else {
            return Ok(true);
        };
        let lines = text.lines().collect::<Vec<_>>();
        for (line_index, line) in lines.iter().enumerate() {
            for (query, regex) in queries {
                if !regex.is_match(line) {
                    continue;
                }
                if seen < offset {
                    seen += 1;
                    continue;
                }
                if items.len() >= limit {
                    return Ok(false);
                }
                let match_line = line_index + 1;
                let start_line = match_line.saturating_sub(context_lines).max(1);
                let end_line = (match_line + context_lines).min(lines.len());
                let source_lines = (start_line..=end_line)
                    .map(|line_number| SourceLine {
                        line_number,
                        text: lines[line_number - 1].to_string(),
                    })
                    .collect();
                items.push(SearchItem {
                    snapshot_id: snapshot.snapshot_id().to_string(),
                    repo: snapshot.repo().to_string(),
                    commit_sha: snapshot.commit_sha().to_string(),
                    path: entry.path.clone(),
                    blob_sha: entry.oid.clone(),
                    query: query.clone(),
                    match_line,
                    start_line,
                    end_line,
                    lines: source_lines,
                });
                seen += 1;
            }
        }
        Ok(true)
    })?;
    Ok(items)
}

async fn search_memory_snapshot_window(
    context: &OperationContext,
    snapshot: &SnapshotHandle,
    queries: &[(String, Regex)],
    filters: &SearchPathFilters,
    context_lines: usize,
    offset: usize,
    limit: usize,
) -> Result<Vec<SearchItem>, String> {
    let memory = snapshot
        .memory()
        .ok_or_else(|| "search_memory_snapshot_window requires a memory snapshot".to_string())?;
    let mut seen = 0usize;
    let mut items = Vec::with_capacity(limit.min(MAX_CONTEXT_CANDIDATES));
    let max_blob = 4 * 1024 * 1024u64;
    for entry in memory.walk_entries() {
        context.check()?;
        if entry.kind != wit_snapshot::EntryKind::File
            || !path_has_prefix(&entry.path, &filters.prefix)
            || filters
                .includes
                .as_ref()
                .is_some_and(|set| !set.is_match(&entry.path))
            || filters
                .excludes
                .as_ref()
                .is_some_and(|set| set.is_match(&entry.path))
        {
            continue;
        }
        let Some(size) = entry.size else {
            continue;
        };
        if size > max_blob {
            continue;
        }
        let Ok(text) = memory.blob_text_by_sha(&entry.sha, max_blob).await else {
            continue;
        };
        let lines = text.lines().collect::<Vec<_>>();
        for (line_index, line) in lines.iter().enumerate() {
            for (query, regex) in queries {
                if !regex.is_match(line) {
                    continue;
                }
                if seen < offset {
                    seen += 1;
                    continue;
                }
                if items.len() >= limit {
                    return Ok(items);
                }
                let match_line = line_index + 1;
                let start_line = match_line.saturating_sub(context_lines).max(1);
                let end_line = (match_line + context_lines).min(lines.len());
                let source_lines = (start_line..=end_line)
                    .map(|line_number| SourceLine {
                        line_number,
                        text: lines[line_number - 1].to_string(),
                    })
                    .collect();
                items.push(SearchItem {
                    snapshot_id: snapshot.snapshot_id().to_string(),
                    repo: snapshot.repo().to_string(),
                    commit_sha: snapshot.commit_sha().to_string(),
                    path: entry.path.clone(),
                    blob_sha: entry.sha.clone(),
                    query: query.clone(),
                    match_line,
                    start_line,
                    end_line,
                    lines: source_lines,
                });
                seen += 1;
            }
        }
    }
    Ok(items)
}

fn read_snapshot_window(
    context: &OperationContext,
    snapshot: &SnapshotHandle,
    path: &str,
    requested_start: usize,
    requested_end: Option<usize>,
    offset: usize,
    limit: usize,
) -> Result<Vec<ReadLineItem>, String> {
    let SnapshotHandle::Disk { repo_path, .. } = snapshot else {
        return Err("read_snapshot_window requires a disk snapshot".to_string());
    };
    let oid = git_stdout_with_context(
        context,
        repo_path,
        &["rev-parse", &format!("HEAD:{path}")],
        "resolve file blob",
    )?;
    let text = blob_text(context, snapshot, &oid, 16 * 1024 * 1024)?;
    let lines = text.lines().collect::<Vec<_>>();
    let end = requested_end.unwrap_or(lines.len()).min(lines.len());
    if requested_start > lines.len().saturating_add(1) {
        return Err(format!(
            "start_line {requested_start} exceeds file length {}",
            lines.len()
        ));
    }
    let page_start = requested_start.saturating_add(offset);
    if page_start > end {
        return Ok(Vec::new());
    }
    Ok((page_start..=end)
        .take(limit)
        .map(|line_number| ReadLineItem {
            snapshot_id: snapshot.snapshot_id().to_string(),
            repo: snapshot.repo().to_string(),
            commit_sha: snapshot.commit_sha().to_string(),
            path: path.to_string(),
            blob_sha: oid.clone(),
            start_line: line_number,
            end_line: line_number,
            text: lines[line_number - 1].to_string(),
        })
        .collect())
}

fn blob_text(
    context: &OperationContext,
    snapshot: &SnapshotHandle,
    oid: &str,
    max_bytes: usize,
) -> Result<String, String> {
    let SnapshotHandle::Disk { repo_path, .. } = snapshot else {
        return Err("blob_text requires a disk snapshot".to_string());
    };
    let size = git_stdout_with_context(
        context,
        repo_path,
        &["cat-file", "-s", oid],
        "read blob size",
    )?
    .parse::<usize>()
    .map_err(|_| "git cat-file returned an invalid blob size".to_string())?;
    if size > max_bytes {
        return Err(format!(
            "blob {oid} is {size} bytes, above this operation's {max_bytes}-byte safety limit"
        ));
    }
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo_path)
        .args(["cat-file", "blob", oid]);
    let output = command_output(context, &mut command, "read snapshot blob")
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "failed to read blob {oid}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if output.stdout.contains(&0) {
        return Err(format!("blob {oid} is binary"));
    }
    String::from_utf8(output.stdout).map_err(|_| format!("blob {oid} is not valid UTF-8"))
}

fn rank_context(mut matches: Vec<SearchItem>, queries: &[String]) -> Vec<ContextItem> {
    matches.sort_by(|left, right| {
        (
            &left.path,
            left.start_line,
            left.end_line,
            left.match_line,
            &left.query,
        )
            .cmp(&(
                &right.path,
                right.start_line,
                right.end_line,
                right.match_line,
                &right.query,
            ))
    });
    let mut merged: Vec<ContextItem> = Vec::new();
    for item in matches {
        if let Some(previous) = merged.last_mut()
            && previous.path == item.path
            && previous.blob_sha == item.blob_sha
            && item.start_line <= previous.end_line.saturating_add(1)
        {
            previous.end_line = previous.end_line.max(item.end_line);
            previous.score += 1;
            if !previous.queries.contains(&item.query) {
                previous.queries.push(item.query);
            }
            let mut lines = previous
                .lines
                .iter()
                .cloned()
                .map(|line| (line.line_number, line))
                .collect::<BTreeMap<_, _>>();
            for line in item.lines {
                lines.entry(line.line_number).or_insert(line);
            }
            previous.lines = lines.into_values().collect();
            continue;
        }
        merged.push(ContextItem {
            snapshot_id: item.snapshot_id,
            repo: item.repo,
            commit_sha: item.commit_sha,
            path: item.path,
            blob_sha: item.blob_sha,
            start_line: item.start_line,
            end_line: item.end_line,
            score: 1,
            ranking_reasons: Vec::new(),
            queries: vec![item.query],
            lines: item.lines,
        });
    }

    for item in &mut merged {
        let hit_count = item.score;
        let exact_hits = queries
            .iter()
            .filter(|query| {
                item.lines
                    .iter()
                    .any(|line| line.text.contains(query.as_str()))
            })
            .count() as i64;
        let path_hits = queries
            .iter()
            .filter(|query| {
                item.path
                    .to_ascii_lowercase()
                    .contains(&query.to_ascii_lowercase())
            })
            .count() as i64;
        let position_bonus = 20_i64.saturating_sub((item.start_line / 100) as i64);
        item.score = exact_hits * 100 + path_hits * 25 + hit_count * 10 + position_bonus;
        if exact_hits > 0 {
            item.ranking_reasons
                .push(format!("{exact_hits} exact query text hit(s)"));
        }
        if path_hits > 0 {
            item.ranking_reasons
                .push(format!("{path_hits} path relevance hit(s)"));
        }
        item.ranking_reasons
            .push(format!("{hit_count} match(es) in merged window"));
        item.ranking_reasons.push(format!(
            "source position starts at line {}",
            item.start_line
        ));
        item.queries.sort();
    }
    merged.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.start_line.cmp(&right.start_line))
    });
    merged
}

fn ast_item_from_symbol(
    snapshot: &SnapshotHandle,
    path: &str,
    oid: &str,
    language: AstLanguage,
    symbol: AstSymbol,
) -> AstItem {
    AstItem {
        snapshot_id: snapshot.snapshot_id().to_string(),
        repo: snapshot.repo().to_string(),
        commit_sha: snapshot.commit_sha().to_string(),
        path: path.to_string(),
        blob_sha: oid.to_string(),
        language: language.name().to_string(),
        kind: symbol.kind,
        name: symbol.name,
        start_line: symbol.start_line,
        end_line: symbol.end_line,
        start_col: symbol.start_col,
        end_col: symbol.end_col,
        parent: symbol.parent,
        depth: symbol.depth,
        signature: symbol.signature,
        capture: None,
        pattern_index: None,
        match_index: None,
    }
}

fn ast_item_from_capture(
    snapshot: &SnapshotHandle,
    path: &str,
    oid: &str,
    language: AstLanguage,
    capture: AstCapture,
) -> AstItem {
    AstItem {
        snapshot_id: snapshot.snapshot_id().to_string(),
        repo: snapshot.repo().to_string(),
        commit_sha: snapshot.commit_sha().to_string(),
        path: path.to_string(),
        blob_sha: oid.to_string(),
        language: language.name().to_string(),
        kind: capture.node_kind,
        name: capture.text.clone(),
        start_line: capture.start_line,
        end_line: capture.end_line,
        start_col: capture.start_col,
        end_col: capture.end_col,
        parent: None,
        depth: 0,
        signature: capture.text,
        capture: Some(capture.capture),
        pattern_index: Some(capture.pattern_index),
        match_index: Some(capture.match_index),
    }
}

fn render_ast_items(items: &[AstItem]) -> String {
    items
        .iter()
        .map(|item| match &item.capture {
            Some(capture) => format!(
                "{}:{}:{}: @{} ({}) {}",
                item.path,
                item.start_line,
                item.start_col + 1,
                capture,
                item.kind,
                item.name
            ),
            None => format!(
                "{}:{}-{}: {}{} {}",
                item.path,
                item.start_line,
                item.end_line,
                "  ".repeat(item.depth),
                item.kind,
                item.name
            ),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_search_items(items: &[SearchItem]) -> String {
    items
        .iter()
        .map(|item| {
            let lines = item
                .lines
                .iter()
                .map(|line| format!("{}: {}", line.line_number, line.text))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{}:{} [{}]\n{}",
                item.path, item.match_line, item.query, lines
            )
        })
        .collect::<Vec<_>>()
        .join("\n--\n")
}

fn render_read_items(items: &[ReadLineItem], number_lines: bool) -> String {
    items
        .iter()
        .map(|item| {
            if number_lines {
                format!("{}\t{}", item.start_line, item.text)
            } else {
                item.text.clone()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_context_items(items: &[ContextItem]) -> String {
    items
        .iter()
        .map(|item| {
            let lines = item
                .lines
                .iter()
                .map(|line| format!("{}: {}", line.line_number, line.text))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{}:{}-{} score={}\n{}",
                item.path, item.start_line, item.end_line, item.score, lines
            )
        })
        .collect::<Vec<_>>()
        .join("\n--\n")
}

#[allow(clippy::too_many_arguments)]
fn paginate_vec<T, F>(
    tool: &str,
    snapshot_id: Option<&str>,
    fingerprint: &str,
    cursor: Option<&str>,
    items: Vec<T>,
    max_items: usize,
    max_bytes: usize,
    include_rendered_text: bool,
    render: F,
) -> Result<Page<T>, String>
where
    T: Clone + Serialize,
    F: Fn(&[T]) -> String,
{
    let offset = cursor_offset(tool, snapshot_id, fingerprint, cursor)?;
    paginate_vec_with_offset(
        tool,
        snapshot_id,
        fingerprint,
        offset,
        items,
        max_items,
        max_bytes,
        include_rendered_text,
        render,
    )
}

#[allow(clippy::too_many_arguments)]
fn paginate_vec_with_offset<T, F>(
    tool: &str,
    snapshot_id: Option<&str>,
    fingerprint: &str,
    offset: usize,
    items: Vec<T>,
    max_items: usize,
    max_bytes: usize,
    include_rendered_text: bool,
    render: F,
) -> Result<Page<T>, String>
where
    T: Clone + Serialize,
    F: Fn(&[T]) -> String,
{
    let window = items.into_iter().skip(offset).take(max_items + 1).collect();
    paginate_window(
        tool,
        snapshot_id,
        fingerprint,
        offset,
        window,
        max_items,
        max_bytes,
        include_rendered_text,
        render,
    )
}

#[allow(clippy::too_many_arguments)]
fn paginate_window<T, F>(
    tool: &str,
    snapshot_id: Option<&str>,
    fingerprint: &str,
    offset: usize,
    mut window: Vec<T>,
    max_items: usize,
    max_bytes: usize,
    include_rendered_text: bool,
    render: F,
) -> Result<Page<T>, String>
where
    T: Clone + Serialize,
    F: Fn(&[T]) -> String,
{
    let mut has_more = window.len() > max_items;
    window.truncate(max_items);
    loop {
        let next_cursor = if has_more && !window.is_empty() {
            Some(encode_cursor(&CursorToken {
                version: 1,
                tool: tool.to_string(),
                snapshot_id: snapshot_id.map(str::to_string),
                fingerprint: fingerprint.to_string(),
                offset: offset + window.len(),
            })?)
        } else {
            None
        };
        let rendered_text = include_rendered_text.then(|| render(&window));
        let mut page = Page {
            api_version: "2".to_string(),
            returned_items: window.len(),
            items: window.clone(),
            has_more,
            next_cursor,
            budget: BudgetInfo {
                requested_bytes: max_bytes,
                serialized_bytes: 0,
                remaining_bytes: max_bytes,
                warning: None,
            },
            rendered_text,
        };
        stabilize_serialized_size(&mut page)?;
        if page.budget.serialized_bytes <= max_bytes {
            if page.items.is_empty() && page.has_more {
                return Err(format!(
                    "max_bytes {max_bytes} cannot fit one {tool} item plus its provenance envelope"
                ));
            }
            return Ok(page);
        }
        if window.pop().is_none() {
            return Err(format!(
                "max_bytes {max_bytes} is too small for the {tool} response envelope"
            ));
        }
        has_more = true;
    }
}

fn stabilize_serialized_size<T: Serialize>(page: &mut Page<T>) -> Result<(), String> {
    for _ in 0..8 {
        let size = serde_json::to_vec(page)
            .map_err(|err| format!("failed to serialize MCP response: {err}"))?
            .len();
        if page.budget.serialized_bytes == size {
            return Ok(());
        }
        update_budget(&mut page.budget, size);
    }
    Err("MCP response size metadata did not stabilize".to_string())
}

fn stabilize_compact_size<T: Serialize>(
    response: &mut T,
    mut budget: impl FnMut(&mut T) -> &mut BudgetInfo,
) -> Result<(), String> {
    for _ in 0..8 {
        let size = serde_json::to_vec(response)
            .map_err(|err| format!("failed to serialize MCP response: {err}"))?
            .len();
        if budget(response).serialized_bytes == size {
            return Ok(());
        }
        update_budget(budget(response), size);
    }
    Err("MCP response size metadata did not stabilize".to_string())
}

fn update_budget(budget: &mut BudgetInfo, serialized_bytes: usize) {
    budget.serialized_bytes = serialized_bytes;
    budget.remaining_bytes = budget.requested_bytes.saturating_sub(serialized_bytes);
    budget.warning = (serialized_bytes.saturating_mul(5)
        >= budget.requested_bytes.saturating_mul(4))
    .then(|| {
        "response is near max_bytes; return fewer fields or continue with next_cursor".to_string()
    });
}

fn reset_budget(mut budget: BudgetInfo) -> BudgetInfo {
    budget.serialized_bytes = 0;
    budget.remaining_bytes = budget.requested_bytes;
    budget.warning = None;
    budget
}

fn compact_list_page(
    page: Page<ListItem>,
    snapshot: &SnapshotHandle,
) -> Result<CompactListPage, String> {
    let mut response = CompactListPage {
        api_version: page.api_version,
        format: CompactListFormat::Paths,
        snapshot_id: snapshot.snapshot_id().to_string(),
        repo: snapshot.repo().to_string(),
        commit_sha: snapshot.commit_sha().to_string(),
        paths: page.items.into_iter().map(|item| item.path).collect(),
        returned_items: page.returned_items,
        has_more: page.has_more,
        next_cursor: page.next_cursor,
        budget: reset_budget(page.budget),
    };
    stabilize_compact_size(&mut response, |response| &mut response.budget)?;
    Ok(response)
}

fn compact_read_text_page(
    page: Page<ReadLineItem>,
    snapshot: &SnapshotHandle,
    path: &str,
) -> Result<CompactReadTextPage, String> {
    let first = page.items.first();
    let mut response = CompactReadTextPage {
        api_version: page.api_version,
        format: CompactReadTextFormat::Text,
        snapshot_id: snapshot.snapshot_id().to_string(),
        repo: snapshot.repo().to_string(),
        commit_sha: snapshot.commit_sha().to_string(),
        path: path.to_string(),
        blob_sha: first.map(|item| item.blob_sha.clone()),
        start_line: first.map(|item| item.start_line),
        end_line: page.items.last().map(|item| item.end_line),
        text: page
            .items
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        returned_lines: page.returned_items,
        has_more: page.has_more,
        next_cursor: page.next_cursor,
        budget: reset_budget(page.budget),
    };
    stabilize_compact_size(&mut response, |response| &mut response.budget)?;
    Ok(response)
}

fn compact_read_lines_page(
    page: Page<ReadLineItem>,
    snapshot: &SnapshotHandle,
    path: &str,
) -> Result<CompactReadLinesPage, String> {
    let first = page.items.first();
    let mut response = CompactReadLinesPage {
        api_version: page.api_version,
        format: CompactReadLinesFormat::Lines,
        snapshot_id: snapshot.snapshot_id().to_string(),
        repo: snapshot.repo().to_string(),
        commit_sha: snapshot.commit_sha().to_string(),
        path: path.to_string(),
        blob_sha: first.map(|item| item.blob_sha.clone()),
        start_line: first.map(|item| item.start_line),
        end_line: page.items.last().map(|item| item.end_line),
        lines: page
            .items
            .into_iter()
            .map(|item| SourceLine {
                line_number: item.start_line,
                text: item.text,
            })
            .collect(),
        returned_lines: page.returned_items,
        has_more: page.has_more,
        next_cursor: page.next_cursor,
        budget: reset_budget(page.budget),
    };
    stabilize_compact_size(&mut response, |response| &mut response.budget)?;
    Ok(response)
}

fn fingerprint(value: &serde_json::Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|err| format!("failed to normalize cursor arguments: {err}"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn encode_cursor(cursor: &CursorToken) -> Result<String, String> {
    let bytes = serde_json::to_vec(cursor)
        .map_err(|err| format!("failed to encode continuation cursor: {err}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn decode_cursor(value: &str) -> Result<CursorToken, String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| "invalid continuation cursor encoding".to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "invalid continuation cursor payload".to_string())
}

fn cursor_offset(
    tool: &str,
    snapshot_id: Option<&str>,
    fingerprint: &str,
    cursor: Option<&str>,
) -> Result<usize, String> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let decoded = decode_cursor(cursor)?;
    if decoded.version != 1
        || decoded.tool != tool
        || decoded.snapshot_id.as_deref() != snapshot_id
        || decoded.fingerprint != fingerprint
    {
        return Err(
            "continuation cursor does not match this tool, snapshot, or normalized arguments"
                .to_string(),
        );
    }
    Ok(decoded.offset)
}

fn validate_budget(value: Option<usize>) -> Result<usize, String> {
    let value = value.unwrap_or(DEFAULT_BUDGET_BYTES);
    if !(MIN_BUDGET_BYTES..=MAX_BUDGET_BYTES).contains(&value) {
        return Err(format!(
            "max_bytes must be between {MIN_BUDGET_BYTES} and {MAX_BUDGET_BYTES}"
        ));
    }
    Ok(value)
}

fn validate_page_items(value: Option<usize>) -> Result<usize, String> {
    validate_range(value, DEFAULT_PAGE_ITEMS, MAX_PAGE_ITEMS, "max_items")
}

fn validate_range(
    value: Option<usize>,
    default: usize,
    max: usize,
    name: &str,
) -> Result<usize, String> {
    let value = value.unwrap_or(default);
    if value == 0 || value > max {
        return Err(format!("{name} must be between 1 and {max}"));
    }
    Ok(value)
}

fn anyhow_error(err: anyhow::Error) -> String {
    format!("{err:#}")
}

fn snapshot_error(err: SnapshotError) -> String {
    err.to_string()
}

fn contextual_error(
    context: &OperationContext,
    action: &str,
    error: impl std::fmt::Display,
) -> String {
    context
        .check()
        .err()
        .unwrap_or_else(|| format!("{action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{Duration, Instant},
    };

    fn operations_with_snapshot() -> (WitOperations, String) {
        let temp_dir = tempfile::tempdir().unwrap();
        let repo_path = temp_dir.path().join("repo");
        fs::create_dir(&repo_path).unwrap();
        run_git(Some(&repo_path), &["init"], "initialize test repository").unwrap();
        run_git(
            Some(&repo_path),
            &["config", "user.name", "Wit Tests"],
            "configure test author",
        )
        .unwrap();
        run_git(
            Some(&repo_path),
            &["config", "user.email", "wit-tests@example.invalid"],
            "configure test email",
        )
        .unwrap();
        fs::create_dir(repo_path.join("src")).unwrap();
        fs::write(
            repo_path.join("src/lib.rs"),
            "pub fn answer() -> u8 {\n    42\n}\n",
        )
        .unwrap();
        run_git(Some(&repo_path), &["add", "."], "stage test fixture").unwrap();
        run_git(
            Some(&repo_path),
            &["commit", "-m", "fixture"],
            "commit test fixture",
        )
        .unwrap();
        let commit_sha = git_stdout(
            &repo_path,
            &["rev-parse", "HEAD^{commit}"],
            "resolve test commit",
        )
        .unwrap();
        let id = snapshot_id("owner/repo", &commit_sha);
        let operations = WitOperations::new();
        operations.snapshots.lock().unwrap().insert(
            id.clone(),
            SnapshotRecord::Disk {
                _temp_dir: temp_dir,
                repo_path,
                repo: "owner/repo".to_string(),
                commit_sha,
            },
        );
        (operations, id)
    }

    fn operations_with_memory_snapshot() -> (WitOperations, String) {
        let tree = serde_json::json!({
            "sha": "treesha",
            "truncated": false,
            "tree": [
                {"path": "README.md", "mode": "100644", "type": "blob", "sha": "blob-readme", "size": 12},
                {"path": "src", "mode": "040000", "type": "tree", "sha": "tree-src"},
                {"path": "src/lib.rs", "mode": "100644", "type": "blob", "sha": "blob-lib", "size": 28}
            ]
        })
        .to_string();
        let client = ReqwestGitHubClient::new("http://127.0.0.1:9", None).unwrap();
        let snap = snapshot_from_tree_json(
            Arc::new(client),
            "owner/repo",
            "main",
            "abc123memory",
            "treesha",
            &tree,
            MemoryBackendLimits::default(),
        )
        .unwrap();
        snap.preload_blob("blob-readme", b"hello world\n".to_vec())
            .unwrap();
        snap.preload_blob("blob-lib", b"pub fn answer() -> u8 {\n    42\n}\n".to_vec())
            .unwrap();
        let id = snapshot_id("owner/repo", "abc123memory");
        let operations = WitOperations::with_backend(CliSnapshotBackend::Memory);
        operations.snapshots.lock().unwrap().insert(
            id.clone(),
            SnapshotRecord::Memory {
                snapshot: Arc::new(snap),
                snapshot_id: id.clone(),
            },
        );
        (operations, id)
    }

    #[tokio::test]
    async fn memory_snapshot_list_and_read_without_disk() {
        let probe = tempfile::tempdir().unwrap();
        let previous_cache = std::env::var_os("WIT_CACHE_DIR");
        // Safety: scoped env override restored before the test returns.
        unsafe {
            std::env::set_var("WIT_CACHE_DIR", probe.path());
        }
        let (operations, snapshot_id) = operations_with_memory_snapshot();
        let listed = operations
            .list(
                &OperationContext::default(),
                ListArgs {
                    snapshot_id: snapshot_id.clone(),
                    path: None,
                    depth: Some(2),
                    include_metadata: true,
                    format: ListFormat::Structured,
                    cursor: None,
                    max_items: None,
                    max_bytes: None,
                    include_rendered_text: false,
                },
            )
            .await
            .unwrap();
        let ListResponse::Structured(page) = listed else {
            panic!("expected structured list");
        };
        let paths: Vec<_> = page.items.iter().map(|item| item.path.as_str()).collect();
        assert!(paths.contains(&"README.md"));
        assert!(paths.contains(&"src"));
        assert!(paths.contains(&"src/lib.rs"));

        let read = operations
            .read(
                &OperationContext::default(),
                ReadArgs {
                    snapshot_id,
                    path: "README.md".to_string(),
                    start_line: Some(1),
                    end_line: Some(1),
                    number_lines: false,
                    format: ReadFormat::Text,
                    cursor: None,
                    max_lines: None,
                    max_bytes: None,
                    include_rendered_text: false,
                },
            )
            .await
            .unwrap();
        let ReadResponse::Text(page) = read else {
            panic!("expected text read");
        };
        assert_eq!(page.text, "hello world");
        assert!(
            std::fs::read_dir(probe.path()).unwrap().next().is_none(),
            "memory MCP path must not write WIT_CACHE_DIR"
        );
        // Safety: restore previous value so parallel tests are not polluted.
        unsafe {
            match previous_cache {
                Some(value) => std::env::set_var("WIT_CACHE_DIR", value),
                None => std::env::remove_var("WIT_CACHE_DIR"),
            }
        }
    }

    #[tokio::test]
    async fn memory_search_code_without_disk() {
        let probe = tempfile::tempdir().unwrap();
        let previous_cache = std::env::var_os("WIT_CACHE_DIR");
        // Safety: scoped env override restored before the test returns.
        unsafe {
            std::env::set_var("WIT_CACHE_DIR", probe.path());
        }
        let (operations, snapshot_id) = operations_with_memory_snapshot();
        let page = operations
            .search_code(
                &OperationContext::default(),
                SearchCodeArgs {
                    snapshot_id: snapshot_id.clone(),
                    queries: vec!["hello".to_string()],
                    ..SearchCodeArgs::default()
                },
            )
            .await
            .unwrap();
        assert!(
            page.items
                .iter()
                .any(|item| item.path == "README.md" && item.match_line == 1),
            "expected README.md match, got {:?}",
            page.items
        );

        let context_page = operations
            .context(
                &OperationContext::default(),
                ContextArgs {
                    snapshot_id,
                    queries: vec!["answer".to_string()],
                    ..ContextArgs::default()
                },
            )
            .await
            .unwrap();
        assert!(
            context_page
                .items
                .iter()
                .any(|item| item.path == "src/lib.rs"),
            "expected src/lib.rs context hit"
        );
        assert!(
            std::fs::read_dir(probe.path()).unwrap().next().is_none(),
            "memory wit_search_code must not write WIT_CACHE_DIR"
        );
        // Safety: restore previous value so parallel tests are not polluted.
        unsafe {
            match previous_cache {
                Some(value) => std::env::set_var("WIT_CACHE_DIR", value),
                None => std::env::remove_var("WIT_CACHE_DIR"),
            }
        }
    }

    #[tokio::test]
    async fn memory_wit_open_surfaces_typed_rate_limit_without_cache_writes() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };

        let probe = tempfile::tempdir().unwrap();
        let previous_cache = std::env::var_os("WIT_CACHE_DIR");
        // Safety: scoped env override restored before the test returns.
        unsafe {
            std::env::set_var("WIT_CACHE_DIR", probe.path());
        }

        let server = MockServer::start().await;
        let body = r#"{"message":"API rate limit exceeded for anonymous requests"}"#;
        Mock::given(method("GET"))
            .and(path("/repos/acme/demo"))
            .respond_with(ResponseTemplate::new(403).set_body_string(body))
            .mount(&server)
            .await;

        let client = ReqwestGitHubClient::new(server.uri(), None).unwrap();
        let operations = WitOperations::with_memory_github_client(client);
        let error = operations
            .open(
                &OperationContext::default(),
                OpenArgs {
                    repo: "acme/demo".to_string(),
                    reference: None,
                    freshness: Freshness::AllowStale,
                },
            )
            .await
            .expect_err("rate-limited open must fail");

        let expected = SnapshotError::from_status(403, body, "acme/demo");
        assert!(
            matches!(expected, SnapshotError::RateLimited(_)),
            "fixture must map to RateLimited, got {expected:?}"
        );
        assert_eq!(error, expected.to_string());
        assert!(
            operations.snapshots.lock().unwrap().is_empty(),
            "failed open must not publish a snapshot"
        );
        assert!(
            std::fs::read_dir(probe.path()).unwrap().next().is_none(),
            "memory wit_open must not write WIT_CACHE_DIR on failure"
        );

        // Safety: restore previous value so parallel tests are not polluted.
        unsafe {
            match previous_cache {
                Some(value) => std::env::set_var("WIT_CACHE_DIR", value),
                None => std::env::remove_var("WIT_CACHE_DIR"),
            }
        }
    }

    #[tokio::test]
    async fn typed_service_reads_snapshot_shared_by_clones_without_mcp() {
        let (operations, snapshot_id) = operations_with_snapshot();
        let second_adapter = operations.clone();
        let page = second_adapter
            .list(
                &OperationContext::default(),
                ListArgs {
                    snapshot_id: snapshot_id.clone(),
                    path: Some("src".to_string()),
                    depth: Some(1),
                    ..ListArgs::default()
                },
            )
            .await
            .unwrap();
        let ListResponse::Structured(page) = page else {
            panic!("default list format must stay structured");
        };
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].path, "src/lib.rs");
        assert_eq!(page.items[0].snapshot_id, snapshot_id);

        let read = operations
            .read(
                &OperationContext::default(),
                ReadArgs {
                    snapshot_id,
                    path: "src/lib.rs".to_string(),
                    start_line: Some(2),
                    end_line: Some(2),
                    ..ReadArgs::default()
                },
            )
            .await
            .unwrap();
        let ReadResponse::Structured(read) = read else {
            panic!("default read format must stay structured");
        };
        assert_eq!(read.items.len(), 1);
        assert_eq!(read.items[0].text, "    42");
    }

    #[tokio::test]
    async fn operation_context_rejects_cancellation_and_expired_deadlines() {
        let cancellation = OperationCancellation::default();
        let cancelled = OperationContext::new(None, cancellation.clone());
        cancellation.cancel();
        assert_eq!(cancelled.check().unwrap_err(), "operation cancelled");

        let expired = OperationContext::with_deadline(Instant::now() - Duration::from_millis(1));
        assert_eq!(expired.check().unwrap_err(), "operation deadline exceeded");

        let error = WitOperations::new()
            .list(
                &expired,
                ListArgs {
                    snapshot_id: "owner/repo@deadbeef".to_string(),
                    ..ListArgs::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error, "operation deadline exceeded");
    }

    #[test]
    fn cancelled_snapshot_clone_returns_stable_error_without_a_record() {
        let (operations, snapshot_id) = operations_with_snapshot();
        let source = operations.snapshot(&snapshot_id).unwrap();
        let cancellation = OperationCancellation::default();
        let context = OperationContext::new(None, cancellation.clone());
        cancellation.cancel();

        let error = match clone_snapshot(
            &context,
            "owner/cancelled",
            source.require_disk_path().unwrap(),
            source.commit_sha(),
        ) {
            Ok(_) => panic!("cancelled snapshot must not be created"),
            Err(error) => error,
        };

        assert_eq!(error, "operation cancelled");
        assert_eq!(operations.snapshots.lock().unwrap().len(), 1);
    }

    #[test]
    fn same_snapshot_reuse_is_not_removed_by_late_owner_cancellation() {
        let operations = WitOperations::new();
        let snapshot_id = "owner/repo@abc".to_string();
        let cancellation = OperationCancellation::default();
        let first_context = OperationContext::new(None, cancellation.clone());
        let second_context = OperationContext::default();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut workers = Vec::new();
        for (context, suffix) in [(first_context.clone(), "first"), (second_context, "second")] {
            let operations = operations.clone();
            let snapshot_id = snapshot_id.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                let temp_dir = tempfile::tempdir().unwrap();
                let repo_path = temp_dir.path().join(suffix);
                fs::create_dir(&repo_path).unwrap();
                barrier.wait();
                operations.publish_snapshot(
                    &context,
                    snapshot_id,
                    SnapshotRecord::Disk {
                        _temp_dir: temp_dir,
                        repo_path,
                        repo: "owner/repo".to_string(),
                        commit_sha: "abc".to_string(),
                    },
                )
            }));
        }
        barrier.wait();
        for worker in workers {
            worker.join().unwrap().unwrap();
        }
        cancellation.cancel();

        assert_eq!(first_context.check().unwrap_err(), "operation cancelled");
        assert!(operations.snapshot(&snapshot_id).is_ok());
        assert_eq!(operations.snapshots.lock().unwrap().len(), 1);
    }

    #[test]
    fn cursor_rejects_changed_query_or_snapshot() {
        let token = encode_cursor(&CursorToken {
            version: 1,
            tool: "wit_list".to_string(),
            snapshot_id: Some("owner/repo@abc".to_string()),
            fingerprint: "same".to_string(),
            offset: 10,
        })
        .unwrap();
        assert_eq!(
            cursor_offset("wit_list", Some("owner/repo@abc"), "same", Some(&token)).unwrap(),
            10
        );
        assert!(
            cursor_offset(
                "wit_list",
                Some("owner/repo@different"),
                "same",
                Some(&token)
            )
            .unwrap_err()
            .contains("does not match")
        );
        assert!(
            cursor_offset("wit_list", Some("owner/repo@abc"), "changed", Some(&token))
                .unwrap_err()
                .contains("does not match")
        );
    }

    #[test]
    fn whole_response_budget_shrinks_items_and_reports_exact_size() {
        let items = (0..100)
            .map(|index| format!("item-{index}-{}", "x".repeat(100)))
            .collect::<Vec<_>>();
        let page = paginate_vec(
            "test",
            None,
            "fingerprint",
            None,
            items,
            100,
            2048,
            false,
            |items| items.join("\n"),
        )
        .unwrap();
        let serialized = serde_json::to_vec(&page).unwrap();
        assert_eq!(serialized.len(), page.budget.serialized_bytes);
        assert!(serialized.len() <= 2048);
        assert_eq!(page.budget.remaining_bytes, 2048 - serialized.len());
        assert!(page.budget.warning.is_some());
        assert!(page.has_more);
        assert!(page.next_cursor.is_some());
    }

    #[test]
    fn concatenating_cursor_pages_reconstructs_stable_order() {
        let expected = (0..11).collect::<Vec<_>>();
        let mut actual = Vec::new();
        let mut cursor = None;
        loop {
            let page = paginate_vec(
                "test",
                Some("owner/repo@abc"),
                "fixed",
                cursor.as_deref(),
                expected.clone(),
                3,
                DEFAULT_BUDGET_BYTES,
                false,
                |_| String::new(),
            )
            .unwrap();
            actual.extend(page.items);
            if !page.has_more {
                assert!(page.next_cursor.is_none());
                break;
            }
            cursor = page.next_cursor;
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn context_ranking_merges_overlapping_windows_deterministically() {
        let base = SearchItem {
            snapshot_id: "owner/repo@abc".to_string(),
            repo: "owner/repo".to_string(),
            commit_sha: "abc".to_string(),
            path: "src/cache.rs".to_string(),
            blob_sha: "blob".to_string(),
            query: "Cache".to_string(),
            match_line: 2,
            start_line: 1,
            end_line: 3,
            lines: vec![
                SourceLine {
                    line_number: 1,
                    text: "struct Cache".to_string(),
                },
                SourceLine {
                    line_number: 2,
                    text: "impl Cache".to_string(),
                },
            ],
        };
        let mut second = base.clone();
        second.query = "impl".to_string();
        second.match_line = 3;
        second.start_line = 2;
        second.end_line = 4;
        second.lines.push(SourceLine {
            line_number: 4,
            text: "}".to_string(),
        });
        let ranked = rank_context(
            vec![second, base],
            &["Cache".to_string(), "impl".to_string()],
        );
        assert_eq!(ranked.len(), 1);
        assert_eq!((ranked[0].start_line, ranked[0].end_line), (1, 4));
        assert_eq!(ranked[0].queries, vec!["Cache", "impl"]);
        assert_eq!(ranked[0].lines.len(), 3);
    }

    #[test]
    fn high_match_window_keeps_only_requested_items() {
        let items = (0..100_000).collect::<Vec<_>>();
        let page = paginate_vec(
            "test",
            None,
            "fixed",
            None,
            items,
            25,
            DEFAULT_BUDGET_BYTES,
            false,
            |_| String::new(),
        )
        .unwrap();
        assert_eq!(page.items.len(), 25);
        assert!(page.has_more);
    }
}

impl Default for WitOperations {
    fn default() -> Self {
        Self::new()
    }
}

enum SnapshotRecord {
    Disk {
        _temp_dir: TempDir,
        repo_path: PathBuf,
        repo: String,
        commit_sha: String,
    },
    Memory {
        snapshot: Arc<MemorySnapshot<ReqwestGitHubClient>>,
        snapshot_id: String,
    },
}

impl SnapshotRecord {
    fn handle(&self) -> SnapshotHandle {
        match self {
            Self::Disk {
                repo_path,
                repo,
                commit_sha,
                ..
            } => SnapshotHandle::Disk {
                repo_path: repo_path.clone(),
                repo: repo.clone(),
                commit_sha: commit_sha.clone(),
                snapshot_id: snapshot_id(repo, commit_sha),
            },
            Self::Memory {
                snapshot,
                snapshot_id,
            } => SnapshotHandle::Memory {
                snapshot: Arc::clone(snapshot),
                snapshot_id: snapshot_id.clone(),
            },
        }
    }
}

#[derive(Clone)]
enum SnapshotHandle {
    Disk {
        repo_path: PathBuf,
        repo: String,
        commit_sha: String,
        snapshot_id: String,
    },
    Memory {
        snapshot: Arc<MemorySnapshot<ReqwestGitHubClient>>,
        snapshot_id: String,
    },
}

impl SnapshotHandle {
    fn snapshot_id(&self) -> &str {
        match self {
            Self::Disk { snapshot_id, .. } | Self::Memory { snapshot_id, .. } => snapshot_id,
        }
    }

    fn repo(&self) -> &str {
        match self {
            Self::Disk { repo, .. } => repo,
            Self::Memory { snapshot, .. } => snapshot.provenance().repo.as_str(),
        }
    }

    fn commit_sha(&self) -> &str {
        match self {
            Self::Disk { commit_sha, .. } => commit_sha,
            Self::Memory { snapshot, .. } => snapshot.provenance().commit_sha.as_str(),
        }
    }

    fn require_disk_path(&self) -> Result<&Path, String> {
        match self {
            Self::Disk { repo_path, .. } => Ok(repo_path),
            Self::Memory { .. } => Err(
                "this operation requires the disk snapshot backend; reopen with WIT_SNAPSHOT_BACKEND=disk or omit the memory backend"
                    .to_string(),
            ),
        }
    }

    fn memory(&self) -> Option<&MemorySnapshot<ReqwestGitHubClient>> {
        match self {
            Self::Memory { snapshot, .. } => Some(snapshot.as_ref()),
            Self::Disk { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    #[default]
    AllowStale,
    RequireFresh,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ListFormat {
    /// Full per-entry provenance and metadata.
    #[default]
    Structured,
    /// One paths array with snapshot provenance stored once at the top level.
    Paths,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReadFormat {
    /// Full per-line provenance; this remains the direct MCP default.
    #[default]
    Structured,
    /// Joined file text with provenance stored once at the top level.
    Text,
    /// Line-number/text pairs with provenance stored once at the top level.
    Lines,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompactListFormat {
    Paths,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompactReadTextFormat {
    Text,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompactReadLinesFormat {
    Lines,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OpenArgs {
    /// GitHub repository in owner/repo form.
    pub repo: String,
    /// Default branch when omitted; otherwise a branch, tag, refs/heads or refs/tags name, or full commit SHA.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// allow_stale uses branch cache SWR; require_fresh refreshes before pinning.
    #[serde(default)]
    pub freshness: Freshness,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct RefsArgs {
    /// GitHub repository in owner/repo form.
    pub repo: String,
    /// Optionally resolve one branch, tag, or full commit SHA alongside the ref listing.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub cursor: Option<String>,
    pub max_items: Option<usize>,
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub include_rendered_text: bool,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct FindRepositoriesArgs {
    pub pattern: Option<String>,
    pub lang: Option<String>,
    /// Raw GitHub repository-search terms and qualifiers.
    pub query: Option<String>,
    pub cursor: Option<String>,
    pub max_items: Option<usize>,
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub include_rendered_text: bool,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ListArgs {
    pub snapshot_id: String,
    /// Directory or file path; omit for repository root.
    pub path: Option<String>,
    /// Levels below path to return. Defaults to 2.
    pub depth: Option<usize>,
    /// Include sizes and line counts for returned text files.
    #[serde(default)]
    pub include_metadata: bool,
    /// structured returns full entries; paths returns one compact paths array.
    #[serde(default)]
    pub format: ListFormat,
    pub cursor: Option<String>,
    pub max_items: Option<usize>,
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub include_rendered_text: bool,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct SearchCodeArgs {
    pub snapshot_id: String,
    /// One or more Rust regular expressions. Results identify the matching query.
    pub queries: Vec<String>,
    /// Optional git-style glob filters such as **/*.rs.
    #[serde(default)]
    pub globs: Vec<String>,
    /// Optional single include glob; combined with globs when both are present.
    pub glob: Option<String>,
    /// Limit matches to this repository-relative file or directory prefix.
    pub path_prefix: Option<String>,
    /// Git-style globs to exclude after include filtering.
    #[serde(default)]
    pub exclude: Vec<String>,
    pub context_lines: Option<usize>,
    pub max_results: Option<usize>,
    pub cursor: Option<String>,
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub include_rendered_text: bool,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ReadArgs {
    pub snapshot_id: String,
    pub path: String,
    /// One-based inclusive start line. Defaults to 1.
    pub start_line: Option<usize>,
    /// One-based inclusive end line. Defaults to end of file.
    pub end_line: Option<usize>,
    /// Controls optional rendered_text; structured line numbers are always returned for provenance.
    #[serde(default)]
    pub number_lines: bool,
    /// Direct MCP defaults to structured. Code Mode defaults this to text and also supports lines.
    #[serde(default)]
    pub format: ReadFormat,
    pub cursor: Option<String>,
    pub max_lines: Option<usize>,
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub include_rendered_text: bool,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ContextArgs {
    pub snapshot_id: String,
    pub queries: Vec<String>,
    #[serde(default)]
    pub globs: Vec<String>,
    pub context_lines: Option<usize>,
    pub max_results: Option<usize>,
    pub cursor: Option<String>,
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub include_rendered_text: bool,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AstMode {
    /// Language-aware definition index (functions, types, methods, constants).
    #[default]
    Symbols,
    /// Raw tree-sitter S-expression query with captures.
    Query,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct AstArgs {
    pub snapshot_id: String,
    /// symbols (default) lists definitions; query runs a tree-sitter query.
    #[serde(default)]
    pub mode: AstMode,
    /// Repository-relative file or directory to restrict the walk to.
    pub path: Option<String>,
    /// Optional git-style include globs such as **/*.rs.
    #[serde(default)]
    pub globs: Vec<String>,
    /// Git-style globs to exclude after include filtering.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Restrict to one language (rust, python, javascript, typescript, tsx, go, java, c). Required for query mode unless path names a single file.
    pub language: Option<String>,
    /// tree-sitter query (query mode), e.g. (call_expression function: (identifier) @callee (#eq? @callee "helper")).
    pub query: Option<String>,
    /// symbols mode: keep only these kind labels (fn, struct, class, method, ...).
    #[serde(default)]
    pub kinds: Vec<String>,
    /// symbols mode: keep only names matching this regex.
    pub name: Option<String>,
    /// Maximum files parsed (default 200, max 1000).
    pub max_files: Option<usize>,
    pub cursor: Option<String>,
    pub max_items: Option<usize>,
    pub max_bytes: Option<usize>,
    #[serde(default)]
    pub include_rendered_text: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AstItem {
    pub snapshot_id: String,
    pub repo: String,
    pub commit_sha: String,
    pub path: String,
    pub blob_sha: String,
    pub language: String,
    /// symbols: kind label (fn, struct, ...); query: the captured node kind.
    pub kind: String,
    /// symbols: definition name; query: first line of the captured text.
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub start_col: usize,
    pub end_col: usize,
    /// symbols: enclosing definition, if any.
    pub parent: Option<String>,
    /// symbols: nesting depth (0 = top level).
    pub depth: usize,
    /// symbols: first line of the definition.
    pub signature: String,
    /// query: capture name without @.
    pub capture: Option<String>,
    /// query: index of the matching pattern.
    pub pattern_index: Option<usize>,
    /// query: match ordinal within the file, to regroup captures of one match.
    pub match_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct OpenResponse {
    pub api_version: String,
    pub snapshot_id: String,
    pub repo: String,
    pub requested_ref: String,
    pub resolved_ref: String,
    pub commit_sha: String,
    pub cache: CacheProvenance,
    pub capabilities: SnapshotCapabilities,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CacheProvenance {
    pub state: String,
    pub last_checked_at: Option<u64>,
    pub last_updated_at: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SnapshotCapabilities {
    pub branches: bool,
    pub tags: bool,
    pub full_commit_sha: bool,
    pub pull_request_heads: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RefItem {
    pub repo: String,
    pub name: String,
    pub kind: String,
    pub resolved_ref: String,
    pub commit_sha: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RepositoryItem {
    pub name: String,
    pub full_name: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub stars: u32,
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListItem {
    pub snapshot_id: String,
    pub repo: String,
    pub commit_sha: String,
    pub path: String,
    pub kind: String,
    pub blob_sha: Option<String>,
    pub size_bytes: Option<u64>,
    pub lines: Option<usize>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CompactListPage {
    pub api_version: String,
    pub format: CompactListFormat,
    pub snapshot_id: String,
    pub repo: String,
    pub commit_sha: String,
    pub paths: Vec<String>,
    pub returned_items: usize,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub budget: BudgetInfo,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StructuredListPage {
    pub api_version: String,
    pub items: Vec<ListItem>,
    pub returned_items: usize,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub budget: BudgetInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_text: Option<String>,
}

impl From<Page<ListItem>> for StructuredListPage {
    fn from(page: Page<ListItem>) -> Self {
        Self {
            api_version: page.api_version,
            items: page.items,
            returned_items: page.returned_items,
            has_more: page.has_more,
            next_cursor: page.next_cursor,
            budget: page.budget,
            rendered_text: page.rendered_text,
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ListResponse {
    Structured(StructuredListPage),
    Paths(CompactListPage),
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SourceLine {
    pub line_number: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SearchItem {
    pub snapshot_id: String,
    pub repo: String,
    pub commit_sha: String,
    pub path: String,
    pub blob_sha: String,
    pub query: String,
    pub match_line: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub lines: Vec<SourceLine>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReadLineItem {
    pub snapshot_id: String,
    pub repo: String,
    pub commit_sha: String,
    pub path: String,
    pub blob_sha: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CompactReadTextPage {
    pub api_version: String,
    pub format: CompactReadTextFormat,
    pub snapshot_id: String,
    pub repo: String,
    pub commit_sha: String,
    pub path: String,
    pub blob_sha: Option<String>,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub text: String,
    pub returned_lines: usize,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub budget: BudgetInfo,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CompactReadLinesPage {
    pub api_version: String,
    pub format: CompactReadLinesFormat,
    pub snapshot_id: String,
    pub repo: String,
    pub commit_sha: String,
    pub path: String,
    pub blob_sha: Option<String>,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub lines: Vec<SourceLine>,
    pub returned_lines: usize,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub budget: BudgetInfo,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StructuredReadPage {
    pub api_version: String,
    pub items: Vec<ReadLineItem>,
    pub returned_items: usize,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub budget: BudgetInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_text: Option<String>,
}

impl From<Page<ReadLineItem>> for StructuredReadPage {
    fn from(page: Page<ReadLineItem>) -> Self {
        Self {
            api_version: page.api_version,
            items: page.items,
            returned_items: page.returned_items,
            has_more: page.has_more,
            next_cursor: page.next_cursor,
            budget: page.budget,
            rendered_text: page.rendered_text,
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ReadResponse {
    Structured(StructuredReadPage),
    Text(CompactReadTextPage),
    Lines(CompactReadLinesPage),
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ContextItem {
    pub snapshot_id: String,
    pub repo: String,
    pub commit_sha: String,
    pub path: String,
    pub blob_sha: String,
    pub start_line: usize,
    pub end_line: usize,
    pub score: i64,
    pub ranking_reasons: Vec<String>,
    pub queries: Vec<String>,
    pub lines: Vec<SourceLine>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BudgetInfo {
    pub requested_bytes: usize,
    pub serialized_bytes: usize,
    pub remaining_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Page<T> {
    pub api_version: String,
    pub items: Vec<T>,
    pub returned_items: usize,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub budget: BudgetInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CursorToken {
    version: u8,
    tool: String,
    snapshot_id: Option<String>,
    fingerprint: String,
    offset: usize,
}

impl WitOperations {
    pub async fn open(
        &self,
        context: &OperationContext,
        args: OpenArgs,
    ) -> Result<OpenResponse, String> {
        context.check()?;
        validate_repo(&args.repo)?;
        let requested = args
            .reference
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("HEAD")
            .to_string();

        if self.snapshot_backend == CliSnapshotBackend::Memory {
            return self.open_memory(context, args.repo, requested).await;
        }

        let remote = github_remote_url(&args.repo);
        let resolved = resolve_requested_ref(context, &remote, &requested)?;
        let (record, commit_sha, cache) = match &resolved.kind {
            ResolvedRefKind::DefaultBranch(branch) | ResolvedRefKind::Branch(branch) => {
                let selection = if matches!(&resolved.kind, ResolvedRefKind::DefaultBranch(_)) {
                    CacheBranchSelection::Default
                } else {
                    CacheBranchSelection::named(branch)
                };
                let mode = match args.freshness {
                    Freshness::AllowStale => CacheAcquisitionMode::ServeStaleAndRevalidate,
                    Freshness::RequireFresh => CacheAcquisitionMode::ForceInvalidate,
                };
                let cached = cache_github_repo_with_context(context, &args.repo, selection, mode)
                    .await
                    .map_err(anyhow_error)?;
                let commit_sha = git_stdout_with_context(
                    context,
                    cached.path(),
                    &["rev-parse", "HEAD^{commit}"],
                    "resolve cached snapshot commit",
                )?;
                let cache = cache_provenance(cached.path(), args.freshness);
                let record = clone_snapshot(context, &args.repo, cached.path(), &commit_sha)?;
                (record, commit_sha, cache)
            }
            ResolvedRefKind::Tag | ResolvedRefKind::Commit => {
                let (record, commit_sha) = fetch_snapshot(
                    context,
                    &args.repo,
                    &remote,
                    &resolved.fetch_ref,
                    &resolved.resolved_ref,
                )?;
                (
                    record,
                    commit_sha,
                    CacheProvenance {
                        state: "fresh".to_string(),
                        last_checked_at: None,
                        last_updated_at: None,
                        last_error: None,
                    },
                )
            }
        };
        let id = snapshot_id(&args.repo, &commit_sha);
        self.publish_snapshot(context, id.clone(), record)?;
        Ok(OpenResponse {
            api_version: "2".to_string(),
            snapshot_id: id,
            repo: args.repo,
            requested_ref: requested,
            resolved_ref: resolved.resolved_ref,
            commit_sha,
            cache,
            capabilities: SnapshotCapabilities {
                branches: true,
                tags: true,
                full_commit_sha: true,
                pull_request_heads: "not_supported; resolve the PR head to a full commit SHA"
                    .to_string(),
            },
        })
    }

    async fn open_memory(
        &self,
        context: &OperationContext,
        repo: String,
        requested: String,
    ) -> Result<OpenResponse, String> {
        context.check()?;
        let backend = self.memory_backend()?;
        let branch = if requested == "HEAD" {
            None
        } else {
            Some(requested.as_str())
        };
        let snapshot = context
            .wait(backend.open(&repo, branch))
            .await?
            .map_err(snapshot_error)?;
        context.check()?;
        let provenance = snapshot.provenance().clone();
        let commit_sha = provenance.commit_sha.clone();
        let id = snapshot_id(&repo, &commit_sha);
        let record = SnapshotRecord::Memory {
            snapshot: Arc::new(snapshot),
            snapshot_id: id.clone(),
        };
        self.publish_snapshot(context, id.clone(), record)?;
        Ok(OpenResponse {
            api_version: "2".to_string(),
            snapshot_id: id,
            repo,
            requested_ref: provenance.requested_ref,
            resolved_ref: provenance.resolved_ref,
            commit_sha,
            cache: CacheProvenance {
                state: "memory".to_string(),
                last_checked_at: None,
                last_updated_at: None,
                last_error: None,
            },
            capabilities: SnapshotCapabilities {
                branches: true,
                tags: true,
                full_commit_sha: true,
                pull_request_heads: "not_supported; resolve the PR head to a full commit SHA"
                    .to_string(),
            },
        })
    }

    pub async fn refs(
        &self,
        context: &OperationContext,
        args: RefsArgs,
    ) -> Result<Page<RefItem>, String> {
        context.check()?;
        validate_repo(&args.repo)?;
        let max_items = validate_page_items(args.max_items)?;
        let max_bytes = validate_budget(args.max_bytes)?;
        let remote = github_remote_url(&args.repo);
        let mut refs = list_remote_refs(context, &args.repo, &remote)?;
        if let Some(reference) = args.reference.as_deref() {
            let resolved = resolve_requested_ref(context, &remote, reference)?;
            if !refs
                .iter()
                .any(|item| item.resolved_ref == resolved.resolved_ref)
            {
                refs.push(RefItem {
                    repo: args.repo.clone(),
                    name: reference.to_string(),
                    kind: resolved.kind.label().to_string(),
                    resolved_ref: resolved.resolved_ref,
                    commit_sha: resolved.commit_sha.unwrap_or_else(|| reference.to_string()),
                    is_default: false,
                });
            }
        }
        refs.sort_by(|left, right| {
            (!left.is_default, &left.kind, &left.name).cmp(&(
                !right.is_default,
                &right.kind,
                &right.name,
            ))
        });
        let fingerprint = fingerprint(&json!({
            "repo": args.repo,
            "ref": args.reference,
            "include_rendered_text": args.include_rendered_text,
        }))?;
        let page = paginate_vec(
            "wit_refs",
            None,
            &fingerprint,
            args.cursor.as_deref(),
            refs,
            max_items,
            max_bytes,
            args.include_rendered_text,
            |items| {
                items
                    .iter()
                    .map(|item| {
                        format!("{}\t{}\t{}", item.kind, item.commit_sha, item.resolved_ref)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        )?;
        context.check()?;
        Ok(page)
    }

    pub async fn find_repositories(
        &self,
        context: &OperationContext,
        args: FindRepositoriesArgs,
    ) -> Result<Page<RepositoryItem>, String> {
        context.check()?;
        let max_items = validate_page_items(args.max_items)?;
        let max_bytes = validate_budget(args.max_bytes)?;
        let fingerprint = fingerprint(&json!({
            "pattern": args.pattern,
            "lang": args.lang,
            "query": args.query,
            "include_rendered_text": args.include_rendered_text,
        }))?;
        let offset = cursor_offset(
            "wit_find_repositories",
            None,
            &fingerprint,
            args.cursor.as_deref(),
        )?;
        let fetch_limit = (offset + max_items + 1).min(MAX_GITHUB_REPOS);
        let results = GitHubSearchClient::new()
            .search_repositories_with_context(
                context,
                args.pattern.as_deref(),
                args.lang.as_deref(),
                args.query.as_deref(),
                fetch_limit,
            )
            .await
            .map_err(anyhow_error)?;
        let items = results
            .repositories
            .into_iter()
            .map(|repo| RepositoryItem {
                name: repo.name,
                full_name: repo.full_name,
                description: repo.description,
                language: repo.language,
                stars: repo.stars,
                html_url: repo.html_url,
            })
            .collect();
        let page = paginate_vec_with_offset(
            "wit_find_repositories",
            None,
            &fingerprint,
            offset,
            items,
            max_items,
            max_bytes,
            args.include_rendered_text,
            |items| {
                items
                    .iter()
                    .map(|item| format!("{}\t{}", item.stars, item.full_name))
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        )?;
        context.check()?;
        Ok(page)
    }

    pub async fn list(
        &self,
        context: &OperationContext,
        args: ListArgs,
    ) -> Result<ListResponse, String> {
        context.check()?;
        let snapshot = self.snapshot(&args.snapshot_id)?;
        let depth = validate_range(args.depth, DEFAULT_LIST_DEPTH, MAX_LIST_DEPTH, "depth")?;
        let max_items = validate_page_items(args.max_items)?;
        let max_bytes = validate_budget(args.max_bytes)?;
        let base_path = normalize_repo_path(args.path.as_deref().unwrap_or(""))?;
        let fingerprint = fingerprint(&json!({
            "snapshot_id": args.snapshot_id,
            "path": base_path,
            "depth": depth,
            "include_metadata": args.include_metadata,
            "format": args.format,
            "include_rendered_text": args.include_rendered_text,
        }))?;
        let offset = cursor_offset(
            "wit_list",
            Some(snapshot.snapshot_id()),
            &fingerprint,
            args.cursor.as_deref(),
        )?;
        let items = if snapshot.memory().is_some() {
            list_memory_snapshot_window(
                context,
                &snapshot,
                &base_path,
                depth,
                args.include_metadata,
                offset,
                max_items + 1,
            )
            .await?
        } else {
            list_snapshot_window(
                context,
                &snapshot,
                &base_path,
                depth,
                args.include_metadata,
                offset,
                max_items + 1,
            )?
        };
        let page = paginate_window(
            "wit_list",
            Some(snapshot.snapshot_id()),
            &fingerprint,
            offset,
            items,
            max_items,
            max_bytes,
            args.include_rendered_text,
            |items| {
                items
                    .iter()
                    .map(|item| item.path.clone())
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        )?;
        context.check()?;
        match args.format {
            ListFormat::Structured => Ok(ListResponse::Structured(page.into())),
            ListFormat::Paths => compact_list_page(page, &snapshot).map(ListResponse::Paths),
        }
    }

    pub async fn search_code(
        &self,
        context: &OperationContext,
        args: SearchCodeArgs,
    ) -> Result<Page<SearchItem>, String> {
        context.check()?;
        let snapshot = self.snapshot(&args.snapshot_id)?;
        let queries = compile_queries(&args.queries)?;
        let mut include_globs = args.globs.clone();
        if let Some(glob) = args.glob.as_deref() {
            include_globs.push(glob.to_string());
        }
        let globs = compile_globs(&include_globs)?;
        let path_prefix = normalize_repo_path(args.path_prefix.as_deref().unwrap_or(""))?;
        let excludes = compile_globs(&args.exclude)?;
        let context_lines = validate_range(
            args.context_lines,
            DEFAULT_CONTEXT_LINES,
            MAX_CONTEXT_LINES,
            "context_lines",
        )?;
        let max_results = validate_range(
            args.max_results,
            DEFAULT_PAGE_ITEMS,
            MAX_PAGE_ITEMS,
            "max_results",
        )?;
        let max_bytes = validate_budget(args.max_bytes)?;
        let fingerprint = fingerprint(&json!({
            "snapshot_id": args.snapshot_id,
            "queries": args.queries,
            "globs": include_globs,
            "path_prefix": path_prefix,
            "exclude": args.exclude,
            "context_lines": context_lines,
            "max_results": max_results,
            "include_rendered_text": args.include_rendered_text,
        }))?;
        let offset = cursor_offset(
            "wit_search_code",
            Some(snapshot.snapshot_id()),
            &fingerprint,
            args.cursor.as_deref(),
        )?;
        let filters = SearchPathFilters {
            includes: globs,
            prefix: path_prefix,
            excludes,
        };
        let items = if snapshot.memory().is_some() {
            search_memory_snapshot_window(
                context,
                &snapshot,
                &queries,
                &filters,
                context_lines,
                offset,
                max_results + 1,
            )
            .await?
        } else {
            snapshot.require_disk_path()?;
            search_snapshot_window(
                context,
                &snapshot,
                &queries,
                &filters,
                context_lines,
                offset,
                max_results + 1,
            )?
        };
        let page = paginate_window(
            "wit_search_code",
            Some(snapshot.snapshot_id()),
            &fingerprint,
            offset,
            items,
            max_results,
            max_bytes,
            args.include_rendered_text,
            render_search_items,
        )?;
        context.check()?;
        Ok(page)
    }

    pub async fn ast(
        &self,
        context: &OperationContext,
        args: AstArgs,
    ) -> Result<Page<AstItem>, String> {
        context.check()?;
        let snapshot = self.snapshot(&args.snapshot_id)?;
        let path_prefix = normalize_repo_path(args.path.as_deref().unwrap_or(""))?;
        let includes = compile_globs(&args.globs)?;
        let excludes = compile_globs(&args.exclude)?;
        let language = match args
            .language
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(name) => Some(AstLanguage::from_name(name).ok_or_else(|| {
                format!(
                    "unknown language '{name}'; supported: {}",
                    ast::supported_languages_summary()
                )
            })?),
            None => None,
        };
        let single_file_language = if language.is_none() && !path_prefix.is_empty() {
            AstLanguage::from_path(&path_prefix)
        } else {
            None
        };
        let query = match args.mode {
            AstMode::Query => {
                let query = args
                    .query
                    .as_deref()
                    .map(str::trim)
                    .filter(|query| !query.is_empty())
                    .ok_or_else(|| {
                        "query mode requires a non-empty tree-sitter query".to_string()
                    })?;
                let target = language.or(single_file_language).ok_or_else(|| {
                    "query mode needs language (rust, python, javascript, typescript, tsx, go, java, c) unless path names a single source file".to_string()
                })?;
                ast::validate_query(target, query).map_err(|err| err.to_string())?;
                Some((target, query.to_string()))
            }
            AstMode::Symbols => {
                if args
                    .query
                    .as_deref()
                    .is_some_and(|query| !query.trim().is_empty())
                {
                    return Err("query is only accepted with mode: 'query'".to_string());
                }
                None
            }
        };
        let filter = SymbolFilter {
            kinds: args
                .kinds
                .iter()
                .map(|kind| kind.trim().to_string())
                .filter(|kind| !kind.is_empty())
                .collect(),
            name: match args
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                Some(pattern) => {
                    Some(Regex::new(pattern).map_err(|err| format!("invalid name regex: {err}"))?)
                }
                None => None,
            },
        };
        let max_files = validate_range(
            args.max_files,
            DEFAULT_AST_MAX_FILES,
            MAX_AST_MAX_FILES,
            "max_files",
        )?;
        let max_items = validate_range(
            args.max_items,
            DEFAULT_PAGE_ITEMS,
            MAX_PAGE_ITEMS,
            "max_items",
        )?;
        let max_bytes = validate_budget(args.max_bytes)?;
        let fingerprint = fingerprint(&json!({
            "snapshot_id": args.snapshot_id,
            "mode": args.mode,
            "path": path_prefix,
            "globs": args.globs,
            "exclude": args.exclude,
            "language": language.map(AstLanguage::name),
            "query": query.as_ref().map(|(_, text)| text.clone()),
            "kinds": filter.kinds,
            "name": args.name,
            "max_files": max_files,
            "max_items": max_items,
            "include_rendered_text": args.include_rendered_text,
        }))?;
        let offset = cursor_offset(
            "wit_ast",
            Some(snapshot.snapshot_id()),
            &fingerprint,
            args.cursor.as_deref(),
        )?;

        // Phase 1: candidate files (both backends share the tree walk).
        let mut candidates: Vec<(String, String, AstLanguage)> = Vec::new();
        walk_tree(context, &snapshot, |entry| {
            if entry.kind != "blob"
                || !path_has_prefix(&entry.path, &path_prefix)
                || includes
                    .as_ref()
                    .is_some_and(|set| !set.is_match(&entry.path))
                || excludes
                    .as_ref()
                    .is_some_and(|set| set.is_match(&entry.path))
                || entry
                    .size
                    .is_some_and(|size| size as usize > ast::MAX_AST_SOURCE_BYTES)
            {
                return Ok(true);
            }
            let Some(file_language) = AstLanguage::from_path(&entry.path) else {
                return Ok(true);
            };
            if language.is_some_and(|wanted| wanted != file_language) {
                return Ok(true);
            }
            if let Some((target, _)) = &query
                && *target != file_language
            {
                return Ok(true);
            }
            candidates.push((entry.path.clone(), entry.oid.clone(), file_language));
            Ok(candidates.len() < max_files)
        })?;

        // Phase 2: parse each candidate and window the items.
        let limit = max_items + 1;
        let mut seen = 0usize;
        let mut items: Vec<AstItem> = Vec::new();
        'files: for (path, oid, file_language) in candidates {
            context.check()?;
            let text = if let Some(memory) = snapshot.memory() {
                match memory
                    .blob_text_by_sha(&oid, ast::MAX_AST_SOURCE_BYTES as u64)
                    .await
                {
                    Ok(text) => text,
                    Err(_) => continue,
                }
            } else {
                match blob_text(context, &snapshot, &oid, ast::MAX_AST_SOURCE_BYTES) {
                    Ok(text) => text,
                    Err(_) => continue,
                }
            };
            let file_items: Vec<AstItem> = match &query {
                Some((target, query_text)) => match ast::run_query(*target, &text, query_text) {
                    Ok(captures) => captures
                        .into_iter()
                        .map(|capture| {
                            ast_item_from_capture(&snapshot, &path, &oid, file_language, capture)
                        })
                        .collect(),
                    Err(ast::AstError::Query(message)) => {
                        return Err(format!("invalid tree-sitter query: {message}"));
                    }
                    Err(_) => continue,
                },
                None => match ast::symbols(file_language, &text, &filter) {
                    Ok(symbols) => symbols
                        .into_iter()
                        .map(|symbol| {
                            ast_item_from_symbol(&snapshot, &path, &oid, file_language, symbol)
                        })
                        .collect(),
                    Err(_) => continue,
                },
            };
            for item in file_items {
                if seen < offset {
                    seen += 1;
                    continue;
                }
                if items.len() >= limit {
                    break 'files;
                }
                items.push(item);
                seen += 1;
            }
        }
        let page = paginate_window(
            "wit_ast",
            Some(snapshot.snapshot_id()),
            &fingerprint,
            offset,
            items,
            max_items,
            max_bytes,
            args.include_rendered_text,
            render_ast_items,
        )?;
        context.check()?;
        Ok(page)
    }

    pub async fn read(
        &self,
        context: &OperationContext,
        args: ReadArgs,
    ) -> Result<ReadResponse, String> {
        context.check()?;
        let snapshot = self.snapshot(&args.snapshot_id)?;
        let path = normalize_repo_path(&args.path)?;
        if path.is_empty() {
            return Err("path must identify a file".to_string());
        }
        let start_line = args.start_line.unwrap_or(1);
        if start_line == 0 {
            return Err("start_line is one-based and must be >= 1".to_string());
        }
        if args.end_line.is_some_and(|end| end < start_line) {
            return Err("end_line must be >= start_line".to_string());
        }
        let max_lines = validate_range(
            args.max_lines,
            DEFAULT_PAGE_ITEMS,
            MAX_PAGE_ITEMS,
            "max_lines",
        )?;
        let max_bytes = validate_budget(args.max_bytes)?;
        let fingerprint = fingerprint(&json!({
            "snapshot_id": args.snapshot_id,
            "path": path,
            "start_line": start_line,
            "end_line": args.end_line,
            "number_lines": args.number_lines,
            "format": args.format,
            "max_lines": max_lines,
            "include_rendered_text": args.include_rendered_text,
        }))?;
        let offset = cursor_offset(
            "wit_read",
            Some(snapshot.snapshot_id()),
            &fingerprint,
            args.cursor.as_deref(),
        )?;
        let items = if snapshot.memory().is_some() {
            read_memory_snapshot_window(
                context,
                &snapshot,
                &path,
                start_line,
                args.end_line,
                offset,
                max_lines + 1,
            )
            .await?
        } else {
            read_snapshot_window(
                context,
                &snapshot,
                &path,
                start_line,
                args.end_line,
                offset,
                max_lines + 1,
            )?
        };
        let number_lines = args.number_lines;
        let page = paginate_window(
            "wit_read",
            Some(snapshot.snapshot_id()),
            &fingerprint,
            offset,
            items,
            max_lines,
            max_bytes,
            args.include_rendered_text,
            move |items| render_read_items(items, number_lines),
        )?;
        context.check()?;
        match args.format {
            ReadFormat::Structured => Ok(ReadResponse::Structured(page.into())),
            ReadFormat::Text => {
                compact_read_text_page(page, &snapshot, &path).map(ReadResponse::Text)
            }
            ReadFormat::Lines => {
                compact_read_lines_page(page, &snapshot, &path).map(ReadResponse::Lines)
            }
        }
    }

    pub async fn context(
        &self,
        context: &OperationContext,
        args: ContextArgs,
    ) -> Result<Page<ContextItem>, String> {
        context.check()?;
        let snapshot = self.snapshot(&args.snapshot_id)?;
        let queries = compile_queries(&args.queries)?;
        let globs = compile_globs(&args.globs)?;
        let context_lines = validate_range(
            args.context_lines,
            DEFAULT_CONTEXT_LINES,
            MAX_CONTEXT_LINES,
            "context_lines",
        )?;
        let max_results = validate_range(
            args.max_results,
            DEFAULT_CONTEXT_RESULTS,
            MAX_CONTEXT_RESULTS,
            "max_results",
        )?;
        let max_bytes = validate_budget(args.max_bytes)?;
        let fingerprint = fingerprint(&json!({
            "snapshot_id": args.snapshot_id,
            "queries": args.queries,
            "globs": args.globs,
            "context_lines": context_lines,
            "max_results": max_results,
            "include_rendered_text": args.include_rendered_text,
        }))?;
        let offset = cursor_offset(
            "wit_context",
            Some(snapshot.snapshot_id()),
            &fingerprint,
            args.cursor.as_deref(),
        )?;
        let filters = SearchPathFilters {
            includes: globs,
            prefix: String::new(),
            excludes: None,
        };
        let candidates = if snapshot.memory().is_some() {
            search_memory_snapshot_window(
                context,
                &snapshot,
                &queries,
                &filters,
                context_lines,
                0,
                MAX_CONTEXT_CANDIDATES + 1,
            )
            .await?
        } else {
            snapshot.require_disk_path()?;
            search_snapshot_window(
                context,
                &snapshot,
                &queries,
                &filters,
                context_lines,
                0,
                MAX_CONTEXT_CANDIDATES + 1,
            )?
        };
        if candidates.len() > MAX_CONTEXT_CANDIDATES {
            return Err(format!(
                "wit_context matched more than {MAX_CONTEXT_CANDIDATES} candidate windows; narrow queries or globs so deterministic ranking stays memory-bounded"
            ));
        }
        let ranked = rank_context(candidates, &args.queries);
        let items = ranked
            .into_iter()
            .skip(offset)
            .take(max_results + 1)
            .collect();
        let page = paginate_window(
            "wit_context",
            Some(snapshot.snapshot_id()),
            &fingerprint,
            offset,
            items,
            max_results,
            max_bytes,
            args.include_rendered_text,
            render_context_items,
        )?;
        context.check()?;
        Ok(page)
    }
}
