use anyhow::{Context, ensure};
use octocrab::{Octocrab, models::Repository};

use crate::{ensure_rustls_provider, operation_context::OperationContext};

const DEFAULT_SORT: &str = "stars";
const DEFAULT_ORDER: &str = "desc";
const DEFAULT_PER_PAGE: u8 = 100;

/// GitHub repository search returns at most 1000 items; cap what we keep after paging.
pub const MAX_GITHUB_REPOS: usize = 1000;
pub const DEFAULT_GITHUB_REPO_LIMIT: usize = 10;

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

    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<RepositorySearchResults> {
        self.search_with_context(&OperationContext::default(), query, limit)
            .await
    }

    pub async fn search_with_context(
        &self,
        context: &OperationContext,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<RepositorySearchResults> {
        let query = query.trim();
        ensure!(!query.is_empty(), "github search query cannot be empty");
        ensure!(limit > 0, "github search limit must be greater than zero");

        let limit = limit.min(MAX_GITHUB_REPOS);
        let per_page = limit.min(usize::from(DEFAULT_PER_PAGE)) as u8;

        let request = self
            .octocrab
            .search()
            .repositories(query)
            .sort(DEFAULT_SORT)
            .order(DEFAULT_ORDER)
            .per_page(per_page)
            .send();
        let mut page = context
            .wait(request)
            .await
            .map_err(anyhow::Error::msg)?
            .with_context(|| format!("failed to search GitHub repositories for `{query}`"))?;

        let total_count = page.total_count.unwrap_or(page.items.len() as u64);
        let incomplete_results = page.incomplete_results.unwrap_or(false);
        let mut repositories: Vec<RepositorySummary> = page
            .take_items()
            .into_iter()
            .map(RepositorySummary::from)
            .collect();
        let mut next_page = page.next.clone();

        while repositories.len() < limit {
            let Some(mut page) = context
                .wait(self.octocrab.get_page::<Repository>(&next_page))
                .await
                .map_err(anyhow::Error::msg)?
                .context("failed to fetch additional GitHub repository search pages")?
            else {
                break;
            };

            next_page = page.next.clone();
            repositories.extend(page.take_items().into_iter().map(RepositorySummary::from));
        }

        if repositories.len() > limit {
            repositories.truncate(limit);
        }

        Ok(RepositorySearchResults {
            total_count,
            incomplete_results,
            repositories,
        })
    }

    pub async fn search_repositories(
        &self,
        pattern: Option<&str>,
        language: Option<&str>,
        query: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<RepositorySearchResults> {
        let query = build_repository_query(pattern, language, query)?;
        self.search(&query, limit).await
    }

    pub async fn search_repositories_with_context(
        &self,
        context: &OperationContext,
        pattern: Option<&str>,
        language: Option<&str>,
        query: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<RepositorySearchResults> {
        let query = build_repository_query(pattern, language, query)?;
        self.search_with_context(context, &query, limit).await
    }
}

pub async fn search(
    query: impl AsRef<str>,
    limit: usize,
) -> anyhow::Result<RepositorySearchResults> {
    GitHubSearchClient::new()
        .search(query.as_ref(), limit)
        .await
}

pub fn build_repository_query(
    pattern: Option<&str>,
    language: Option<&str>,
    query: Option<&str>,
) -> anyhow::Result<String> {
    let mut parts = Vec::new();

    if let Some(pattern) = pattern.map(str::trim).filter(|value| !value.is_empty()) {
        parts.push(format!("{} in:name", normalize_search_term(pattern)));
    }

    if let Some(language) = language.map(str::trim).filter(|value| !value.is_empty()) {
        parts.push(format!("language:{}", normalize_search_term(language)));
    }

    if let Some(query) = query.map(str::trim).filter(|value| !value.is_empty()) {
        parts.push(query.to_string());
    }

    ensure!(
        !parts.is_empty(),
        "repository search requires at least one search filter (--pattern, --lang, or --query)"
    );

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
    use crate::{
        ensure_rustls_provider,
        operation_context::{OPERATION_CANCELLED, OperationCancellation},
    };
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
    fn build_repository_query_combines_name_language_and_raw_query() {
        assert_eq!(
            build_repository_query(
                Some("my repo"),
                Some("Jupyter Notebook"),
                Some("stars:>1000 archived:false")
            )
            .unwrap(),
            "\"my repo\" in:name language:\"Jupyter Notebook\" stars:>1000 archived:false"
        );
    }

    #[test]
    fn build_repository_query_allows_raw_query_without_pattern() {
        assert_eq!(
            build_repository_query(None, None, Some("stars:>1000 topic:tui")).unwrap(),
            "stars:>1000 topic:tui"
        );
    }

    #[test]
    fn build_repository_query_requires_at_least_one_filter() {
        let err = build_repository_query(None, None, None).unwrap_err();
        assert!(
            err.to_string().contains("at least one"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn build_repository_query_trims_empty_inputs() {
        assert_eq!(
            build_repository_query(Some("  "), Some(" Rust "), Some("  archived:false  ")).unwrap(),
            "language:Rust archived:false"
        );
    }

    #[test]
    fn search_limit_constants_match_cli_contract() {
        assert_eq!(DEFAULT_GITHUB_REPO_LIMIT, 10);
        assert_eq!(MAX_GITHUB_REPOS, 1000);
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
            .search_repositories(Some("x"), None, None, 30)
            .await
            .expect_err("401 should fail");

        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("failed to search") && msg.contains("github"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn search_repositories_stops_waiting_when_cancelled() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/repositories"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_secs(30))
                    .set_body_json(json!({
                        "total_count": 0,
                        "incomplete_results": false,
                        "items": []
                    })),
            )
            .mount(&mock_server)
            .await;

        let cancellation = OperationCancellation::default();
        let context = OperationContext::new(None, cancellation.clone());
        let client = search_client(&mock_server.uri());
        let task = tokio::spawn(async move {
            client
                .search_repositories_with_context(&context, Some("slow"), None, None, 10)
                .await
        });
        tokio::task::yield_now().await;
        cancellation.cancel();
        let error = tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .expect("HTTP cancellation should be bounded")
            .unwrap()
            .unwrap_err();
        assert_eq!(error.to_string(), OPERATION_CANCELLED);
    }

    #[tokio::test]
    async fn search_repositories_uses_limit_as_page_size_and_stops_after_first_page() {
        let mock_server = MockServer::start().await;
        let query = "ratatui in:name language:Rust";
        let next_page_url = format!(
            "{}/search/repositories?q=ratatui+in:name+language:Rust&sort=stars&order=desc&per_page=2&page=2",
            mock_server.uri()
        );

        let first_page = json!({
            "total_count": 3,
            "incomplete_results": false,
            "items": [
                repo_json(1, "ratatui", "ratatui/ratatui", "Terminal UI library", "Rust", 10_000),
                repo_json(2, "ratatui-website", "ratatui/website", "Docs site", "Rust", 2_000)
            ],
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
            .and(query_param("per_page", "2"))
            .and(query_param_is_missing("page"))
            .respond_with(first_response)
            .mount(&mock_server)
            .await;

        let results = search_client(&mock_server.uri())
            .search_repositories(Some("ratatui"), Some("Rust"), None, 2)
            .await
            .expect("search should succeed");

        assert_eq!(
            results,
            RepositorySearchResults {
                total_count: 3,
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
            .search_repositories(Some("ratatui"), Some("Rust"), None, 200)
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
