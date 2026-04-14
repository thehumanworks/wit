//! Optional live check against GitHub (authenticated when `GITHUB_TOKEN` is set).
//! Run: `cargo test -p wit --test search_github_live -- --ignored`

use wit::{ensure_rustls_provider, search::GitHubSearchClient};

#[tokio::test]
#[ignore = "hits real GitHub API"]
async fn github_search_client_name_query_smoke() {
    ensure_rustls_provider();
    let client = GitHubSearchClient::new();
    let results = client
        .search_repositories(Some("ratatui"), Some("Rust"), None, 10)
        .await
        .expect("GitHub repository search should succeed");

    assert!(
        !results.repositories.is_empty(),
        "expected at least one repository for ratatui + Rust"
    );
    assert!(
        results.repositories[0].full_name.contains("ratatui"),
        "unexpected first hit: {:?}",
        results.repositories[0]
    );
}
