use anyhow::Context;
use fs2::FileExt;
use gix::{Repository, bstr::ByteSlice};
use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ptree::{TreeBuilder, print_tree};
use std::{
    collections::HashSet,
    fs::File,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

pub const WIT_CACHE_DIR_ENV: &str = "WIT_CACHE_DIR";
pub const WIT_CACHE_SUBDIR: &str = ".wit/cache";
static CACHE_PROCESS_LOCK: Mutex<()> = Mutex::new(());

struct CacheLock {
    _file_lock: File,
    _process_lock: std::sync::MutexGuard<'static, ()>,
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

pub async fn cache_github_repo(owner_repo: &str, refresh: bool) -> anyhow::Result<Repository> {
    let repo_url = format!("https://github.com/{owner_repo}", owner_repo = owner_repo);
    let cache_path = wit_cache_dir().join(owner_repo);
    let _cache_lock = acquire_cache_lock()?;

    if cache_path.exists() && !refresh {
        match gix::open(&cache_path) {
            Ok(repo) if cache_has_head_commit(&repo) => return Ok(repo),
            Ok(_) | Err(_) => {
                // A prior failed fetch can leave a cache directory with an unborn HEAD.
                // Treat it as stale and re-clone.
            }
        }
    }

    recache_repo(&repo_url, &cache_path)
}

fn acquire_cache_lock() -> anyhow::Result<CacheLock> {
    let process_lock = CACHE_PROCESS_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let lock_path = cache_lock_path();

    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("failed to create cache lock parent '{}'", parent.display())
        })?;
    }

    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("failed to open cache lock '{}'", lock_path.display()))?;
    lock_file
        .lock_exclusive()
        .with_context(|| format!("failed to lock cache '{}'", lock_path.display()))?;

    Ok(CacheLock {
        _file_lock: lock_file,
        _process_lock: process_lock,
    })
}

fn cache_lock_path() -> PathBuf {
    wit_cache_dir().join(".cache.lock")
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_acquire_cache_lock_blocks_parallel_calls() {
        let _temp = tempfile::tempdir().unwrap();
        let first_lock = acquire_cache_lock().unwrap();

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();

        let handle = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _second_lock = acquire_cache_lock().unwrap();
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
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
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
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
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
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
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
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
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
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
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
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
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
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
        let result = head(&repo, "Cargo.toml", 5, false).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 5, "head should return exactly 5 lines");
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_head_with_numbers() {
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
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
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
        let result = tail(&repo, "Cargo.toml", 5, None, false).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 5, "tail should return exactly 5 lines");
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_tail_from_line() {
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
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
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
        let full = read_file(&repo, "Cargo.toml").unwrap();
        let total_lines = full.lines().count();

        let result = tail(&repo, "Cargo.toml", 3, None, true).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 3);

        // Last line should have line number equal to total_lines
        let last_line_num: usize = lines[2]
            .trim()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(last_line_num, total_lines);
    }

    // ==================== Cache Refresh Tests ====================

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_cache_github_repo_uses_existing() {
        // First cache the repo
        let repo1 = cache_github_repo("ratatui/ratatui", false).await.unwrap();
        let path1 = repo1.path().to_path_buf();

        // Second call with refresh=false should reuse the cache
        let repo2 = cache_github_repo("ratatui/ratatui", false).await.unwrap();
        let path2 = repo2.path().to_path_buf();

        // Both should point to the same path (cache was reused)
        assert_eq!(path1, path2, "Cache should be reused when refresh=false");
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_cache_github_repo_refresh_replaces() {
        // First cache the repo
        let repo1 = cache_github_repo("ratatui/ratatui", false).await.unwrap();
        let path1 = repo1.path().to_path_buf();

        // Verify cache path exists
        assert!(
            path1.exists(),
            "Cache path should exist after initial clone"
        );

        // Second call with refresh=true should delete and re-clone
        let repo2 = cache_github_repo("ratatui/ratatui", true).await.unwrap();
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
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
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
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
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
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
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
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
        let entries = list_dir(&repo, Some("nonexistent_dir_xyz"), false).unwrap();
        assert!(
            entries.is_empty(),
            "Nonexistent directory should return empty vec"
        );
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn test_list_dir_file_path() {
        let repo = cache_github_repo("ratatui/ratatui", false).await.unwrap();
        let entries = list_dir(&repo, Some("Cargo.toml"), false).unwrap();
        // Cargo.toml is a file, not a directory, so nothing is "under" it
        assert!(
            entries.is_empty(),
            "File path should return empty vec (not a directory)"
        );
    }
}
