use anyhow::{Context, bail, ensure};
use octocrab::{Octocrab, models::Repository};

use crate::ensure_rustls_provider;

const DEFAULT_SORT: &str = "stars";
const DEFAULT_ORDER: &str = "desc";
const DEFAULT_PER_PAGE: u8 = 100;

/// GitHub repository search returns at most 1000 items; cap what we keep after paging.
pub const MAX_GITHUB_REPOS: usize = 1000;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RepositorySearchResults {
    pub total_count: u64,
    pub incomplete_results: bool,
    pub repositories: Vec<RepositorySummary>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RepositorySummary {
    pub name: String,
    pub full_name: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub stars: u32,
    pub html_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GitHubSearchClient {
    octocrab: Octocrab,
}

impl Default for GitHubSearchClient {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHubSearchClient {
    /// Build a client using `GITHUB_TOKEN` when set (higher rate limits); otherwise unauthenticated.
    pub fn new() -> Self {
        ensure_rustls_provider();
        let mut builder = Octocrab::builder();
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            let token = token.trim();
            if !token.is_empty() {
                builder = builder.personal_token(token.to_string());
            }
        }
        let octo = builder
            .build()
            .context("failed to build GitHub client")
            .unwrap();
        Self::with_octocrab(octo)
    }

    pub fn with_octocrab(octocrab: Octocrab) -> Self {
        Self { octocrab }
    }

    pub async fn search(&self, query: &str) -> anyhow::Result<RepositorySearchResults> {
        let query = query.trim();
        ensure!(!query.is_empty(), "github search query cannot be empty");

        let page = self
            .octocrab
            .search()
            .repositories(query)
            .sort(DEFAULT_SORT)
            .order(DEFAULT_ORDER)
            .per_page(DEFAULT_PER_PAGE)
            .send()
            .await
            .with_context(|| format!("failed to search GitHub repositories for `{query}`"))?;

        let total_count = page.total_count.unwrap_or(page.items.len() as u64);
        let incomplete_results = page.incomplete_results.unwrap_or(false);
        let repositories = self
            .octocrab
            .all_pages(page)
            .await
            .context("failed to fetch additional GitHub repository search pages")?;

        let mut repositories: Vec<RepositorySummary> = repositories
            .into_iter()
            .map(RepositorySummary::from)
            .collect();
        if repositories.len() > MAX_GITHUB_REPOS {
            repositories.truncate(MAX_GITHUB_REPOS);
        }

        Ok(RepositorySearchResults {
            total_count,
            incomplete_results,
            repositories,
        })
    }

    pub async fn search_repositories(
        &self,
        pattern: &str,
        language: Option<&str>,
        regex: bool,
    ) -> anyhow::Result<RepositorySearchResults> {
        let pattern = pattern.trim();
        ensure!(
            !pattern.is_empty(),
            "repository search pattern cannot be empty"
        );

        let query = build_github_repository_query(pattern, language, regex)?;
        self.search(&query).await
    }
}

pub async fn search(query: impl AsRef<str>) -> anyhow::Result<RepositorySearchResults> {
    GitHubSearchClient::new().search(query.as_ref()).await
}

pub fn build_repository_query(pattern: &str, language: Option<&str>) -> String {
    let mut query = vec![format!("{} in:name", normalize_search_term(pattern))];

    if let Some(language) = language.map(str::trim).filter(|value| !value.is_empty()) {
        query.push(format!("language:{}", normalize_search_term(language)));
    }

    query.join(" ")
}

/// Build a GitHub `q` string for repository search. With `regex: true`, only simple name tokens are allowed.
pub fn build_github_repository_query(
    pattern: &str,
    language: Option<&str>,
    regex: bool,
) -> anyhow::Result<String> {
    let pattern = pattern.trim();
    ensure!(
        !pattern.is_empty(),
        "repository search pattern cannot be empty"
    );

    if !regex {
        return Ok(build_repository_query(pattern, language));
    }

    if pattern.chars().any(char::is_whitespace) {
        bail!(
            "GitHub repository name search does not support whitespace in -p with --regex true; pass --regex false for a literal name, or use -q/-w so search uses grep.app."
        );
    }

    if !pattern
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        bail!(
            "GitHub repository name search with --regex true only supports simple tokens (letters, digits, ., _, -). Use --regex false for a literal phrase, or use -q/-w for grep.app search."
        );
    }

    let mut parts = vec![format!("{pattern} in:name")];
    if let Some(language) = language.map(str::trim).filter(|value| !value.is_empty()) {
        parts.push(format!("language:{}", normalize_search_term(language)));
    }

    Ok(parts.join(" "))
}

fn normalize_search_term(value: &str) -> String {
    let value = value.trim();
    if value.chars().any(char::is_whitespace) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn normalize_language(language: Option<serde_json::Value>) -> Option<String> {
    language.and_then(|value| match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(language) => Some(language),
        other => Some(other.to_string()),
    })
}

impl From<Repository> for RepositorySummary {
    fn from(repository: Repository) -> Self {
        Self {
            name: repository.name.clone(),
            full_name: repository.full_name.unwrap_or(repository.name),
            description: repository.description,
            language: normalize_language(repository.language),
            stars: repository.stargazers_count.unwrap_or_default(),
            html_url: repository.html_url.map(|url| url.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ensure_rustls_provider;
    use octocrab::Octocrab;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param, query_param_is_missing},
    };

    fn repo_json(
        id: u64,
        name: &str,
        full_name: &str,
        description: &str,
        language: &str,
        stars: u32,
    ) -> serde_json::Value {
        json!({
            "id": id,
            "name": name,
            "full_name": full_name,
            "html_url": format!("https://github.com/{full_name}"),
            "description": description,
            "url": format!("https://api.github.com/repos/{full_name}"),
            "language": language,
            "stargazers_count": stars,
        })
    }

    fn search_client(uri: &str) -> GitHubSearchClient {
        ensure_rustls_provider();
        let octocrab = Octocrab::builder().base_uri(uri).unwrap().build().unwrap();
        GitHubSearchClient::with_octocrab(octocrab)
    }

    #[test]
    fn build_repository_query_scopes_name_and_language() {
        assert_eq!(
            build_repository_query("my repo", Some("Jupyter Notebook")),
            "\"my repo\" in:name language:\"Jupyter Notebook\""
        );
    }

    #[test]
    fn build_github_repository_query_literal_matches_build_repository_query() {
        assert_eq!(
            build_github_repository_query("my repo", Some("Jupyter Notebook"), false).unwrap(),
            build_repository_query("my repo", Some("Jupyter Notebook"))
        );
    }

    #[test]
    fn build_github_repository_query_regex_rejects_operators() {
        let err = build_github_repository_query("foo+bar", None, true).unwrap_err();
        assert!(
            err.to_string().contains("simple tokens"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn build_github_repository_query_regex_rejects_whitespace() {
        let err = build_github_repository_query("my repo", None, true).unwrap_err();
        assert!(err.to_string().contains("whitespace"));
    }

    #[test]
    fn build_github_repository_query_regex_simple_token() {
        assert_eq!(
            build_github_repository_query("ratatui", Some("Rust"), true).unwrap(),
            "ratatui in:name language:Rust"
        );
    }

    #[tokio::test]
    async fn search_repositories_surfaces_github_http_errors() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/repositories"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "message": "Bad credentials",
                "documentation_url": "https://docs.github.com/rest"
            })))
            .mount(&mock_server)
            .await;

        let err = search_client(&mock_server.uri())
            .search_repositories("x", None, true)
            .await
            .expect_err("401 should fail");

        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("failed to search") && msg.contains("github"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn search_repositories_collects_all_pages_and_maps_results() {
        let mock_server = MockServer::start().await;
        let query = "ratatui in:name language:Rust";
        let next_page_url = format!(
            "{}/search/repositories?q=ratatui+in:name+language:Rust&sort=stars&order=desc&per_page=100&page=2",
            mock_server.uri()
        );

        let first_page = json!({
            "total_count": 2,
            "incomplete_results": false,
            "items": [repo_json(1, "ratatui", "ratatui/ratatui", "Terminal UI library", "Rust", 10_000)],
        });
        let first_response = ResponseTemplate::new(200)
            .append_header(
                "Link",
                format!("<{next_page_url}>; rel=\"next\", <{next_page_url}>; rel=\"last\""),
            )
            .set_body_json(first_page);

        Mock::given(method("GET"))
            .and(path("/search/repositories"))
            .and(query_param("q", query))
            .and(query_param("sort", "stars"))
            .and(query_param("order", "desc"))
            .and(query_param("per_page", "100"))
            .and(query_param_is_missing("page"))
            .respond_with(first_response)
            .mount(&mock_server)
            .await;

        let second_page = json!({
            "total_count": 2,
            "incomplete_results": false,
            "items": [repo_json(2, "ratatui-website", "ratatui/website", "Docs site", "Rust", 2_000)],
        });
        Mock::given(method("GET"))
            .and(path("/search/repositories"))
            .and(query_param("q", query))
            .and(query_param("sort", "stars"))
            .and(query_param("order", "desc"))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(second_page))
            .mount(&mock_server)
            .await;

        let results = search_client(&mock_server.uri())
            .search_repositories("ratatui", Some("Rust"), true)
            .await
            .expect("search should succeed");

        assert_eq!(
            results,
            RepositorySearchResults {
                total_count: 2,
                incomplete_results: false,
                repositories: vec![
                    RepositorySummary {
                        name: "ratatui".to_string(),
                        full_name: "ratatui/ratatui".to_string(),
                        description: Some("Terminal UI library".to_string()),
                        language: Some("Rust".to_string()),
                        stars: 10_000,
                        html_url: Some("https://github.com/ratatui/ratatui".to_string()),
                    },
                    RepositorySummary {
                        name: "ratatui-website".to_string(),
                        full_name: "ratatui/website".to_string(),
                        description: Some("Docs site".to_string()),
                        language: Some("Rust".to_string()),
                        stars: 2_000,
                        html_url: Some("https://github.com/ratatui/website".to_string()),
                    },
                ],
            }
        );
    }
}
