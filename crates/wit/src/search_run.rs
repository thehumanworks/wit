//! Orchestrates `wit search`: GitHub repository search vs grep.app.
//!
//! Hybrid routing: GitHub `search/repositories` when the code query is the default `.*` and
//! snippets are off; otherwise this module calls [`wits::client::GrepClient`] (grep.app).
//! Keep `GrepClient` usage here so `scripts/check_wit_search_migration.sh` can allowlist this file.

use anyhow::Context;
use wits::{RepoListMetric, types::RepoMatch};

use crate::search::{GitHubSearchClient, RepositorySummary};

/// `true` when repository discovery should use GitHub's REST API instead of grep.app facets.
///
/// Matrix: GitHub only for default code query `.*` and without `--with-snippets`.
pub fn use_github_repo_search(code_query: &str, with_snippets: bool) -> bool {
    let q = code_query.trim();
    q == ".*" && !with_snippets
}

pub fn repo_matches_from_github_summaries(summaries: &[RepositorySummary]) -> Vec<RepoMatch> {
    summaries
        .iter()
        .map(|r| RepoMatch {
            name: r.full_name.clone(),
            hits: u64::from(r.stars),
            files: vec![],
        })
        .collect()
}

pub async fn run_repository_search(
    pattern: &str,
    lang: Option<&str>,
    regex: bool,
    code_query: &str,
    with_snippets: bool,
) -> anyhow::Result<(Vec<RepoMatch>, RepoListMetric, bool)> {
    if use_github_repo_search(code_query, with_snippets) {
        let client = GitHubSearchClient::new();
        let results = client
            .search_repositories(pattern, lang, regex)
            .await
            .map_err(github_search_hint)?;
        let incomplete = results.incomplete_results;
        let repos = repo_matches_from_github_summaries(&results.repositories);
        Ok((repos, RepoListMetric::Stars, incomplete))
    } else {
        let client = wits::client::GrepClient::new();
        let repos = client
            .repo_search(pattern, lang, regex, code_query, with_snippets)
            .await
            .context("grep.app search failed")?;
        Ok((repos, RepoListMetric::CodeHits, false))
    }
}

fn github_search_hint(err: anyhow::Error) -> anyhow::Error {
    let s = err.to_string().to_lowercase();
    if s.contains("401")
        || s.contains("unauthorized")
        || s.contains("403")
        || s.contains("forbidden")
    {
        return err.context(
            "GitHub API rejected the request. Set GITHUB_TOKEN for authenticated access and higher rate limits.",
        );
    }
    if s.contains("429") || s.contains("rate limit") {
        return err.context("GitHub API rate limit exceeded. Retry later or set GITHUB_TOKEN.");
    }
    err
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_github_only_for_default_query_without_snippets() {
        assert!(use_github_repo_search(".*", false));
        assert!(use_github_repo_search("  .*  ", false));
        assert!(!use_github_repo_search(".*", true));
        assert!(!use_github_repo_search("foo", false));
        assert!(!use_github_repo_search("foo", true));
    }

    #[test]
    fn github_repo_match_uses_full_name_and_stars_as_metric_value() {
        let summaries = vec![RepositorySummary {
            name: "ratatui".to_string(),
            full_name: "ratatui/ratatui".to_string(),
            description: None,
            language: None,
            stars: 42,
            html_url: None,
        }];
        let m = repo_matches_from_github_summaries(&summaries);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].name, "ratatui/ratatui");
        assert_eq!(m[0].hits, 42);
        assert!(m[0].files.is_empty());
    }
}
