use anyhow::Context;
use fs2::FileExt;
use gix::{Repository, bstr::ByteSlice};
use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ptree::{TreeBuilder, print_tree};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

pub const WIT_CACHE_DIR_ENV: &str = "WIT_CACHE_DIR";
pub const WIT_CACHE_SUBDIR: &str = ".wit/cache";
const CACHE_SCHEMA_VERSION: u32 = 1;
const CACHE_METADATA_FILE: &str = "metadata.json";
const CACHE_METADATA_TEMP_FILE: &str = "metadata.json.tmp";
const CACHE_LOCK_FILE: &str = ".cache.lock";
static CACHE_PROCESS_LOCKS: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

struct CacheLock {
    path: PathBuf,
    _file_lock: File,
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        if let Ok(mut locks) = CACHE_PROCESS_LOCKS.lock()
            && let Some(paths) = locks.as_mut()
        {
            paths.remove(&self.path);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheAcquisitionMode {
    ServeStaleAndRevalidate,
    ForceInvalidate,
}

impl CacheAcquisitionMode {
    fn is_force_invalidate(self) -> bool {
        matches!(self, Self::ForceInvalidate)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheTarget {
    owner_repo: String,
    owner: String,
    repo: String,
    branch: String,
    encoded_branch: String,
}

impl CacheTarget {
    fn new(owner_repo: &str, branch: &str) -> anyhow::Result<Self> {
        let (owner, repo) = split_owner_repo(owner_repo)?;
        if branch.is_empty() {
            anyhow::bail!("cache branch must not be empty");
        }

        Ok(Self {
            owner_repo: owner_repo.to_string(),
            owner: owner.to_string(),
            repo: repo.to_string(),
            branch: branch.to_string(),
            encoded_branch: encode_branch_for_path(branch),
        })
    }

    fn branch_dir(&self, cache_dir: &Path) -> PathBuf {
        cache_dir
            .join(&self.owner)
            .join(&self.repo)
            .join("branches")
            .join(&self.encoded_branch)
    }

    fn repo_path(&self, cache_dir: &Path) -> PathBuf {
        self.branch_dir(cache_dir).join("repo.git")
    }

    fn metadata_path(&self, cache_dir: &Path) -> PathBuf {
        self.branch_dir(cache_dir).join(CACHE_METADATA_FILE)
    }

    fn lock_path(&self, cache_dir: &Path) -> PathBuf {
        self.branch_dir(cache_dir).join(CACHE_LOCK_FILE)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedBranch {
    name: String,
    current_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedCacheTarget {
    target: CacheTarget,
    remote_url: String,
    branch: ResolvedBranch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CacheMetadata {
    cache_schema_version: u32,
    owner_repo: String,
    branch: String,
    remote_url: String,
    current_sha: String,
    last_checked_at: u64,
    last_updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

impl CacheMetadata {
    fn new(resolved: &ResolvedCacheTarget, last_checked_at: u64, last_updated_at: u64) -> Self {
        Self {
            cache_schema_version: CACHE_SCHEMA_VERSION,
            owner_repo: resolved.target.owner_repo.clone(),
            branch: resolved.branch.name.clone(),
            remote_url: resolved.remote_url.clone(),
            current_sha: resolved.branch.current_sha.clone(),
            last_checked_at,
            last_updated_at,
            last_error: None,
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.cache_schema_version != CACHE_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported cache metadata schema version {}",
                self.cache_schema_version
            );
        }
        if self.owner_repo.is_empty()
            || self.branch.is_empty()
            || self.remote_url.is_empty()
            || self.current_sha.is_empty()
        {
            anyhow::bail!("cache metadata is missing required identity fields");
        }
        Ok(())
    }

    fn matches_identity(&self, resolved: &ResolvedCacheTarget) -> bool {
        self.validate().is_ok()
            && self.owner_repo == resolved.target.owner_repo
            && self.branch == resolved.branch.name
            && self.remote_url == resolved.remote_url
    }
}

fn encode_branch_for_path(branch: &str) -> String {
    let mut encoded = String::with_capacity(branch.len() + 2);
    encoded.push_str("b-");
    for byte in branch.bytes() {
        match byte {
            b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn split_owner_repo(owner_repo: &str) -> anyhow::Result<(&str, &str)> {
    let (owner, repo) = owner_repo
        .split_once('/')
        .filter(|(owner, repo)| !owner.is_empty() && !repo.is_empty() && !repo.contains('/'))
        .with_context(|| format!("expected GitHub repository as owner/repo, got '{owner_repo}'"))?;
    if !is_safe_repo_component(owner) || !is_safe_repo_component(repo) {
        anyhow::bail!("invalid GitHub repository identity: '{owner_repo}'");
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

fn repo_cache_root(cache_dir: &Path, owner_repo: &str) -> anyhow::Result<PathBuf> {
    let (owner, repo) = split_owner_repo(owner_repo)?;
    Ok(cache_dir.join(owner).join(repo))
}

fn github_remote_url(owner_repo: &str) -> String {
    format!("https://github.com/{owner_repo}", owner_repo = owner_repo)
}

fn default_cache_target_for_cache(
    owner_repo: &str,
    remote_url: &str,
    cache_dir: &Path,
    mode: CacheAcquisitionMode,
) -> anyhow::Result<ResolvedCacheTarget> {
    if !mode.is_force_invalidate()
        && let Some(cached) = cached_default_cache_target(owner_repo, remote_url, cache_dir)?
    {
        return Ok(cached);
    }

    default_cache_target(owner_repo, remote_url)
}

fn cached_default_cache_target(
    owner_repo: &str,
    remote_url: &str,
    cache_dir: &Path,
) -> anyhow::Result<Option<ResolvedCacheTarget>> {
    let branches_dir = repo_cache_root(cache_dir, owner_repo)?.join("branches");
    let entries = match std::fs::read_dir(&branches_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "failed to read cache branches directory '{}'",
                    branches_dir.display()
                )
            });
        }
    };
    let mut matches = Vec::new();

    for entry in entries {
        let entry = entry.with_context(|| {
            format!(
                "failed to read cache branch entry under '{}'",
                branches_dir.display()
            )
        })?;
        if !entry
            .file_type()
            .with_context(|| format!("failed to inspect '{}'", entry.path().display()))?
            .is_dir()
        {
            continue;
        }

        let metadata_path = entry.path().join(CACHE_METADATA_FILE);
        let Ok(metadata) = read_cache_metadata(&metadata_path) else {
            continue;
        };
        if metadata.owner_repo != owner_repo || metadata.remote_url != remote_url {
            continue;
        }

        let target = CacheTarget::new(owner_repo, &metadata.branch)?;
        if target.metadata_path(cache_dir) != metadata_path {
            continue;
        }

        matches.push(ResolvedCacheTarget {
            target,
            remote_url: metadata.remote_url.clone(),
            branch: ResolvedBranch {
                name: metadata.branch,
                current_sha: metadata.current_sha,
            },
        });
    }

    Ok(if matches.len() == 1 {
        matches.pop()
    } else {
        None
    })
}

fn default_cache_target(owner_repo: &str, remote_url: &str) -> anyhow::Result<ResolvedCacheTarget> {
    split_owner_repo(owner_repo)?;
    let branch = resolve_default_branch(remote_url)?;
    let target = CacheTarget::new(owner_repo, &branch.name)?;
    Ok(ResolvedCacheTarget {
        target,
        remote_url: remote_url.to_string(),
        branch,
    })
}

fn resolve_default_branch(remote_url: &str) -> anyhow::Result<ResolvedBranch> {
    let output = Command::new("git")
        .arg("ls-remote")
        .arg("--symref")
        .arg(remote_url)
        .arg("HEAD")
        .output()
        .context("failed to invoke git for default branch resolution")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        anyhow::bail!(
            "git ls-remote failed (status: {}) stderr: '{}' stdout: '{}'",
            output.status,
            stderr,
            stdout
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_default_branch_ls_remote(&stdout)
}

fn resolve_branch_sha(remote_url: &str, branch: &str) -> anyhow::Result<String> {
    let branch_ref = format!("refs/heads/{branch}");
    let output = Command::new("git")
        .arg("ls-remote")
        .arg(remote_url)
        .arg(&branch_ref)
        .output()
        .with_context(|| format!("failed to invoke git for branch '{branch}' resolution"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        anyhow::bail!(
            "git ls-remote failed for branch '{}' (status: {}) stderr: '{}' stdout: '{}'",
            branch,
            output.status,
            stderr,
            stdout
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_branch_sha_ls_remote(&stdout, branch)
}

fn parse_branch_sha_ls_remote(output: &str, branch: &str) -> anyhow::Result<String> {
    let branch_ref = format!("refs/heads/{branch}");
    for line in output.lines() {
        if let Some((value, target)) = line.split_once('\t')
            && target == branch_ref
            && value.chars().all(|ch| ch.is_ascii_hexdigit())
        {
            return Ok(value.to_string());
        }
    }

    anyhow::bail!("remote branch '{branch}' did not include a commit SHA")
}

fn parse_default_branch_ls_remote(output: &str) -> anyhow::Result<ResolvedBranch> {
    let mut branch = None;
    let mut sha = None;

    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("ref: refs/heads/") {
            if let Some((name, target)) = rest.split_once('\t')
                && target == "HEAD"
            {
                branch = Some(name.to_string());
            }
            continue;
        }

        if let Some((value, target)) = line.split_once('\t')
            && target == "HEAD"
            && value.chars().all(|ch| ch.is_ascii_hexdigit())
        {
            sha = Some(value.to_string());
        }
    }

    let name = branch.context("remote HEAD did not resolve to refs/heads/<branch>")?;
    let current_sha = sha.context("remote HEAD did not include a commit SHA")?;
    Ok(ResolvedBranch { name, current_sha })
}

fn cache_metadata_is_usable(metadata_path: &Path, resolved: &ResolvedCacheTarget) -> bool {
    read_cache_metadata(metadata_path).is_ok_and(|metadata| metadata.matches_identity(resolved))
}

fn read_cache_metadata(metadata_path: &Path) -> anyhow::Result<CacheMetadata> {
    let file = File::open(metadata_path).with_context(|| {
        format!(
            "failed to open cache metadata '{}'",
            metadata_path.display()
        )
    })?;
    let metadata: CacheMetadata = serde_json::from_reader(file).with_context(|| {
        format!(
            "failed to parse cache metadata '{}'",
            metadata_path.display()
        )
    })?;
    metadata.validate()?;
    Ok(metadata)
}

fn write_cache_metadata(metadata_path: &Path, metadata: &CacheMetadata) -> anyhow::Result<()> {
    let temp_path = write_cache_metadata_temp(metadata_path, metadata)?;
    replace_cache_metadata(&temp_path, metadata_path)
}

fn write_cache_metadata_temp(
    metadata_path: &Path,
    metadata: &CacheMetadata,
) -> anyhow::Result<PathBuf> {
    metadata.validate()?;
    let parent = metadata_path.parent().with_context(|| {
        format!(
            "cache metadata path '{}' has no parent",
            metadata_path.display()
        )
    })?;
    std::fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create cache metadata parent '{}'",
            parent.display()
        )
    })?;

    let temp_path = parent.join(CACHE_METADATA_TEMP_FILE);
    let mut temp_file = File::create(&temp_path).with_context(|| {
        format!(
            "failed to create temporary cache metadata '{}'",
            temp_path.display()
        )
    })?;
    serde_json::to_writer_pretty(&mut temp_file, metadata).with_context(|| {
        format!(
            "failed to write temporary cache metadata '{}'",
            temp_path.display()
        )
    })?;
    temp_file
        .write_all(b"\n")
        .with_context(|| format!("failed to finish cache metadata '{}'", temp_path.display()))?;
    temp_file
        .sync_all()
        .with_context(|| format!("failed to sync cache metadata '{}'", temp_path.display()))?;
    Ok(temp_path)
}

fn replace_cache_metadata(temp_path: &Path, metadata_path: &Path) -> anyhow::Result<()> {
    std::fs::rename(temp_path, metadata_path).with_context(|| {
        format!(
            "failed to replace cache metadata '{}' with '{}'",
            metadata_path.display(),
            temp_path.display()
        )
    })?;
    Ok(())
}

fn remove_legacy_repo_cache_if_present(cache_dir: &Path, owner_repo: &str) -> anyhow::Result<()> {
    let repo_root = repo_cache_root(cache_dir, owner_repo)?;
    if is_legacy_bare_cache_dir(&repo_root) {
        remove_cache_dir(&repo_root)?;
    }
    Ok(())
}

fn is_legacy_bare_cache_dir(path: &Path) -> bool {
    path.join("HEAD").is_file() && path.join("objects").is_dir() && path.join("refs").is_dir()
}

fn current_unix_timestamp() -> anyhow::Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before UNIX epoch")?
        .as_secs())
}

pub fn revalidate_github_repo(owner_repo: &str) -> anyhow::Result<()> {
    split_owner_repo(owner_repo)?;
    let remote_url = github_remote_url(owner_repo);
    let cache_dir = wit_cache_dir();
    remove_legacy_repo_cache_if_present(&cache_dir, owner_repo)?;
    let resolved = default_cache_target_for_cache(
        owner_repo,
        &remote_url,
        &cache_dir,
        CacheAcquisitionMode::ServeStaleAndRevalidate,
    )?;
    let _cache_lock = acquire_cache_lock(&resolved.target.lock_path(&cache_dir))?;
    revalidate_cache_target(&resolved, &cache_dir)
}

pub fn wit_cache_dir() -> PathBuf {
    if let Some(path) = std::env::var_os(WIT_CACHE_DIR_ENV).filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }

    std::env::temp_dir().join(WIT_CACHE_SUBDIR)
}

#[derive(Debug, Clone, Default)]
pub struct IgnoreMatcher {
    literal_paths: Vec<String>,
    literal_components: HashSet<String>,
    glob_set: Option<GlobSet>,
}

impl IgnoreMatcher {
    pub fn new(patterns: &[String]) -> anyhow::Result<Self> {
        let mut literal_paths = Vec::new();
        let mut literal_components = HashSet::new();
        let mut glob_builder = GlobSetBuilder::new();
        let mut has_globs = false;

        for raw_pattern in patterns {
            let normalized = normalize_repo_path(raw_pattern);
            if normalized.is_empty() {
                continue;
            }

            if pattern_has_glob_chars(&normalized) {
                glob_builder.add(Glob::new(&normalized)?);
                if !normalized.contains('/') {
                    glob_builder.add(Glob::new(&format!("**/{}", normalized))?);
                }
                has_globs = true;
                continue;
            }

            if normalized.contains('/') {
                literal_paths.push(normalized);
            } else {
                literal_components.insert(normalized);
            }
        }

        let glob_set = if has_globs {
            Some(glob_builder.build()?)
        } else {
            None
        };

        Ok(Self {
            literal_paths,
            literal_components,
            glob_set,
        })
    }

    pub fn is_ignored(&self, path: &str) -> bool {
        let path = normalize_repo_path(path);
        if path.is_empty() {
            return false;
        }

        if self
            .glob_set
            .as_ref()
            .is_some_and(|set| set.is_match(&path))
        {
            return true;
        }

        if self.literal_paths.iter().any(|prefix| {
            path == *prefix
                || path
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
        }) {
            return true;
        }

        path.split('/')
            .any(|component| self.literal_components.contains(component))
    }
}

fn pattern_has_glob_chars(pattern: &str) -> bool {
    pattern
        .chars()
        .any(|c| matches!(c, '*' | '?' | '[' | ']' | '{' | '}'))
}

fn normalize_repo_path(value: &str) -> String {
    value
        .trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

/// Options for grep search (mirrors ripgrep CLI options)
#[derive(Debug, Clone, Default)]
pub struct GrepOptions {
    /// Case insensitive search (-i)
    pub ignore_case: bool,
    /// Smart case: case-insensitive if pattern is all lowercase (-S)
    pub smart_case: bool,
    /// Match whole words only (-w)
    pub word_regexp: bool,
    /// Invert match - show non-matching lines (-v)
    pub invert_match: bool,
    /// Max matches total (None = unlimited)
    pub max_count: Option<usize>,
    /// Lines of context before match (-B)
    pub before_context: usize,
    /// Lines of context after match (-A)
    pub after_context: usize,
    /// Glob pattern to filter files (-g)
    pub glob: Option<String>,
    /// Only show file names with matches (-l)
    pub files_with_matches: bool,
    /// Only show count of matches (-c)
    pub count: bool,
    /// Ignore patterns (files, directories, globs)
    pub ignore: Vec<String>,
}

impl GrepOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ignore_case(mut self, yes: bool) -> Self {
        self.ignore_case = yes;
        self
    }

    pub fn smart_case(mut self, yes: bool) -> Self {
        self.smart_case = yes;
        self
    }

    pub fn word_regexp(mut self, yes: bool) -> Self {
        self.word_regexp = yes;
        self
    }

    pub fn invert_match(mut self, yes: bool) -> Self {
        self.invert_match = yes;
        self
    }

    pub fn max_count(mut self, count: usize) -> Self {
        self.max_count = Some(count);
        self
    }

    pub fn context(mut self, lines: usize) -> Self {
        self.before_context = lines;
        self.after_context = lines;
        self
    }

    pub fn before_context(mut self, lines: usize) -> Self {
        self.before_context = lines;
        self
    }

    pub fn after_context(mut self, lines: usize) -> Self {
        self.after_context = lines;
        self
    }

    pub fn glob(mut self, pattern: Option<String>) -> Self {
        self.glob = pattern;
        self
    }

    pub fn files_with_matches(mut self, yes: bool) -> Self {
        self.files_with_matches = yes;
        self
    }

    pub fn count(mut self, yes: bool) -> Self {
        self.count = yes;
        self
    }

    pub fn ignore(mut self, patterns: Vec<String>) -> Self {
        self.ignore = patterns;
        self
    }
}

/// A single match result from grep search
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepMatch {
    pub path: String,
    pub line_number: u64,
    pub content: String,
    /// Whether this line is a context line (not an actual match)
    pub is_context: bool,
}

/// Result of a grep search
#[derive(Debug, Clone)]
pub enum GrepResult {
    /// Normal matches with line content
    Matches(Vec<GrepMatch>),
    /// File names only (-l mode)
    Files(Vec<String>),
    /// Count per file (-c mode)
    Counts(Vec<(String, usize)>),
}

pub async fn cache_github_repo(
    owner_repo: &str,
    mode: CacheAcquisitionMode,
) -> anyhow::Result<Repository> {
    split_owner_repo(owner_repo)?;
    let remote_url = github_remote_url(owner_repo);
    let cache_dir = wit_cache_dir();
    remove_legacy_repo_cache_if_present(&cache_dir, owner_repo)?;
    let resolved = default_cache_target_for_cache(owner_repo, &remote_url, &cache_dir, mode)?;
    let _cache_lock = acquire_cache_lock(&resolved.target.lock_path(&cache_dir))?;
    cache_github_repo_target(&resolved, &cache_dir, mode)
}

fn cache_github_repo_target(
    resolved: &ResolvedCacheTarget,
    cache_dir: &Path,
    mode: CacheAcquisitionMode,
) -> anyhow::Result<Repository> {
    let target = &resolved.target;
    let cache_path = target.repo_path(cache_dir);
    let metadata_path = target.metadata_path(cache_dir);

    if cache_path.exists() && !mode.is_force_invalidate() {
        match gix::open(&cache_path) {
            Ok(repo)
                if cache_has_head_commit(&repo)
                    && cache_metadata_is_usable(&metadata_path, resolved) =>
            {
                if mode == CacheAcquisitionMode::ServeStaleAndRevalidate {
                    let _ = spawn_cache_revalidation(resolved, cache_dir);
                }
                return Ok(repo);
            }
            Ok(_) | Err(_) => {
                // A prior failed fetch can leave a cache directory with an unborn HEAD.
                // Missing or corrupt metadata likewise makes the cache unsafe to trust.
            }
        }
    }

    remove_cache_metadata(&metadata_path)?;
    let repo = recache_repo(&resolved.remote_url, &cache_path)?;
    let now = current_unix_timestamp()?;
    let metadata = CacheMetadata::new(resolved, now, now);
    write_cache_metadata(&metadata_path, &metadata)?;
    Ok(repo)
}

#[cfg(not(test))]
fn spawn_cache_revalidation(
    resolved: &ResolvedCacheTarget,
    cache_dir: &Path,
) -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("failed to determine current executable")?;
    Command::new(exe)
        .arg("__cache-revalidate")
        .arg("--repo")
        .arg(&resolved.target.owner_repo)
        .env(WIT_CACHE_DIR_ENV, cache_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("failed to spawn cache revalidation worker")?;
    Ok(())
}

#[cfg(test)]
fn spawn_cache_revalidation(
    _resolved: &ResolvedCacheTarget,
    _cache_dir: &Path,
) -> anyhow::Result<()> {
    Ok(())
}

fn revalidate_cache_target(resolved: &ResolvedCacheTarget, cache_dir: &Path) -> anyhow::Result<()> {
    let metadata_path = resolved.target.metadata_path(cache_dir);
    let mut metadata = read_cache_metadata(&metadata_path)?;
    let now = current_unix_timestamp()?;

    match resolve_branch_sha(&resolved.remote_url, &metadata.branch) {
        Ok(remote_sha) if remote_sha == metadata.current_sha => {
            metadata.last_checked_at = now;
            metadata.last_error = None;
            write_cache_metadata(&metadata_path, &metadata)?;
            Ok(())
        }
        Ok(remote_sha) => {
            let refreshed = ResolvedCacheTarget {
                target: resolved.target.clone(),
                remote_url: resolved.remote_url.clone(),
                branch: ResolvedBranch {
                    name: metadata.branch.clone(),
                    current_sha: remote_sha,
                },
            };
            match refresh_repo_preserving_existing(
                &refreshed.remote_url,
                &refreshed.target.repo_path(cache_dir),
            ) {
                Ok(_) => {
                    let updated = CacheMetadata::new(&refreshed, now, now);
                    write_cache_metadata(&metadata_path, &updated)?;
                    Ok(())
                }
                Err(err) => {
                    metadata.last_checked_at = now;
                    metadata.last_error = Some(err.to_string());
                    write_cache_metadata(&metadata_path, &metadata)?;
                    Err(err)
                }
            }
        }
        Err(err) => {
            metadata.last_checked_at = now;
            metadata.last_error = Some(err.to_string());
            write_cache_metadata(&metadata_path, &metadata)?;
            Err(err)
        }
    }
}

fn remove_cache_metadata(metadata_path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(metadata_path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| {
            format!(
                "failed to remove stale cache metadata '{}'",
                metadata_path.display()
            )
        }),
    }
}

fn acquire_cache_lock(lock_path: &Path) -> anyhow::Result<CacheLock> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("failed to create cache lock parent '{}'", parent.display())
        })?;
    }

    acquire_process_cache_lock(lock_path);

    let lock_result = (|| -> anyhow::Result<File> {
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(lock_path)
            .with_context(|| format!("failed to open cache lock '{}'", lock_path.display()))?;
        lock_file
            .lock_exclusive()
            .with_context(|| format!("failed to lock cache '{}'", lock_path.display()))?;
        Ok(lock_file)
    })();

    let lock_file = match lock_result {
        Ok(lock_file) => lock_file,
        Err(err) => {
            release_process_cache_lock(lock_path);
            return Err(err);
        }
    };

    Ok(CacheLock {
        path: lock_path.to_path_buf(),
        _file_lock: lock_file,
    })
}

fn acquire_process_cache_lock(lock_path: &Path) {
    let lock_path = lock_path.to_path_buf();
    loop {
        {
            let mut locks = CACHE_PROCESS_LOCKS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let paths = locks.get_or_insert_with(HashSet::new);
            if paths.insert(lock_path.clone()) {
                return;
            }
        }
        thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn release_process_cache_lock(lock_path: &Path) {
    if let Ok(mut locks) = CACHE_PROCESS_LOCKS.lock()
        && let Some(paths) = locks.as_mut()
    {
        paths.remove(lock_path);
    }
}

fn recache_repo(repo_url: &str, cache_path: &Path) -> anyhow::Result<Repository> {
    remove_cache_dir(cache_path)?;

    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create cache parent '{}'", parent.display()))?;
    }

    match clone_with_gix(repo_url, cache_path) {
        Ok(repo) => Ok(repo),
        Err(err) => {
            remove_cache_dir(cache_path)?;
            if should_fallback_to_git_cli(&err) {
                if let Some(parent) = cache_path.parent() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create cache parent '{}'", parent.display())
                    })?;
                }
                clone_with_git_cli(repo_url, cache_path).with_context(|| {
                    format!("gix clone timed out and git fallback failed for '{repo_url}'")
                })
            } else {
                Err(err).with_context(|| format!("failed to cache repository from '{repo_url}'"))
            }
        }
    }
}

fn refresh_repo_preserving_existing(
    repo_url: &str,
    cache_path: &Path,
) -> anyhow::Result<Repository> {
    let parent = cache_path
        .parent()
        .with_context(|| format!("cache path '{}' has no parent", cache_path.display()))?;
    let staging_path = parent.join("repo.git.tmp");
    remove_cache_dir(&staging_path)?;

    match recache_repo(repo_url, &staging_path) {
        Ok(_) => {
            remove_cache_dir(cache_path)?;
            std::fs::rename(&staging_path, cache_path).with_context(|| {
                format!(
                    "failed to promote refreshed cache '{}' to '{}'",
                    staging_path.display(),
                    cache_path.display()
                )
            })?;
            gix::open(cache_path).with_context(|| {
                format!("failed to open refreshed cache '{}'", cache_path.display())
            })
        }
        Err(err) => {
            remove_cache_dir(&staging_path)?;
            Err(err)
        }
    }
}

fn remove_cache_dir(cache_path: &Path) -> anyhow::Result<()> {
    if cache_path.exists() {
        std::fs::remove_dir_all(cache_path)
            .with_context(|| format!("failed to remove cache '{}'", cache_path.display()))?;
    }
    Ok(())
}

fn clone_with_gix(repo_url: &str, cache_path: &Path) -> anyhow::Result<Repository> {
    std::fs::create_dir_all(cache_path)
        .with_context(|| format!("failed to create cache '{}'", cache_path.display()))?;

    let (repo, _) = gix::prepare_clone_bare(repo_url, cache_path)?
        .with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(
            1.try_into().unwrap(),
        ))
        .fetch_only(gix::progress::Discard, &gix::interrupt::IS_INTERRUPTED)?;

    Ok(repo)
}

fn clone_with_git_cli(repo_url: &str, cache_path: &Path) -> anyhow::Result<Repository> {
    let output = Command::new("git")
        .arg("clone")
        .arg("--bare")
        .arg("--depth")
        .arg("1")
        .arg(repo_url)
        .arg(cache_path)
        .output()
        .context("failed to invoke git for cache fallback")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        anyhow::bail!(
            "git clone fallback failed (status: {}) stderr: '{}' stdout: '{}'",
            output.status,
            stderr,
            stdout
        );
    }

    gix::open(cache_path)
        .with_context(|| format!("failed to open fallback cache '{}'", cache_path.display()))
}

fn should_fallback_to_git_cli(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        let message = cause.to_string().to_ascii_lowercase();
        message.contains("timed out") || message.contains("timeout")
    })
}

fn cache_has_head_commit(repo: &Repository) -> bool {
    repo.head_commit().is_ok()
}

struct MatchCollector<'a> {
    path: &'a str,
    matches: &'a mut Vec<GrepMatch>,
    max_count: usize,
    total_count: usize,
}

impl Sink for MatchCollector<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        if self.total_count >= self.max_count {
            return Ok(false);
        }

        self.matches.push(GrepMatch {
            path: self.path.to_string(),
            line_number: mat.line_number().unwrap_or(0),
            content: String::from_utf8_lossy(mat.bytes()).trim_end().to_string(),
            is_context: false,
        });
        self.total_count += 1;

        // Return false to stop once we've hit max count.
        Ok(self.total_count < self.max_count)
    }

    fn context(&mut self, _: &Searcher, ctx: &SinkContext<'_>) -> Result<bool, Self::Error> {
        self.matches.push(GrepMatch {
            path: self.path.to_string(),
            line_number: ctx.line_number().unwrap_or(0),
            content: String::from_utf8_lossy(ctx.bytes()).trim_end().to_string(),
            is_context: true,
        });
        Ok(true)
    }

    fn context_break(&mut self, _: &Searcher) -> Result<bool, Self::Error> {
        // Add a separator marker (empty line_number signals break)
        self.matches.push(GrepMatch {
            path: self.path.to_string(),
            line_number: 0,
            content: "--".to_string(),
            is_context: true,
        });
        Ok(true)
    }
}

/// Check if a path matches a glob pattern
fn matches_glob(path: &str, pattern: &str) -> bool {
    // Simple glob matching: support *, **, and ?
    let pattern = pattern.trim();

    // Handle negation
    let (negated, pattern) = if let Some(stripped) = pattern.strip_prefix('!') {
        (true, stripped)
    } else {
        (false, pattern)
    };

    let matched = if pattern.contains("**") {
        // ** matches any path segments
        let parts: Vec<&str> = pattern.split("**").collect();
        if parts.len() == 2 {
            let (prefix, suffix) = (
                parts[0].trim_end_matches('/'),
                parts[1].trim_start_matches('/'),
            );
            (prefix.is_empty() || path.starts_with(prefix))
                && (suffix.is_empty() || path.ends_with(suffix) || simple_glob_match(path, suffix))
        } else {
            simple_glob_match(path, pattern)
        }
    } else {
        simple_glob_match(path, pattern)
    };

    if negated { !matched } else { matched }
}

fn simple_glob_match(text: &str, pattern: &str) -> bool {
    // Match *.ext style patterns against filename or full path
    if pattern.starts_with("*.") {
        let ext = &pattern[1..]; // includes the dot
        text.ends_with(ext)
    } else if pattern.contains('*') {
        // Convert glob to regex-like matching
        let regex_pattern = pattern
            .replace('.', r"\.")
            .replace('*', ".*")
            .replace('?', ".");
        regex::Regex::new(&format!("^{}$", regex_pattern))
            .map(|re| re.is_match(text))
            .unwrap_or(false)
    } else {
        // Exact match or path contains pattern
        text == pattern || text.ends_with(&format!("/{}", pattern)) || text.contains(pattern)
    }
}

/// Search a repository for a pattern with options
pub fn grep_repo(repo: &gix::Repository, pattern: &str) -> anyhow::Result<GrepResult> {
    grep_repo_with_options(repo, pattern, &GrepOptions::default())
}

/// Search a repository for a pattern with full options
pub fn grep_repo_with_options(
    repo: &gix::Repository,
    pattern: &str,
    opts: &GrepOptions,
) -> anyhow::Result<GrepResult> {
    // Build the regex matcher with options
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(opts.ignore_case)
        .case_smart(opts.smart_case)
        .word(opts.word_regexp)
        .build(pattern)?;

    let tree = repo.head_commit()?.tree()?;
    let ignore_matcher = IgnoreMatcher::new(&opts.ignore)?;

    let mut recorder = gix::traverse::tree::Recorder::default();
    tree.traverse().breadthfirst(&mut recorder)?;

    // Collect matching files for -l and -c modes
    let mut file_matches: Vec<String> = Vec::new();
    let mut file_counts: Vec<(String, usize)> = Vec::new();
    let mut all_matches: Vec<GrepMatch> = Vec::new();
    let mut total_match_count = 0usize;

    for entry in recorder.records.iter().filter(|e| e.mode.is_blob()) {
        let path = entry.filepath.to_str()?.to_string();

        if ignore_matcher.is_ignored(&path) {
            continue;
        }

        // Apply glob filter if specified
        if opts
            .glob
            .as_ref()
            .is_some_and(|glob| !matches_glob(&path, glob))
        {
            continue;
        }

        let object = repo.find_object(entry.oid)?;
        let blob = object.into_blob();

        // Skip binary files
        if blob.data.find_byte(0).is_some() {
            continue;
        }

        let remaining = opts
            .max_count
            .map_or(usize::MAX, |max| max.saturating_sub(total_match_count));

        // Early exit if we've hit max count
        if remaining == 0 {
            break;
        }

        let mut file_match_list: Vec<GrepMatch> = Vec::new();

        let mut searcher_builder = SearcherBuilder::new();
        searcher_builder.line_number(true);
        searcher_builder.invert_match(opts.invert_match);

        if opts.before_context > 0 || opts.after_context > 0 {
            searcher_builder.before_context(opts.before_context);
            searcher_builder.after_context(opts.after_context);
        }

        searcher_builder.build().search_slice(
            &matcher,
            &blob.data,
            MatchCollector {
                path: &path,
                matches: &mut file_match_list,
                max_count: remaining,
                total_count: 0,
            },
        )?;

        let match_count = file_match_list.iter().filter(|m| !m.is_context).count();

        if match_count > 0 {
            if opts.files_with_matches {
                file_matches.push(path.clone());
            } else if opts.count {
                file_counts.push((path.clone(), match_count));
            } else {
                all_matches.extend(file_match_list);
            }
            total_match_count += match_count;
        }
    }

    if opts.files_with_matches {
        Ok(GrepResult::Files(file_matches))
    } else if opts.count {
        Ok(GrepResult::Counts(file_counts))
    } else {
        Ok(GrepResult::Matches(all_matches))
    }
}

pub fn read_file(repo: &gix::Repository, path: &str) -> anyhow::Result<String> {
    read_file_with_ignore(repo, path, &[])
}

pub fn read_file_with_ignore(
    repo: &gix::Repository,
    path: &str,
    ignore_patterns: &[String],
) -> anyhow::Result<String> {
    let ignore_matcher = IgnoreMatcher::new(ignore_patterns)?;
    let normalized_path = normalize_repo_path(path);
    if ignore_matcher.is_ignored(&normalized_path) {
        anyhow::bail!("File '{}' is excluded by --ignore", path);
    }

    let tree = repo.head_commit()?.tree()?;

    let entry = tree
        .lookup_entry_by_path(path)?
        .ok_or_else(|| anyhow::anyhow!("File not found: {}", path))?;

    let object = entry.object()?;
    let blob = object.into_blob();
    let content = blob.data.to_str()?;

    Ok(content.to_string())
}

/// Output the first N lines of a file (like POSIX head)
///
/// - `count`: number of lines to output (default behavior: 10)
/// - `number`: if true, prefix each line with its line number
pub fn head(
    repo: &gix::Repository,
    path: &str,
    count: usize,
    number: bool,
) -> anyhow::Result<String> {
    head_with_ignore(repo, path, count, number, &[])
}

pub fn head_with_ignore(
    repo: &gix::Repository,
    path: &str,
    count: usize,
    number: bool,
    ignore_patterns: &[String],
) -> anyhow::Result<String> {
    let content = read_file_with_ignore(repo, path, ignore_patterns)?;
    let selected: Vec<&str> = content.lines().take(count).collect();

    if number {
        Ok(selected
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}  {}", i + 1, line))
            .collect::<Vec<_>>()
            .join("\n"))
    } else {
        Ok(selected.join("\n"))
    }
}

/// Output the last N lines of a file (like POSIX tail)
///
/// - `count`: number of lines to output from the end
/// - `from_line`: if Some(n), output starting from line n to end (like `tail -n +N`)
/// - `number`: if true, prefix each line with its line number
pub fn tail(
    repo: &gix::Repository,
    path: &str,
    count: usize,
    from_line: Option<usize>,
    number: bool,
) -> anyhow::Result<String> {
    tail_with_ignore(repo, path, count, from_line, number, &[])
}

pub fn tail_with_ignore(
    repo: &gix::Repository,
    path: &str,
    count: usize,
    from_line: Option<usize>,
    number: bool,
    ignore_patterns: &[String],
) -> anyhow::Result<String> {
    let content = read_file_with_ignore(repo, path, ignore_patterns)?;
    let all_lines: Vec<&str> = content.lines().collect();
    let total = all_lines.len();

    let (selected, start_line_num): (Vec<&str>, usize) = if let Some(start) = from_line {
        // tail -n +N: output starting from line N (1-indexed)
        let skip = start.saturating_sub(1);
        (all_lines.into_iter().skip(skip).collect(), start)
    } else {
        // tail -n N: output last N lines
        let skip = total.saturating_sub(count);
        (all_lines.into_iter().skip(skip).collect(), skip + 1)
    };

    if number {
        Ok(selected
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}  {}", start_line_num + i, line))
            .collect::<Vec<_>>()
            .join("\n"))
    } else {
        Ok(selected.join("\n"))
    }
}

/// Count lines in a byte slice. Empty data is 0 lines; any content starts at 1.
fn count_lines(data: &[u8]) -> usize {
    if data.is_empty() {
        return 0;
    }
    let newlines = data.iter().filter(|&&b| b == b'\n').count();
    if data.last() == Some(&b'\n') {
        newlines
    } else {
        newlines + 1
    }
}

/// Metadata about a file in the repository
pub struct FileMetadata {
    pub name: String,
    pub is_dir: bool,
    pub size_bytes: Option<u64>,
    pub lines: Option<usize>,
    pub is_binary: bool,
}

/// List immediate children of a directory in the repository.
/// If `long` is true, includes byte size, line count, and token estimate for files.
pub fn list_dir(
    repo: &gix::Repository,
    path: Option<&str>,
    long: bool,
) -> anyhow::Result<Vec<FileMetadata>> {
    list_dir_with_ignore(repo, path, long, &[])
}

pub fn list_dir_with_ignore(
    repo: &gix::Repository,
    path: Option<&str>,
    long: bool,
    ignore_patterns: &[String],
) -> anyhow::Result<Vec<FileMetadata>> {
    let ignore_matcher = IgnoreMatcher::new(ignore_patterns)?;
    let tree = repo.head_commit()?.tree()?;

    let mut recorder = gix::traverse::tree::Recorder::default();
    tree.traverse().breadthfirst(&mut recorder)?;

    let prefix = path.map(|s| s.trim_end_matches('/')).unwrap_or("");

    // Collect immediate children: deduplicate directory components
    let mut dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // file_name -> (oid, is first occurrence)
    let mut files: std::collections::BTreeMap<String, gix::ObjectId> =
        std::collections::BTreeMap::new();

    for entry in recorder.records.iter().filter(|e| e.mode.is_blob()) {
        let full_path = entry.filepath.to_str()?.to_string();
        if ignore_matcher.is_ignored(&full_path) {
            continue;
        }

        let relative = if prefix.is_empty() {
            full_path.as_str()
        } else if let Some(rest) = full_path.strip_prefix(prefix) {
            if let Some(rest) = rest.strip_prefix('/') {
                rest
            } else {
                continue;
            }
        } else {
            continue;
        };

        // Split into first component and rest
        if let Some(slash_pos) = relative.find('/') {
            // This blob is inside a subdirectory -- record the dir name
            dirs.insert(relative[..slash_pos].to_string());
        } else {
            // Direct child file
            files.insert(relative.to_string(), entry.oid);
        }
    }

    let mut results: Vec<FileMetadata> = Vec::new();

    // Directories first (sorted)
    for name in &dirs {
        results.push(FileMetadata {
            name: name.clone(),
            is_dir: true,
            size_bytes: None,
            lines: None,
            is_binary: false,
        });
    }

    // Files (sorted)
    for (name, oid) in &files {
        if long {
            let object = repo.find_object(*oid)?;
            let blob = object.into_blob();
            let is_binary = blob.data.find_byte(0).is_some();
            let (size_bytes, lines) = if is_binary {
                (Some(blob.data.len() as u64), None)
            } else {
                let line_count = count_lines(&blob.data);
                (Some(blob.data.len() as u64), Some(line_count))
            };
            results.push(FileMetadata {
                name: name.clone(),
                is_dir: false,
                size_bytes,
                lines,
                is_binary,
            });
        } else {
            results.push(FileMetadata {
                name: name.clone(),
                is_dir: false,
                size_bytes: None,
                lines: None,
                is_binary: false,
            });
        }
    }

    Ok(results)
}

pub fn build_tree(repo: &gix::Repository, subdir: Option<&str>, long: bool) -> anyhow::Result<()> {
    build_tree_with_ignore(repo, subdir, long, &[])
}

pub fn build_tree_with_ignore(
    repo: &gix::Repository,
    subdir: Option<&str>,
    long: bool,
    ignore_patterns: &[String],
) -> anyhow::Result<()> {
    let ignore_matcher = IgnoreMatcher::new(ignore_patterns)?;
    let tree = repo.head_commit()?.tree()?;

    let mut recorder = gix::traverse::tree::Recorder::default();
    tree.traverse().breadthfirst(&mut recorder)?;

    // Normalize the subdir path (remove trailing slash if present)
    let subdir = subdir.map(|s| s.trim_end_matches('/'));

    // Determine root label and prefix to strip
    let (root_label, prefix_to_strip) = match subdir {
        Some(dir) => (
            dir.split('/').next_back().unwrap_or(dir).to_string(),
            Some(dir),
        ),
        None => (".".to_string(), None),
    };

    let mut builder = TreeBuilder::new(root_label);

    // Filter to only blobs (files) within the subdir and sort for consistent ordering
    let mut paths: Vec<_> = recorder
        .records
        .iter()
        .filter(|e| e.mode.is_blob())
        .filter(|e| {
            e.filepath
                .to_str()
                .map(|path| !ignore_matcher.is_ignored(path))
                .unwrap_or(false)
        })
        .filter(|e| match prefix_to_strip {
            Some(prefix) => {
                let path = e.filepath.to_str().unwrap();
                path.starts_with(prefix)
                    && (path.len() == prefix.len()
                        || path.as_bytes().get(prefix.len()) == Some(&b'/'))
            }
            None => true,
        })
        .collect();
    paths.sort_by_key(|e| &e.filepath);

    // Track current directory stack for navigating the tree
    let mut current_stack: Vec<&str> = Vec::new();

    for entry in paths {
        let path_str = entry.filepath.to_str().unwrap();

        // Strip the prefix if we're in a subdir
        let relative_path = match prefix_to_strip {
            Some(prefix) => path_str
                .strip_prefix(prefix)
                .unwrap_or(path_str)
                .trim_start_matches('/'),
            None => path_str,
        };

        let parts: Vec<_> = relative_path.split('/').collect();

        // Find common prefix length with current stack
        let common_len = current_stack
            .iter()
            .zip(parts.iter())
            .take_while(|(a, b)| a == b)
            .count();

        // Pop back to common prefix (close directories we've left)
        while current_stack.len() > common_len {
            builder.end_child();
            current_stack.pop();
        }

        // Push new directories (all parts except the last one, which is the file)
        for &part in &parts[common_len..parts.len().saturating_sub(1)] {
            builder.begin_child(part.to_string());
            current_stack.push(part);
        }

        // Add the file (last part)
        if let Some(&filename) = parts.last() {
            if long {
                let object = repo.find_object(entry.oid)?;
                let blob = object.into_blob();
                let is_binary = blob.data.find_byte(0).is_some();
                let label = if is_binary {
                    format!("{} [bin]", filename)
                } else {
                    let lines = count_lines(&blob.data);
                    let tokens = lines * 5;
                    format!("{} ({} ln, ~{} tok)", filename, lines, tokens)
                };
                builder.add_empty_child(label);
            } else {
                builder.add_empty_child(filename.to_string());
            }
        }
    }

    // Close all remaining open directories
    while !current_stack.is_empty() {
        builder.end_child();
        current_stack.pop();
    }

    print_tree(&builder.build())?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeText {
    pub text: String,
    pub entries: usize,
    pub truncated: bool,
}

#[derive(Default)]
struct TreeTextNode {
    children: BTreeMap<String, TreeTextNode>,
}

impl TreeTextNode {
    fn insert(&mut self, parts: &[String]) {
        let Some((head, tail)) = parts.split_first() else {
            return;
        };
        self.children.entry(head.clone()).or_default().insert(tail);
    }
}

pub fn tree_text_with_ignore(
    repo: &gix::Repository,
    subdir: Option<&str>,
    long: bool,
    ignore_patterns: &[String],
    max_entries: Option<usize>,
) -> anyhow::Result<TreeText> {
    let ignore_matcher = IgnoreMatcher::new(ignore_patterns)?;
    let tree = repo.head_commit()?.tree()?;

    let mut recorder = gix::traverse::tree::Recorder::default();
    tree.traverse().breadthfirst(&mut recorder)?;

    let subdir = subdir.map(|s| s.trim_end_matches('/'));
    let (root_label, prefix_to_strip) = match subdir {
        Some(dir) => (
            dir.split('/').next_back().unwrap_or(dir).to_string(),
            Some(dir),
        ),
        None => (".".to_string(), None),
    };

    let mut paths: Vec<_> = recorder
        .records
        .iter()
        .filter(|entry| entry.mode.is_blob())
        .filter(|entry| {
            entry
                .filepath
                .to_str()
                .map(|path| !ignore_matcher.is_ignored(path))
                .unwrap_or(false)
        })
        .filter(|entry| match prefix_to_strip {
            Some(prefix) => {
                let path = entry.filepath.to_str().unwrap();
                path.starts_with(prefix)
                    && (path.len() == prefix.len()
                        || path.as_bytes().get(prefix.len()) == Some(&b'/'))
            }
            None => true,
        })
        .collect();
    paths.sort_by_key(|entry| &entry.filepath);

    let total_entries = paths.len();
    let max_entries = max_entries.unwrap_or(usize::MAX);
    let mut root = TreeTextNode::default();
    let mut inserted = 0usize;

    for entry in paths.into_iter().take(max_entries) {
        let path_str = entry.filepath.to_str().unwrap();
        let relative_path = match prefix_to_strip {
            Some(prefix) => path_str
                .strip_prefix(prefix)
                .unwrap_or(path_str)
                .trim_start_matches('/'),
            None => path_str,
        };

        if relative_path.is_empty() {
            continue;
        }

        let mut parts = relative_path
            .split('/')
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        if long && let Some(filename) = parts.last_mut() {
            let object = repo.find_object(entry.oid)?;
            let blob = object.into_blob();
            if blob.data.find_byte(0).is_some() {
                *filename = format!("{filename} [bin]");
            } else {
                let lines = count_lines(&blob.data);
                *filename = format!("{filename} ({lines} ln, ~{} tok)", lines * 5);
            }
        }

        root.insert(&parts);
        inserted += 1;
    }

    let mut lines = vec![root_label];
    render_tree_text(&root.children, "", &mut lines);

    Ok(TreeText {
        text: lines.join("\n"),
        entries: inserted,
        truncated: inserted < total_entries,
    })
}

fn render_tree_text(
    children: &BTreeMap<String, TreeTextNode>,
    prefix: &str,
    lines: &mut Vec<String>,
) {
    let len = children.len();
    for (index, (name, child)) in children.iter().enumerate() {
        let is_last = index + 1 == len;
        let connector = if is_last { "└── " } else { "├── " };
        lines.push(format!("{prefix}{connector}{name}"));

        let child_prefix = if is_last {
            format!("{prefix}    ")
        } else {
            format!("{prefix}│   ")
        };
        render_tree_text(&child.children, &child_prefix, lines);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_branch_paths_include_owner_repo_and_branch() {
        let cache_dir = Path::new("/tmp/wit-cache");
        let main = CacheTarget::new("owner/repo", "main").unwrap();
        let feature = CacheTarget::new("owner/repo", "feature/x").unwrap();

        assert_eq!(
            main.repo_path(cache_dir),
            cache_dir
                .join("owner")
                .join("repo")
                .join("branches")
                .join("b-main")
                .join("repo.git")
        );
        assert_eq!(
            feature.repo_path(cache_dir),
            cache_dir
                .join("owner")
                .join("repo")
                .join("branches")
                .join("b-feature%2Fx")
                .join("repo.git")
        );
        assert_ne!(main.repo_path(cache_dir), feature.repo_path(cache_dir));
    }

    #[test]
    fn cache_branch_paths_percent_encode_without_collisions() {
        let slash = CacheTarget::new("owner/repo", "feature/x").unwrap();
        let literal_percent = CacheTarget::new("owner/repo", "feature%2Fx").unwrap();
        let lowercase_percent = CacheTarget::new("owner/repo", "feature%2fx").unwrap();
        let uppercase = CacheTarget::new("owner/repo", "Feature/x").unwrap();
        let dot = CacheTarget::new("owner/repo", ".").unwrap();
        let reserved = CacheTarget::new("owner/repo", "con").unwrap();

        let encoded = [
            slash.encoded_branch,
            literal_percent.encoded_branch,
            lowercase_percent.encoded_branch,
            uppercase.encoded_branch,
            dot.encoded_branch,
            reserved.encoded_branch,
        ];

        assert_eq!(encoded[0], "b-feature%2Fx");
        assert_eq!(encoded[1], "b-feature%252%46x");
        assert_eq!(encoded[2], "b-feature%252fx");
        assert_eq!(encoded[3], "b-%46eature%2Fx");
        assert_eq!(encoded[4], "b-%2E");
        assert_eq!(encoded[5], "b-con");

        let unique: HashSet<&String> = encoded.iter().collect();
        assert_eq!(unique.len(), encoded.len());
    }

    #[test]
    fn cache_branch_paths_reject_invalid_identity() {
        assert!(CacheTarget::new("owner", "main").is_err());
        assert!(CacheTarget::new("owner/repo/extra", "main").is_err());
        assert!(CacheTarget::new("../victim", "main").is_err());
        assert!(CacheTarget::new("owner/..", "main").is_err());
        assert!(CacheTarget::new("./repo", "main").is_err());
        assert!(CacheTarget::new("owner/.git", "main").is_ok());
        assert!(CacheTarget::new("own er/repo", "main").is_err());
        assert!(CacheTarget::new("owner/repo\nnext", "main").is_err());
        assert!(CacheTarget::new(r"owner\repo/name", "main").is_err());
        assert!(CacheTarget::new("owner/repo", "").is_err());
    }

    fn run_git(args: &[&str], workdir: Option<&Path>) {
        let mut command = Command::new("git");
        command.args(args);
        if let Some(dir) = workdir {
            command.current_dir(dir);
        }
        let output = command.output().expect("git command should run");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(args: &[&str], workdir: Option<&Path>) -> String {
        let mut command = Command::new("git");
        command.args(args);
        if let Some(dir) = workdir {
            command.current_dir(dir);
        }
        let output = command.output().expect("git command should run");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("git stdout should be utf-8")
            .trim()
            .to_string()
    }

    fn create_remote_with_default_branch(
        temp: &tempfile::TempDir,
        default_branch: &str,
    ) -> (PathBuf, String) {
        let worktree = temp.path().join("worktree");
        let bare_repo = temp.path().join("remote.git");

        run_git(&["init", worktree.to_str().unwrap()], None);
        run_git(&["checkout", "-b", default_branch], Some(&worktree));
        std::fs::write(worktree.join("README.md"), "hello\n").unwrap();
        run_git(&["add", "README.md"], Some(&worktree));
        run_git(
            &[
                "-c",
                "user.name=wit-test",
                "-c",
                "user.email=wit-test@example.com",
                "commit",
                "-m",
                "init",
            ],
            Some(&worktree),
        );

        run_git(&["init", "--bare", bare_repo.to_str().unwrap()], None);
        run_git(
            &["remote", "add", "origin", bare_repo.to_str().unwrap()],
            Some(&worktree),
        );
        run_git(&["push", "origin", default_branch], Some(&worktree));
        let branch_ref = format!("refs/heads/{default_branch}");
        run_git(&["symbolic-ref", "HEAD", &branch_ref], Some(&bare_repo));
        let sha = git_stdout(&["rev-parse", "HEAD"], Some(&worktree));

        (bare_repo, sha)
    }

    fn commit_and_push_file(temp: &tempfile::TempDir, branch: &str, content: &str) -> String {
        let worktree = temp.path().join("worktree");
        std::fs::write(worktree.join("README.md"), content).unwrap();
        run_git(&["add", "README.md"], Some(&worktree));
        run_git(
            &[
                "-c",
                "user.name=wit-test",
                "-c",
                "user.email=wit-test@example.com",
                "commit",
                "-m",
                "update",
            ],
            Some(&worktree),
        );
        run_git(&["push", "origin", branch], Some(&worktree));
        git_stdout(&["rev-parse", "HEAD"], Some(&worktree))
    }

    fn create_bare_repo_with_commit(temp: &tempfile::TempDir, bare_repo: &Path) {
        let worktree = temp.path().join("atomic-worktree");
        run_git(&["init", worktree.to_str().unwrap()], None);
        run_git(&["checkout", "-b", "main"], Some(&worktree));
        std::fs::write(worktree.join("README.md"), "hello\n").unwrap();
        run_git(&["add", "README.md"], Some(&worktree));
        run_git(
            &[
                "-c",
                "user.name=wit-test",
                "-c",
                "user.email=wit-test@example.com",
                "commit",
                "-m",
                "init",
            ],
            Some(&worktree),
        );
        if let Some(parent) = bare_repo.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        run_git(
            &[
                "clone",
                "--bare",
                worktree.to_str().unwrap(),
                bare_repo.to_str().unwrap(),
            ],
            None,
        );
    }

    fn resolved_cache_target_for_test(branch: &str, sha: &str) -> ResolvedCacheTarget {
        ResolvedCacheTarget {
            target: CacheTarget::new("owner/repo", branch).unwrap(),
            remote_url: "/tmp/remote.git".to_string(),
            branch: ResolvedBranch {
                name: branch.to_string(),
                current_sha: sha.to_string(),
            },
        }
    }

    #[test]
    fn cache_default_branch_resolution_uses_remote_head_branch_and_sha() {
        let temp = tempfile::tempdir().unwrap();
        let (remote, sha) = create_remote_with_default_branch(&temp, "trunk");
        let cache_dir = temp.path().join("cache");

        let resolved = default_cache_target("owner/repo", remote.to_str().unwrap()).unwrap();

        assert_eq!(resolved.branch.name, "trunk");
        assert_eq!(resolved.branch.current_sha, sha);
        assert_eq!(resolved.target.owner_repo, "owner/repo");
        assert_eq!(resolved.target.branch, "trunk");
        assert_eq!(
            resolved.target.repo_path(&cache_dir),
            cache_dir
                .join("owner")
                .join("repo")
                .join("branches")
                .join("b-trunk")
                .join("repo.git")
        );
    }

    #[test]
    fn cache_default_branch_resolution_preserves_slash_branch_key() {
        let temp = tempfile::tempdir().unwrap();
        let (remote, sha) = create_remote_with_default_branch(&temp, "release/v1");
        let cache_dir = temp.path().join("cache");

        let resolved = default_cache_target("owner/repo", remote.to_str().unwrap()).unwrap();

        assert_eq!(resolved.branch.name, "release/v1");
        assert_eq!(resolved.branch.current_sha, sha);
        assert_eq!(
            resolved.target.repo_path(&cache_dir),
            cache_dir
                .join("owner")
                .join("repo")
                .join("branches")
                .join("b-release%2Fv1")
                .join("repo.git")
        );
    }

    #[test]
    fn cache_default_branch_resolution_rejects_unresolved_head() {
        let err = parse_default_branch_ls_remote("abc123\tHEAD\n").unwrap_err();
        assert!(err.to_string().contains("remote HEAD did not resolve"));
    }

    #[test]
    fn cache_default_branch_resolution_uses_cached_metadata_without_remote() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join("cache");
        let resolved =
            resolved_cache_target_for_test("trunk", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let metadata = CacheMetadata::new(&resolved, 100, 200);
        write_cache_metadata(&resolved.target.metadata_path(&cache_dir), &metadata).unwrap();

        let cached = default_cache_target_for_cache(
            "owner/repo",
            "/tmp/remote.git",
            &cache_dir,
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .unwrap();

        assert_eq!(cached.branch.name, "trunk");
        assert_eq!(
            cached.branch.current_sha,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(
            cached.target.repo_path(&cache_dir),
            resolved.target.repo_path(&cache_dir)
        );
    }

    #[test]
    fn cache_default_branch_resolution_cache_metadata_writes_real_cache_path() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join("cache");
        let legacy_path = repo_cache_root(&cache_dir, "owner/repo").unwrap();
        create_bare_repo_with_commit(&temp, &legacy_path);
        assert!(is_legacy_bare_cache_dir(&legacy_path));

        let (remote, sha) = create_remote_with_default_branch(&temp, "trunk");
        remove_legacy_repo_cache_if_present(&cache_dir, "owner/repo").unwrap();
        let resolved = default_cache_target("owner/repo", remote.to_str().unwrap()).unwrap();
        let repo = cache_github_repo_target(
            &resolved,
            &cache_dir,
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .unwrap();
        let repo_path = cache_dir
            .join("owner")
            .join("repo")
            .join("branches")
            .join("b-trunk")
            .join("repo.git");
        let metadata_path = repo_path.parent().unwrap().join(CACHE_METADATA_FILE);

        assert_eq!(repo.path(), repo_path);
        assert!(cache_has_head_commit(&repo));
        assert!(!legacy_path.join("HEAD").exists());
        assert!(!legacy_path.join("objects").exists());
        assert!(repo_path.exists());

        let metadata = read_cache_metadata(&metadata_path).unwrap();
        assert_eq!(metadata.owner_repo, "owner/repo");
        assert_eq!(metadata.branch, "trunk");
        assert_eq!(metadata.remote_url, remote.to_str().unwrap());
        assert_eq!(metadata.current_sha, sha);
        assert_eq!(metadata.cache_schema_version, CACHE_SCHEMA_VERSION);
        assert!(metadata.last_checked_at > 0);
        assert!(metadata.last_updated_at > 0);
    }

    #[test]
    fn cache_swr_serves_cached_first() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join("cache");
        let (remote, old_sha) = create_remote_with_default_branch(&temp, "trunk");
        let remote_url = remote.to_str().unwrap();
        let resolved = default_cache_target("owner/repo", remote_url).unwrap();

        let repo =
            cache_github_repo_target(&resolved, &cache_dir, CacheAcquisitionMode::ForceInvalidate)
                .unwrap();
        assert_eq!(read_file(&repo, "README.md").unwrap(), "hello\n");

        let new_sha = commit_and_push_file(&temp, "trunk", "new content\n");
        let cached = default_cache_target_for_cache(
            "owner/repo",
            remote_url,
            &cache_dir,
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .unwrap();
        assert_eq!(cached.branch.current_sha, old_sha);

        let stale_repo = cache_github_repo_target(
            &cached,
            &cache_dir,
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .unwrap();
        assert_eq!(read_file(&stale_repo, "README.md").unwrap(), "hello\n");

        revalidate_cache_target(&cached, &cache_dir).unwrap();

        let refreshed_repo = gix::open(cached.target.repo_path(&cache_dir)).unwrap();
        assert_eq!(
            read_file(&refreshed_repo, "README.md").unwrap(),
            "new content\n"
        );
        let metadata = read_cache_metadata(&cached.target.metadata_path(&cache_dir)).unwrap();
        assert_eq!(metadata.current_sha, new_sha);
        assert!(metadata.last_error.is_none());
    }

    #[test]
    fn cache_swr_refreshes_on_sha_change_updates_checked_at_when_sha_matches() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join("cache");
        let (remote, old_sha) = create_remote_with_default_branch(&temp, "trunk");
        let remote_url = remote.to_str().unwrap();
        let resolved = default_cache_target("owner/repo", remote_url).unwrap();
        cache_github_repo_target(&resolved, &cache_dir, CacheAcquisitionMode::ForceInvalidate)
            .unwrap();

        let metadata_path = resolved.target.metadata_path(&cache_dir);
        let mut metadata = read_cache_metadata(&metadata_path).unwrap();
        metadata.last_checked_at = 1;
        metadata.last_updated_at = 2;
        metadata.last_error = Some("old error".to_string());
        write_cache_metadata(&metadata_path, &metadata).unwrap();

        revalidate_cache_target(&resolved, &cache_dir).unwrap();

        let checked = read_cache_metadata(&metadata_path).unwrap();
        assert_eq!(checked.current_sha, old_sha);
        assert!(checked.last_checked_at > 1);
        assert_eq!(checked.last_updated_at, 2);
        assert!(checked.last_error.is_none());
    }

    #[test]
    fn cache_swr_refreshes_on_sha_change_refreshes_only_when_sha_differs() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join("cache");
        let (remote, old_sha) = create_remote_with_default_branch(&temp, "trunk");
        let remote_url = remote.to_str().unwrap();
        let resolved = default_cache_target("owner/repo", remote_url).unwrap();
        cache_github_repo_target(&resolved, &cache_dir, CacheAcquisitionMode::ForceInvalidate)
            .unwrap();

        let new_sha = commit_and_push_file(&temp, "trunk", "new content\n");
        assert_ne!(old_sha, new_sha);

        revalidate_cache_target(&resolved, &cache_dir).unwrap();

        let refreshed_repo = gix::open(resolved.target.repo_path(&cache_dir)).unwrap();
        assert_eq!(
            read_file(&refreshed_repo, "README.md").unwrap(),
            "new content\n"
        );
        let metadata = read_cache_metadata(&resolved.target.metadata_path(&cache_dir)).unwrap();
        assert_eq!(metadata.current_sha, new_sha);
        assert!(metadata.last_error.is_none());
    }

    #[test]
    fn cache_swr_refreshes_on_sha_change_preserves_cache_on_remote_failure() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join("cache");
        let (remote, old_sha) = create_remote_with_default_branch(&temp, "trunk");
        let remote_url = remote.to_str().unwrap();
        let resolved = default_cache_target("owner/repo", remote_url).unwrap();
        cache_github_repo_target(&resolved, &cache_dir, CacheAcquisitionMode::ForceInvalidate)
            .unwrap();
        let missing_remote = temp.path().join("missing.git");
        let failing = ResolvedCacheTarget {
            target: resolved.target.clone(),
            remote_url: missing_remote.to_string_lossy().to_string(),
            branch: resolved.branch.clone(),
        };

        let err = revalidate_cache_target(&failing, &cache_dir).unwrap_err();
        assert!(err.to_string().contains("git ls-remote failed"));

        let repo = gix::open(resolved.target.repo_path(&cache_dir)).unwrap();
        assert_eq!(read_file(&repo, "README.md").unwrap(), "hello\n");
        let metadata = read_cache_metadata(&resolved.target.metadata_path(&cache_dir)).unwrap();
        assert_eq!(metadata.current_sha, old_sha);
        assert!(metadata.last_error.is_some());
    }

    #[test]
    fn cache_force_invalidation_refreshes_before_returning() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join("cache");
        let (remote, old_sha) = create_remote_with_default_branch(&temp, "trunk");
        let remote_url = remote.to_str().unwrap();
        let initial = default_cache_target("owner/repo", remote_url).unwrap();
        cache_github_repo_target(&initial, &cache_dir, CacheAcquisitionMode::ForceInvalidate)
            .unwrap();

        let new_sha = commit_and_push_file(&temp, "trunk", "forced refresh\n");
        assert_ne!(old_sha, new_sha);
        let refreshed = default_cache_target_for_cache(
            "owner/repo",
            remote_url,
            &cache_dir,
            CacheAcquisitionMode::ForceInvalidate,
        )
        .unwrap();

        let repo = cache_github_repo_target(
            &refreshed,
            &cache_dir,
            CacheAcquisitionMode::ForceInvalidate,
        )
        .unwrap();

        assert_eq!(read_file(&repo, "README.md").unwrap(), "forced refresh\n");
        let metadata = read_cache_metadata(&refreshed.target.metadata_path(&cache_dir)).unwrap();
        assert_eq!(metadata.current_sha, new_sha);
        assert!(metadata.last_error.is_none());
    }

    #[test]
    fn cache_metadata_round_trips_required_fields() {
        let temp = tempfile::tempdir().unwrap();
        let resolved =
            resolved_cache_target_for_test("trunk", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let metadata_path = resolved.target.metadata_path(temp.path());
        let metadata = CacheMetadata::new(&resolved, 100, 200);

        write_cache_metadata(&metadata_path, &metadata).unwrap();

        let stored = read_cache_metadata(&metadata_path).unwrap();
        assert_eq!(stored, metadata);
        assert!(cache_metadata_is_usable(&metadata_path, &resolved));

        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&metadata_path).unwrap()).unwrap();
        for key in [
            "cache_schema_version",
            "owner_repo",
            "branch",
            "remote_url",
            "current_sha",
            "last_checked_at",
            "last_updated_at",
        ] {
            assert!(raw.get(key).is_some(), "metadata missing {key}");
        }
        assert_eq!(raw["owner_repo"], "owner/repo");
        assert_eq!(raw["branch"], "trunk");
        assert_eq!(raw["remote_url"], "/tmp/remote.git");
        assert_eq!(
            raw["current_sha"],
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(raw["cache_schema_version"], CACHE_SCHEMA_VERSION);
    }

    #[test]
    fn cache_metadata_rejects_missing_corrupt_or_incompatible_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let resolved =
            resolved_cache_target_for_test("trunk", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let metadata_path = resolved.target.metadata_path(temp.path());

        assert!(!cache_metadata_is_usable(&metadata_path, &resolved));

        std::fs::create_dir_all(metadata_path.parent().unwrap()).unwrap();
        std::fs::write(&metadata_path, b"{").unwrap();
        assert!(!cache_metadata_is_usable(&metadata_path, &resolved));

        let mut old_schema = CacheMetadata::new(&resolved, 100, 200);
        old_schema.cache_schema_version = 0;
        std::fs::write(&metadata_path, serde_json::to_vec(&old_schema).unwrap()).unwrap();
        assert!(!cache_metadata_is_usable(&metadata_path, &resolved));

        let other_branch = resolved_cache_target_for_test(
            "release/v1",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let wrong_identity = CacheMetadata::new(&other_branch, 100, 200);
        std::fs::write(&metadata_path, serde_json::to_vec(&wrong_identity).unwrap()).unwrap();
        assert!(!cache_metadata_is_usable(&metadata_path, &resolved));
    }

    #[test]
    fn cache_metadata_atomicity_preserves_valid_metadata_when_temp_write_is_partial() {
        let temp = tempfile::tempdir().unwrap();
        let resolved =
            resolved_cache_target_for_test("trunk", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let repo_path = resolved.target.repo_path(temp.path());
        let metadata_path = resolved.target.metadata_path(temp.path());
        let metadata = CacheMetadata::new(&resolved, 100, 200);

        create_bare_repo_with_commit(&temp, &repo_path);
        let repo = gix::open(&repo_path).unwrap();
        assert!(cache_has_head_commit(&repo));

        write_cache_metadata(&metadata_path, &metadata).unwrap();
        let updated = CacheMetadata::new(&resolved, 300, 400);
        let temp_path = write_cache_metadata_temp(&metadata_path, &updated).unwrap();

        let repo = gix::open(&repo_path).unwrap();
        assert!(cache_has_head_commit(&repo));
        assert_eq!(read_cache_metadata(&metadata_path).unwrap(), metadata);
        assert!(cache_metadata_is_usable(&metadata_path, &resolved));

        std::fs::write(&temp_path, b"{").unwrap();

        let repo = gix::open(&repo_path).unwrap();
        assert!(cache_has_head_commit(&repo));
        assert_eq!(read_cache_metadata(&metadata_path).unwrap(), metadata);
        assert!(cache_metadata_is_usable(&metadata_path, &resolved));

        write_cache_metadata(&metadata_path, &updated).unwrap();
        assert_eq!(read_cache_metadata(&metadata_path).unwrap(), updated);
        assert!(!temp_path.exists());
    }

    #[test]
    fn cache_metadata_atomicity_removes_stale_metadata_before_repo_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let resolved =
            resolved_cache_target_for_test("trunk", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let repo_path = resolved.target.repo_path(temp.path());
        let metadata_path = resolved.target.metadata_path(temp.path());
        let metadata = CacheMetadata::new(&resolved, 100, 200);

        create_bare_repo_with_commit(&temp, &repo_path);
        write_cache_metadata(&metadata_path, &metadata).unwrap();
        assert!(cache_metadata_is_usable(&metadata_path, &resolved));

        remove_cache_metadata(&metadata_path).unwrap();

        let repo = gix::open(&repo_path).unwrap();
        assert!(cache_has_head_commit(&repo));
        assert!(!cache_metadata_is_usable(&metadata_path, &resolved));
    }

    // ==================== Glob Matching Tests ====================

    #[test]
    fn test_glob_star_extension() {
        assert!(matches_glob("src/main.rs", "*.rs"));
        assert!(matches_glob("lib/utils/helper.rs", "*.rs"));
        assert!(!matches_glob("src/main.py", "*.rs"));
        assert!(!matches_glob("README.md", "*.rs"));
    }

    #[test]
    fn test_glob_double_star() {
        assert!(matches_glob("src/lib/mod.rs", "**/mod.rs"));
        assert!(matches_glob("mod.rs", "**/mod.rs"));
        assert!(matches_glob("a/b/c/mod.rs", "**/mod.rs"));
        assert!(!matches_glob("src/lib/main.rs", "**/mod.rs"));
    }

    #[test]
    fn test_glob_directory_prefix() {
        assert!(matches_glob("src/main.rs", "src/**"));
        assert!(matches_glob("src/lib/mod.rs", "src/**"));
        assert!(!matches_glob("tests/main.rs", "src/**"));
    }

    #[test]
    fn test_glob_negation() {
        assert!(!matches_glob("src/main.rs", "!*.rs"));
        assert!(matches_glob("src/main.py", "!*.rs"));
    }

    #[test]
    fn test_glob_exact_match() {
        assert!(matches_glob("Cargo.toml", "Cargo.toml"));
        assert!(matches_glob("src/Cargo.toml", "Cargo.toml"));
        assert!(!matches_glob("Cargo.lock", "Cargo.toml"));
    }

    // ==================== IgnoreMatcher Tests ====================

    #[test]
    fn test_ignore_matcher_literal_file_path() {
        let matcher = IgnoreMatcher::new(&["src/main.rs".to_string()]).unwrap();
        assert!(matcher.is_ignored("src/main.rs"));
        assert!(!matcher.is_ignored("src/lib.rs"));
    }

    #[test]
    fn test_ignore_matcher_literal_directory_path() {
        let matcher = IgnoreMatcher::new(&["src/generated".to_string()]).unwrap();
        assert!(matcher.is_ignored("src/generated/output.rs"));
        assert!(matcher.is_ignored("src/generated"));
        assert!(!matcher.is_ignored("src/core/mod.rs"));
    }

    #[test]
    fn test_ignore_matcher_component_name() {
        let matcher = IgnoreMatcher::new(&[".git".to_string()]).unwrap();
        assert!(matcher.is_ignored(".git/config"));
        assert!(matcher.is_ignored("vendor/.git/HEAD"));
        assert!(!matcher.is_ignored("src/gitops/mod.rs"));
    }

    #[test]
    fn test_ignore_matcher_glob_pattern() {
        let matcher = IgnoreMatcher::new(&["*.png".to_string()]).unwrap();
        assert!(matcher.is_ignored("assets/logo.png"));
        assert!(matcher.is_ignored("logo.png"));
        assert!(!matcher.is_ignored("assets/logo.svg"));
    }

    // ==================== GrepOptions Builder Tests ====================

    #[test]
    fn test_grep_options_default() {
        let opts = GrepOptions::default();
        assert!(!opts.ignore_case);
        assert!(!opts.smart_case);
        assert!(!opts.word_regexp);
        assert!(!opts.invert_match);
        assert_eq!(opts.max_count, None);
        assert_eq!(opts.before_context, 0);
        assert_eq!(opts.after_context, 0);
        assert!(opts.glob.is_none());
        assert!(!opts.files_with_matches);
        assert!(!opts.count);
        assert!(opts.ignore.is_empty());
    }

    #[test]
    fn test_grep_options_builder() {
        let opts = GrepOptions::new()
            .ignore_case(true)
            .word_regexp(true)
            .max_count(10)
            .context(3)
            .glob(Some("*.rs".to_string()))
            .ignore(vec![".git".to_string(), "*.png".to_string()])
            .files_with_matches(true);

        assert!(opts.ignore_case);
        assert!(opts.word_regexp);
        assert_eq!(opts.max_count, Some(10));
        assert_eq!(opts.before_context, 3);
        assert_eq!(opts.after_context, 3);
        assert_eq!(opts.glob, Some("*.rs".to_string()));
        assert_eq!(opts.ignore, vec![".git".to_string(), "*.png".to_string()]);
        assert!(opts.files_with_matches);
    }

    #[test]
    fn test_grep_options_separate_context() {
        let opts = GrepOptions::new().before_context(2).after_context(5);

        assert_eq!(opts.before_context, 2);
        assert_eq!(opts.after_context, 5);
    }

    // ==================== GrepMatch Tests ====================

    #[test]
    fn test_grep_match_equality() {
        let m1 = GrepMatch {
            path: "src/main.rs".to_string(),
            line_number: 10,
            content: "fn main()".to_string(),
            is_context: false,
        };
        let m2 = GrepMatch {
            path: "src/main.rs".to_string(),
            line_number: 10,
            content: "fn main()".to_string(),
            is_context: false,
        };
        assert_eq!(m1, m2);
    }

    #[test]
    fn test_grep_match_context_flag() {
        let match_line = GrepMatch {
            path: "test.rs".to_string(),
            line_number: 5,
            content: "match line".to_string(),
            is_context: false,
        };
        let context_line = GrepMatch {
            path: "test.rs".to_string(),
            line_number: 4,
            content: "context line".to_string(),
            is_context: true,
        };

        assert!(!match_line.is_context);
        assert!(context_line.is_context);
    }

    #[test]
    fn test_should_fallback_to_git_cli_on_timeout_error() {
        let timeout_err = anyhow::anyhow!("operation timed out while streaming");
        assert!(should_fallback_to_git_cli(&timeout_err));
    }

    #[test]
    fn test_should_not_fallback_to_git_cli_on_non_timeout_error() {
        let auth_err = anyhow::anyhow!("received HTTP status 401");
        assert!(!should_fallback_to_git_cli(&auth_err));
    }

    #[test]
    fn test_cache_has_head_commit_false_for_empty_bare_repo() {
        let temp = tempfile::tempdir().unwrap();
        let bare_repo = temp.path().join("empty.git");
        run_git(&["init", "--bare", bare_repo.to_str().unwrap()], None);

        let repo = gix::open(&bare_repo).unwrap();
        assert!(!cache_has_head_commit(&repo));
    }

    #[test]
    fn test_cache_has_head_commit_true_for_repo_with_commit() {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("worktree");
        let bare_repo = temp.path().join("with-commit.git");

        run_git(&["init", worktree.to_str().unwrap()], None);
        std::fs::write(worktree.join("README.md"), "hello").unwrap();
        run_git(&["add", "README.md"], Some(&worktree));
        run_git(
            &[
                "-c",
                "user.name=wit-test",
                "-c",
                "user.email=wit-test@example.com",
                "commit",
                "-m",
                "init",
            ],
            Some(&worktree),
        );
        run_git(
            &[
                "clone",
                "--bare",
                worktree.to_str().unwrap(),
                bare_repo.to_str().unwrap(),
            ],
            None,
        );

        let repo = gix::open(&bare_repo).unwrap();
        assert!(cache_has_head_commit(&repo));
    }

    #[test]
    fn cache_branch_locks_same_branch_serializes_unrelated_branches_do_not() {
        let temp = tempfile::tempdir().unwrap();
        let main = CacheTarget::new("owner/repo", "main").unwrap();
        let feature = CacheTarget::new("owner/repo", "feature/x").unwrap();
        let other_repo = CacheTarget::new("owner/other", "main").unwrap();
        let first_lock = acquire_cache_lock(&main.lock_path(temp.path())).unwrap();

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let same_path = main.lock_path(temp.path());

        let handle = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _second_lock = acquire_cache_lock(&same_path).unwrap();
            acquired_tx.send(()).unwrap();
        });

        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("worker thread did not start");
        assert!(
            acquired_rx
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "second lock should block while first lock is held"
        );

        let _feature_lock = acquire_cache_lock(&feature.lock_path(temp.path())).unwrap();
        let _other_repo_lock = acquire_cache_lock(&other_repo.lock_path(temp.path())).unwrap();

        drop(first_lock);

        acquired_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("worker thread did not acquire lock after release");
        handle.join().unwrap();
    }

    #[test]
    fn test_grep_repo_max_count_zero_returns_no_matches() {
        let temp = tempfile::tempdir().unwrap();
        let repo_dir = temp.path().join("repo");

        run_git(&["init", repo_dir.to_str().unwrap()], None);
        std::fs::write(repo_dir.join("README.md"), "Hello\nHello\n").unwrap();
        run_git(&["add", "README.md"], Some(&repo_dir));
        run_git(
            &[
                "-c",
                "user.name=wit-test",
                "-c",
                "user.email=wit-test@example.com",
                "commit",
                "-m",
                "init",
            ],
            Some(&repo_dir),
        );

        let repo = gix::open(&repo_dir).unwrap();
        let opts = GrepOptions::new().max_count(0);
        let result = grep_repo_with_options(&repo, "Hello", &opts).unwrap();

        if let GrepResult::Matches(matches) = result {
            assert!(
                matches.is_empty(),
                "max_count=0 should behave like ripgrep and return no matches"
            );
        } else {
            panic!("Expected Matches result");
        }
    }

    #[test]
    fn test_grep_repo_with_ignore_patterns_skips_ignored_files() {
        let temp = tempfile::tempdir().unwrap();
        let repo_dir = temp.path().join("repo");

        run_git(&["init", repo_dir.to_str().unwrap()], None);
        std::fs::create_dir_all(repo_dir.join(".hidden")).unwrap();
        std::fs::write(repo_dir.join("README.md"), "needle\n").unwrap();
        std::fs::write(repo_dir.join(".hidden").join("secret.txt"), "needle\n").unwrap();
        run_git(&["add", "."], Some(&repo_dir));
        run_git(
            &[
                "-c",
                "user.name=wit-test",
                "-c",
                "user.email=wit-test@example.com",
                "commit",
                "-m",
                "init",
            ],
            Some(&repo_dir),
        );

        let repo = gix::open(&repo_dir).unwrap();
        let opts = GrepOptions::new().ignore(vec![".hidden".to_string()]);
        let result = grep_repo_with_options(&repo, "needle", &opts).unwrap();

        if let GrepResult::Matches(matches) = result {
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0].path, "README.md");
        } else {
            panic!("Expected Matches result");
        }
    }

    #[test]
    fn test_read_file_with_ignore_blocks_explicit_path() {
        let temp = tempfile::tempdir().unwrap();
        let repo_dir = temp.path().join("repo");

        run_git(&["init", repo_dir.to_str().unwrap()], None);
        std::fs::write(repo_dir.join("allowed.txt"), "ok").unwrap();
        std::fs::write(repo_dir.join("blocked.txt"), "nope").unwrap();
        run_git(&["add", "."], Some(&repo_dir));
        run_git(
            &[
                "-c",
                "user.name=wit-test",
                "-c",
                "user.email=wit-test@example.com",
                "commit",
                "-m",
                "init",
            ],
            Some(&repo_dir),
        );

        let repo = gix::open(&repo_dir).unwrap();
        let allowed =
            read_file_with_ignore(&repo, "allowed.txt", &["blocked.txt".to_string()]).unwrap();
        assert_eq!(allowed, "ok");

        let err = read_file_with_ignore(&repo, "blocked.txt", &["blocked.txt".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("excluded by --ignore"));
    }

    #[test]
    fn test_list_dir_with_ignore_hides_ignored_entries() {
        let temp = tempfile::tempdir().unwrap();
        let repo_dir = temp.path().join("repo");

        run_git(&["init", repo_dir.to_str().unwrap()], None);
        std::fs::create_dir_all(repo_dir.join("src")).unwrap();
        std::fs::create_dir_all(repo_dir.join("vendor")).unwrap();
        std::fs::write(repo_dir.join("src").join("main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(repo_dir.join("vendor").join("lib.rs"), "pub fn x() {}\n").unwrap();
        run_git(&["add", "."], Some(&repo_dir));
        run_git(
            &[
                "-c",
                "user.name=wit-test",
                "-c",
                "user.email=wit-test@example.com",
                "commit",
                "-m",
                "init",
            ],
            Some(&repo_dir),
        );

        let repo = gix::open(&repo_dir).unwrap();
        let entries = list_dir_with_ignore(&repo, None, false, &["vendor".to_string()]).unwrap();
        let names: Vec<String> = entries.into_iter().map(|entry| entry.name).collect();
        assert!(names.contains(&"src".to_string()));
        assert!(!names.contains(&"vendor".to_string()));
    }

    // ==================== Integration Tests (require network) ====================
    // These are marked with #[ignore] and can be run with: cargo test -- --ignored

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_grep_repo_basic() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let result = grep_repo(&repo, "impl Widget").unwrap();

        if let GrepResult::Matches(matches) = result {
            assert!(!matches.is_empty(), "Should find 'impl Widget' in ratatui");
            assert!(matches.iter().any(|m| m.path.ends_with(".rs")));
        } else {
            panic!("Expected Matches result");
        }
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_grep_repo_ignore_case() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let opts = GrepOptions::new().ignore_case(true);
        let result = grep_repo_with_options(&repo, "README", &opts).unwrap();

        if let GrepResult::Matches(matches) = result {
            // Should match "README" in various cases
            assert!(!matches.is_empty());
        } else {
            panic!("Expected Matches result");
        }
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_grep_repo_max_count() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let opts = GrepOptions::new().max_count(5);
        let result = grep_repo_with_options(&repo, "fn", &opts).unwrap();

        if let GrepResult::Matches(matches) = result {
            let actual_matches: Vec<_> = matches.iter().filter(|m| !m.is_context).collect();
            assert!(actual_matches.len() <= 5, "Should respect max_count");
        } else {
            panic!("Expected Matches result");
        }
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_grep_repo_glob_filter() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let opts = GrepOptions::new().glob(Some("*.toml".to_string()));
        let result = grep_repo_with_options(&repo, "version", &opts).unwrap();

        if let GrepResult::Matches(matches) = result {
            assert!(
                matches.iter().all(|m| m.path.ends_with(".toml")),
                "All matches should be in .toml files"
            );
        } else {
            panic!("Expected Matches result");
        }
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_grep_repo_files_with_matches() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let opts = GrepOptions::new()
            .glob(Some("*.rs".to_string()))
            .files_with_matches(true);
        let result = grep_repo_with_options(&repo, "struct", &opts).unwrap();

        if let GrepResult::Files(files) = result {
            assert!(!files.is_empty());
            assert!(files.iter().all(|f| f.ends_with(".rs")));
        } else {
            panic!("Expected Files result");
        }
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_grep_repo_count() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let opts = GrepOptions::new()
            .glob(Some("Cargo.toml".to_string()))
            .count(true);
        let result = grep_repo_with_options(&repo, "version", &opts).unwrap();

        if let GrepResult::Counts(counts) = result {
            assert!(!counts.is_empty());
            for (path, count) in &counts {
                assert!(path.ends_with("Cargo.toml"));
                assert!(*count > 0);
            }
        } else {
            panic!("Expected Counts result");
        }
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_head_basic() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let result = head(&repo, "Cargo.toml", 5, false).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 5, "head should return exactly 5 lines");
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_head_with_numbers() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let result = head(&repo, "Cargo.toml", 3, true).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(
            lines[0].trim().starts_with("1"),
            "First line should start with 1"
        );
        assert!(
            lines[1].trim().starts_with("2"),
            "Second line should start with 2"
        );
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_tail_basic() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let result = tail(&repo, "Cargo.toml", 5, None, false).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 5, "tail should return exactly 5 lines");
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_tail_from_line() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        // Get total lines first
        let full = read_file(&repo, "Cargo.toml").unwrap();
        let total_lines = full.lines().count();

        // tail from line 5 should give us (total - 4) lines
        let result = tail(&repo, "Cargo.toml", 0, Some(5), false).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(
            lines.len(),
            total_lines - 4,
            "tail +5 should skip first 4 lines"
        );
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_tail_with_numbers() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let full = read_file(&repo, "Cargo.toml").unwrap();
        let total_lines = full.lines().count();

        let result = tail(&repo, "Cargo.toml", 3, None, true).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 3);

        // Last line should have line number equal to total_lines
        let last_line_num: usize = lines[2].split_whitespace().next().unwrap().parse().unwrap();
        assert_eq!(last_line_num, total_lines);
    }

    // ==================== Cache Refresh Tests ====================

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_cache_github_repo_uses_existing() {
        // First cache the repo
        let repo1 = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let path1 = repo1.path().to_path_buf();

        // Second call with refresh=false should reuse the cache
        let repo2 = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let path2 = repo2.path().to_path_buf();

        // Both should point to the same path (cache was reused)
        assert_eq!(path1, path2, "Cache should be reused when refresh=false");
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_cache_github_repo_refresh_replaces() {
        // First cache the repo
        let repo1 = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let path1 = repo1.path().to_path_buf();

        // Verify cache path exists
        assert!(
            path1.exists(),
            "Cache path should exist after initial clone"
        );

        // Second call with refresh=true should delete and re-clone
        let repo2 = cache_github_repo("ratatui/ratatui", CacheAcquisitionMode::ForceInvalidate)
            .await
            .unwrap();
        let path2 = repo2.path().to_path_buf();

        // Paths should be the same location, but cache was refreshed
        assert_eq!(path1, path2, "Cache path should be the same location");
        // Verify the repo is valid (refresh succeeded)
        assert!(path2.exists(), "Refreshed cache should exist");

        // Try to read a file to confirm the repo is functional
        let result = read_file(&repo2, "Cargo.toml");
        assert!(
            result.is_ok(),
            "Should be able to read file from refreshed cache"
        );
    }

    // ==================== list_dir Tests ====================

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_list_dir_root() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let entries = list_dir(&repo, None, false).unwrap();
        assert!(!entries.is_empty(), "Root should have entries");

        // Should have both dirs and files
        let has_dirs = entries.iter().any(|e| e.is_dir);
        let has_files = entries.iter().any(|e| !e.is_dir);
        assert!(has_dirs, "Root should contain directories");
        assert!(has_files, "Root should contain files");

        // Directories should come before files
        let first_file_idx = entries.iter().position(|e| !e.is_dir);
        let last_dir_idx = entries.iter().rposition(|e| e.is_dir);
        if let (Some(first_file), Some(last_dir)) = (first_file_idx, last_dir_idx) {
            assert!(
                last_dir < first_file,
                "Directories should be listed before files"
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_list_dir_subdir() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let entries = list_dir(&repo, Some("ratatui/src"), false).unwrap();
        assert!(!entries.is_empty(), "ratatui/src/ should have entries");

        // All entries should be immediate children (no slashes in names)
        for entry in &entries {
            assert!(
                !entry.name.contains('/'),
                "Entry '{}' should not contain slashes",
                entry.name
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_list_dir_long() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let entries = list_dir(&repo, Some("ratatui/src"), true).unwrap();
        assert!(!entries.is_empty());

        // Files should have size and line info populated
        for entry in entries.iter().filter(|e| !e.is_dir && !e.is_binary) {
            assert!(
                entry.size_bytes.is_some(),
                "File '{}' should have size_bytes in long mode",
                entry.name
            );
            assert!(
                entry.lines.is_some(),
                "File '{}' should have line count in long mode",
                entry.name
            );
        }

        // Directories should have None for size/lines
        for entry in entries.iter().filter(|e| e.is_dir) {
            assert!(
                entry.size_bytes.is_none(),
                "Dir '{}' should not have size_bytes",
                entry.name
            );
            assert!(
                entry.lines.is_none(),
                "Dir '{}' should not have line count",
                entry.name
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_list_dir_nonexistent() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let entries = list_dir(&repo, Some("nonexistent_dir_xyz"), false).unwrap();
        assert!(
            entries.is_empty(),
            "Nonexistent directory should return empty vec"
        );
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_list_dir_file_path() {
        let repo = cache_github_repo(
            "ratatui/ratatui",
            CacheAcquisitionMode::ServeStaleAndRevalidate,
        )
        .await
        .unwrap();
        let entries = list_dir(&repo, Some("Cargo.toml"), false).unwrap();
        // Cargo.toml is a file, not a directory, so nothing is "under" it
        assert!(
            entries.is_empty(),
            "File path should return empty vec (not a directory)"
        );
    }
}
