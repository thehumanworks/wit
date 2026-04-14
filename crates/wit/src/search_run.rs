//! Orchestrates `wit search` through GitHub repository search only.
use wits::{RepoListMetric, types::RepoMatch};

use crate::search::{GitHubSearchClient, RepositorySummary};

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
    pattern: Option<&str>,
    lang: Option<&str>,
    query: Option<&str>,
    limit: usize,
) -> anyhow::Result<(Vec<RepoMatch>, RepoListMetric, bool)> {
    let client = GitHubSearchClient::new();
    let results = client
        .search_repositories(pattern, lang, query, limit)
        .await
        .map_err(github_search_hint)?;
    let incomplete = results.incomplete_results;
    let repos = repo_matches_from_github_summaries(&results.repositories);
    Ok((repos, RepoListMetric::Stars, incomplete))
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
