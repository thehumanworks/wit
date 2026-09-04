//! Memory-backend helpers for CLI rg/sed/head/tail and branch listing.
//!
//! These operate on an in-RAM [`MemorySnapshot`] and never write `WIT_CACHE_DIR`.

use crate::gitops::ops::{
    BlobWalkOptions, BranchCreatedSource, BranchMetadata, GrepMatch, GrepOptions, GrepResult,
    IgnoreMatcher, matches_glob, path_under_prefix, search_blob_bytes,
};
use wit_snapshot::{
    EntryKind, GitHubHttpClient, MemorySnapshot, ReqwestGitHubClient, SnapshotResult,
    split_owner_repo,
};

/// Ripgrep-style search over an in-memory snapshot (async blob fetches).
pub async fn grep_memory_snapshot<C: GitHubHttpClient + 'static>(
    snap: &MemorySnapshot<C>,
    pattern: &str,
    opts: &GrepOptions,
) -> anyhow::Result<GrepResult> {
    let ignore_matcher = IgnoreMatcher::new(&opts.ignore)?;
    let max_blob = snap.limits().max_blob_bytes;

    let mut file_matches: Vec<String> = Vec::new();
    let mut file_counts: Vec<(String, usize)> = Vec::new();
    let mut all_matches: Vec<GrepMatch> = Vec::new();
    let mut total_match_count = 0usize;

    for entry in snap.walk_entries() {
        if entry.kind != EntryKind::File {
            continue;
        }
        let path = entry.path;
        if ignore_matcher.is_ignored(&path) {
            continue;
        }
        if opts
            .glob
            .as_ref()
            .is_some_and(|glob| !matches_glob(&path, glob))
        {
            continue;
        }

        let remaining = opts
            .max_count
            .map_or(usize::MAX, |max| max.saturating_sub(total_match_count));
        if remaining == 0 {
            break;
        }

        let text = match snap.blob_text_by_sha(&entry.sha, max_blob).await {
            Ok(text) => text,
            Err(_) => continue, // skip binary / oversized / missing
        };

        let file_match_list = search_blob_bytes(&path, text.as_bytes(), pattern, opts, remaining)?;
        let match_count = file_match_list.iter().filter(|m| !m.is_context).count();
        if match_count == 0 {
            continue;
        }
        if opts.files_with_matches {
            file_matches.push(path);
        } else if opts.count {
            file_counts.push((path, match_count));
        } else {
            all_matches.extend(file_match_list);
        }
        total_match_count += match_count;
    }

    if opts.files_with_matches {
        Ok(GrepResult::Files(file_matches))
    } else if opts.count {
        Ok(GrepResult::Counts(file_counts))
    } else {
        Ok(GrepResult::Matches(all_matches))
    }
}

pub async fn read_memory_text<C: GitHubHttpClient + 'static>(
    snap: &MemorySnapshot<C>,
    path: &str,
    ignore_patterns: &[String],
) -> anyhow::Result<String> {
    let ignore_matcher = IgnoreMatcher::new(ignore_patterns)?;
    let normalized = wit_snapshot::normalize_repo_path(path);
    if ignore_matcher.is_ignored(&normalized) {
        anyhow::bail!("File '{path}' is excluded by --ignore");
    }
    let entry = snap
        .entry(&normalized)
        .ok_or_else(|| anyhow::anyhow!("File not found: {path}"))?;
    if entry.kind != EntryKind::File {
        anyhow::bail!("Not a file: {path}");
    }
    let text = snap
        .blob_text_by_sha(&entry.sha, snap.limits().max_blob_bytes)
        .await
        .map_err(|err| anyhow::anyhow!(err))?;
    Ok(text)
}

/// Memory-backend twin of [`crate::gitops::ops::walk_text_blobs`]: visit every
/// text blob under the filters in path order; `visit` returns `false` to stop.
pub async fn walk_memory_text_blobs<C: GitHubHttpClient + 'static>(
    snap: &MemorySnapshot<C>,
    opts: &BlobWalkOptions,
    mut visit: impl FnMut(&str, &str) -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    let ignore_matcher = IgnoreMatcher::new(&opts.ignore)?;
    let max_blob = snap.limits().max_blob_bytes;
    for entry in snap.walk_entries() {
        if entry.kind != EntryKind::File
            || !path_under_prefix(&entry.path, opts.path_prefix.as_deref().unwrap_or(""))
            || ignore_matcher.is_ignored(&entry.path)
            || opts
                .glob
                .as_ref()
                .is_some_and(|glob| !matches_glob(&entry.path, glob))
        {
            continue;
        }
        if opts.max_bytes > 0
            && entry
                .size
                .is_some_and(|size| size as usize > opts.max_bytes)
        {
            continue;
        }
        let Ok(text) = snap.blob_text_by_sha(&entry.sha, max_blob).await else {
            continue; // binary / oversized / missing
        };
        if !visit(&entry.path, &text)? {
            break;
        }
    }
    Ok(())
}

pub fn head_from_text(content: &str, count: usize, number: bool) -> String {
    let selected: Vec<&str> = content.lines().take(count).collect();
    if number {
        selected
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}  {}", i + 1, line))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        selected.join("\n")
    }
}

pub fn tail_from_text(
    content: &str,
    count: usize,
    from_line: Option<usize>,
    number: bool,
) -> String {
    let all_lines: Vec<&str> = content.lines().collect();
    let total = all_lines.len();
    let (selected, start_line_num): (Vec<&str>, usize) = if let Some(start) = from_line {
        let skip = start.saturating_sub(1);
        (all_lines.into_iter().skip(skip).collect(), start)
    } else {
        let skip = total.saturating_sub(count);
        (all_lines.into_iter().skip(skip).collect(), skip + 1)
    };
    if number {
        selected
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}  {}", start_line_num + i, line))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        selected.join("\n")
    }
}

/// Filter snapshot list/tree paths by `--ignore` (same semantics as disk).
pub fn filter_ignored_paths<I, S>(paths: I, ignore_patterns: &[String]) -> anyhow::Result<Vec<S>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let ignore_matcher = IgnoreMatcher::new(ignore_patterns)?;
    Ok(paths
        .into_iter()
        .filter(|path| !ignore_matcher.is_ignored(path.as_ref()))
        .collect())
}

/// List remote branches via the GitHub REST API (no git clone / WIT_CACHE_DIR).
pub async fn list_remote_branches_api(owner_repo: &str) -> anyhow::Result<Vec<BranchMetadata>> {
    let (owner, name) = split_owner_repo(owner_repo).map_err(|err| anyhow::anyhow!(err))?;
    let owner_repo = format!("{owner}/{name}");
    let client = ReqwestGitHubClient::from_env().map_err(|err| anyhow::anyhow!(err))?;

    let (status, repo_body) = client
        .get_json(&format!("/repos/{owner_repo}"))
        .await
        .map_err(|err| anyhow::anyhow!(err))?;
    if status == 404 {
        anyhow::bail!("repository '{owner_repo}' was not found");
    }
    if status == 403 {
        anyhow::bail!("GitHub API rejected listing branches for '{owner_repo}' (HTTP 403)");
    }
    if !(200..300).contains(&status) {
        anyhow::bail!("GitHub API /repos/{owner_repo} returned HTTP {status}");
    }
    let repo_json: serde_json::Value = serde_json::from_str(&repo_body)?;
    let default_branch = repo_json
        .get("default_branch")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("repository response missing default_branch"))?
        .to_string();

    let mut branches_json = Vec::new();
    let mut page = 1u32;
    loop {
        let path = format!("/repos/{owner_repo}/branches?per_page=100&page={page}");
        let (status, body) = client
            .get_json(&path)
            .await
            .map_err(|err| anyhow::anyhow!(err))?;
        if !(200..300).contains(&status) {
            anyhow::bail!("GitHub API list branches returned HTTP {status}");
        }
        let page_items: Vec<serde_json::Value> = serde_json::from_str(&body)?;
        let count = page_items.len();
        branches_json.extend(page_items);
        if count < 100 {
            break;
        }
        page += 1;
        if page > 20 {
            break;
        }
    }
    if branches_json.is_empty() {
        anyhow::bail!("remote did not list any branches");
    }

    let mut branches = Vec::with_capacity(branches_json.len());
    for item in branches_json {
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("branch entry missing name"))?
            .to_string();
        let commit = item
            .get("commit")
            .ok_or_else(|| anyhow::anyhow!("branch entry missing commit"))?;
        let tip_sha = commit
            .get("sha")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tip_commit = commit
            .get("commit")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let tip_author = tip_commit
            .pointer("/author/name")
            .and_then(|v| v.as_str())
            .or_else(|| {
                tip_commit
                    .pointer("/committer/name")
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("")
            .to_string();
        let tip_time = tip_commit
            .pointer("/author/date")
            .and_then(|v| v.as_str())
            .or_else(|| {
                tip_commit
                    .pointer("/committer/date")
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("")
            .to_string();

        let is_default = name == default_branch;
        let (ahead, behind, merged) = if is_default {
            (0usize, 0usize, true)
        } else {
            compare_ahead_behind(&client, &owner_repo, &default_branch, &name)
                .await
                .unwrap_or_default()
        };

        branches.push(BranchMetadata {
            is_default,
            name,
            tip_sha,
            tip_author,
            tip_time: tip_time.clone(),
            ahead,
            behind,
            merged,
            created_time: tip_time,
            created_source: BranchCreatedSource::TipCommitFallback,
        });
    }

    branches.sort_by(|left, right| {
        right
            .is_default
            .cmp(&left.is_default)
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(branches)
}

async fn compare_ahead_behind(
    client: &ReqwestGitHubClient,
    owner_repo: &str,
    base: &str,
    head: &str,
) -> SnapshotResult<(usize, usize, bool)> {
    let path = format!("/repos/{owner_repo}/compare/{base}...{head}");
    let (status, body) = client.get_json(&path).await?;
    if !(200..300).contains(&status) {
        return Ok((0, 0, false));
    }
    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|err| wit_snapshot::SnapshotError::Other(err.to_string()))?;
    let ahead = json.get("ahead_by").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let behind = json.get("behind_by").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    Ok((ahead, behind, ahead == 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitops::ops::GrepOptions;
    use std::sync::Arc;
    use wit_snapshot::{MemoryBackendLimits, ReqwestGitHubClient, snapshot_from_tree_json};

    fn fixture_snapshot() -> MemorySnapshot<ReqwestGitHubClient> {
        let tree = serde_json::json!({
            "sha": "treesha",
            "truncated": false,
            "tree": [
                {"path": "README.md", "mode": "100644", "type": "blob", "sha": "blob-readme", "size": 24},
                {"path": "src/lib.rs", "mode": "100644", "type": "blob", "sha": "blob-lib", "size": 28},
                {"path": "ignored/tmp.log", "mode": "100644", "type": "blob", "sha": "blob-log", "size": 8}
            ]
        })
        .to_string();
        let client = ReqwestGitHubClient::new("http://127.0.0.1:9", None).unwrap();
        let snap = snapshot_from_tree_json(
            Arc::new(client),
            "owner/repo",
            "main",
            "abc123",
            "treesha",
            &tree,
            MemoryBackendLimits::default(),
        )
        .unwrap();
        snap.preload_blob(
            "blob-readme",
            b"Hello World\nSecond line\nThird line\n".to_vec(),
        )
        .unwrap();
        snap.preload_blob("blob-lib", b"pub fn answer() -> u8 {\n    42\n}\n".to_vec())
            .unwrap();
        snap.preload_blob("blob-log", b"noise\n".to_vec()).unwrap();
        snap
    }

    #[test]
    fn head_and_tail_from_text_number_lines() {
        let text = "a\nb\nc\nd\n";
        assert_eq!(head_from_text(text, 2, false), "a\nb");
        assert_eq!(head_from_text(text, 2, true), "     1  a\n     2  b");
        assert_eq!(tail_from_text(text, 2, None, false), "c\nd");
        assert_eq!(tail_from_text(text, 2, None, true), "     3  c\n     4  d");
        assert_eq!(tail_from_text(text, 10, Some(2), false), "b\nc\nd");
    }

    #[tokio::test]
    async fn grep_memory_snapshot_finds_matches_and_respects_ignore() {
        let snap = fixture_snapshot();
        let opts = GrepOptions::new()
            .ignore_case(true)
            .ignore(vec!["ignored/**".to_string()]);
        let result = grep_memory_snapshot(&snap, "hello", &opts).await.unwrap();
        match result {
            GrepResult::Matches(matches) => {
                assert!(!matches.is_empty());
                assert!(matches.iter().all(|m| m.path == "README.md"));
                assert!(matches.iter().any(|m| m.content.contains("Hello")));
            }
            other => panic!("expected matches, got {other:?}"),
        }

        let files = grep_memory_snapshot(
            &snap,
            "answer",
            &GrepOptions::new().files_with_matches(true),
        )
        .await
        .unwrap();
        match files {
            GrepResult::Files(paths) => assert_eq!(paths, vec!["src/lib.rs".to_string()]),
            other => panic!("expected files, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_memory_text_honors_ignore() {
        let snap = fixture_snapshot();
        let err = read_memory_text(&snap, "ignored/tmp.log", &["ignored/**".to_string()])
            .await
            .expect_err("ignored path must fail");
        assert!(err.to_string().contains("excluded by --ignore"));
        let text = read_memory_text(&snap, "README.md", &[]).await.unwrap();
        assert!(text.starts_with("Hello World"));
    }
}
