use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use wiremock::matchers::{method, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wits::client::{BASE_API_ENDPOINT, GrepClient};
use wits::types::GrepSearchResult;

fn cassette_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cassettes")
}

fn load_cassette(name: &str) -> String {
    let path = cassette_dir().join(format!("{name}.json"));
    fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "Failed to read cassette {}: {}. Run `cargo test -p wits --test integration -- --ignored record` to generate cassettes.",
            path.display(),
            e
        )
    })
}

fn is_security_checkpoint(status: reqwest::StatusCode, body: &str) -> bool {
    !status.is_success() && body.contains("Vercel Security Checkpoint")
}

async fn fetch_live_search_body(
    repo_pattern: &str,
    lang: Option<&str>,
    query: &str,
) -> Option<String> {
    let client = reqwest::Client::builder()
        .default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert(reqwest::header::ACCEPT, "application/json".parse().unwrap());
            h
        })
        .build()
        .unwrap();

    let mut url = reqwest::Url::parse(BASE_API_ENDPOINT).unwrap();
    let mut pairs: Vec<(&str, &str)> = vec![
        ("f.repo.pattern", repo_pattern),
        ("regexp", "true"),
        ("q", query),
    ];
    if let Some(lang) = lang {
        pairs.push(("f.lang.pattern", lang));
    }
    url.query_pairs_mut().extend_pairs(pairs);

    let response = client
        .get(url)
        .send()
        .await
        .expect("HTTP request to grep.app should succeed");

    let status = response.status();
    let body = response.text().await.expect("should read response body");

    if is_security_checkpoint(status, &body) {
        eprintln!("Skipping live grep.app test: service returned anti-bot checkpoint ({status})");
        return None;
    }

    assert!(status.is_success(), "grep.app returned {status}: {body}");

    Some(body)
}

/// Helper: mount a mock that responds to any GET with the given body.
/// The client constructs the full URL from `with_base_url`, so we match on
/// query params rather than a hardcoded path — this way tests don't silently
/// pass when the real API path changes.
async fn mount_search_mock(server: &MockServer, body: &str) {
    Mock::given(method("GET"))
        .and(query_param("q", ".*"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(server)
        .await;
}

// ── VCR replay tests (run against cassette fixtures) ───────────────────────

#[tokio::test]
async fn test_repo_search_returns_results() {
    let body = load_cassette("repo_search_basic");
    let mock_server = MockServer::start().await;

    mount_search_mock(&mock_server, &body).await;

    let client = GrepClient::with_base_url(&format!("{}/api/search2", mock_server.uri()));
    let repos = client
        .repo_search("ratatui", None, true, ".*", false)
        .await
        .expect("search should succeed");

    assert!(!repos.is_empty(), "should return at least one repo");

    // Verify structure: each repo has a name and positive hit count
    for repo in &repos {
        assert!(!repo.name.is_empty(), "repo name should not be empty");
        assert!(repo.hits > 0, "repo should have positive hit count");
    }

    // Results should be sorted by hits descending
    let hit_counts: Vec<u64> = repos.iter().map(|r| r.hits).collect();
    for window in hit_counts.windows(2) {
        assert!(
            window[0] >= window[1],
            "repos should be sorted by hits descending"
        );
    }
}

#[tokio::test]
async fn test_repo_search_with_language_filter() {
    let body = load_cassette("repo_search_with_lang");
    let mock_server = MockServer::start().await;

    mount_search_mock(&mock_server, &body).await;

    let client = GrepClient::with_base_url(&format!("{}/api/search2", mock_server.uri()));
    let repos = client
        .repo_search("ratatui", Some("Rust"), true, ".*", false)
        .await
        .expect("search with lang filter should succeed");

    assert!(!repos.is_empty(), "should return repos for Rust filter");
}

#[tokio::test]
async fn test_repo_search_with_snippets() {
    let body = load_cassette("repo_search_with_snippets");
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(query_param("q", "impl Widget"))
        .respond_with(ResponseTemplate::new(200).set_body_string(&body))
        .mount(&mock_server)
        .await;

    let client = GrepClient::with_base_url(&format!("{}/api/search2", mock_server.uri()));
    let repos = client
        .repo_search("ratatui", Some("Rust"), true, "impl Widget", true)
        .await
        .expect("search with snippets should succeed");

    assert!(!repos.is_empty(), "should return repos");

    // At least one repo should have parsed file snippets
    let has_files = repos.iter().any(|r| !r.files.is_empty());
    assert!(has_files, "at least one repo should have file snippets");

    // Verify snippet structure
    for repo in &repos {
        for file in &repo.files {
            assert!(!file.path.is_empty(), "snippet path should not be empty");
            assert!(!file.lines.is_empty(), "snippet should have code lines");
        }
    }
}

#[tokio::test]
async fn test_empty_search_results() {
    let empty_response = serde_json::json!({
        "time": 0,
        "facets": {
            "path": {"buckets": []},
            "repo": {"buckets": []},
            "lang": {"buckets": []}
        },
        "hits": {
            "total": 0,
            "hits": []
        }
    });

    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(empty_response.to_string()))
        .mount(&mock_server)
        .await;

    let client = GrepClient::with_base_url(&format!("{}/api/search2", mock_server.uri()));
    let repos = client
        .repo_search("zzz_nonexistent_repo_xyz", None, true, ".*", false)
        .await
        .expect("search with no results should succeed");

    assert!(repos.is_empty(), "should return empty list");
}

#[tokio::test]
async fn test_parse_snippet_html_structure() {
    let response = serde_json::json!({
        "time": 42,
        "facets": {
            "path": {"buckets": [{"val": "src/lib.rs", "count": 1}]},
            "repo": {"buckets": [{"val": "test-org/test-repo", "count": 1, "owner_id": "123"}]},
            "lang": {"buckets": [{"val": "Rust", "count": 1}]}
        },
        "hits": {
            "total": 1,
            "hits": [{
                "owner_id": "123",
                "repo": "test-org/test-repo",
                "branch": "main",
                "path": "src/widget.rs",
                "content": {
                    "snippet": "<table><tr data-line=\"10\"><td class=\"lnum\">10</td><td class=\"highlight\"><pre>use ratatui::widgets::Widget;</pre></td></tr><tr data-line=\"11\"><td class=\"lnum\">11</td><td class=\"highlight\"><pre><mark>impl Widget</mark> for MyApp {</pre></td></tr><tr data-line=\"12\"><td class=\"lnum\">12</td><td class=\"highlight\"><pre>    fn render(self, area: Rect, buf: &amp;mut Buffer) {</pre></td></tr></table>"
                },
                "total_matches": "3"
            }]
        }
    });

    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(response.to_string()))
        .mount(&mock_server)
        .await;

    let client = GrepClient::with_base_url(&format!("{}/api/search2", mock_server.uri()));
    let repos = client
        .repo_search("test-org/test-repo", None, true, "impl Widget", true)
        .await
        .expect("snippet parsing should succeed");

    assert_eq!(repos.len(), 1);
    let repo = &repos[0];
    assert_eq!(repo.name, "test-org/test-repo");
    assert_eq!(repo.files.len(), 1);

    let file = &repo.files[0];
    assert_eq!(file.path, "src/widget.rs");
    assert_eq!(file.total_matches, 3);
    assert_eq!(file.lines.len(), 3);

    // Line 10: context (no match)
    assert_eq!(file.lines[0].line_number, 10);
    assert!(!file.lines[0].has_match);

    // Line 11: has <mark> match
    assert_eq!(file.lines[1].line_number, 11);
    assert!(file.lines[1].has_match);

    // Line 12: context (no match)
    assert_eq!(file.lines[2].line_number, 12);
    assert!(!file.lines[2].has_match);
}

#[tokio::test]
async fn test_deserialization_of_cassette_matches_types() {
    let body = load_cassette("repo_search_basic");
    let result: GrepSearchResult =
        serde_json::from_str(&body).expect("cassette should deserialize into GrepSearchResult");

    assert!(
        !result.facets.repo.buckets.is_empty(),
        "should have repo buckets"
    );
    assert!(result.hits.total > 0, "should have total hits");
}

// ── Smoke test (hits real API) ───────────────────────────────────────────────
// Run with: cargo test -p wits --test integration -- --ignored smoke

#[tokio::test]
#[ignore = "hits real grep.app API - smoke test"]
async fn smoke_test_api_returns_json() {
    let Some(body) = fetch_live_search_body("tokio", Some("Rust"), "async").await else {
        return;
    };

    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("q", "async"))
        .respond_with(ResponseTemplate::new(200).set_body_string(&body))
        .mount(&mock_server)
        .await;

    let client = GrepClient::with_base_url(&format!("{}/api/search2", mock_server.uri()));
    let repos = client
        .repo_search("tokio", Some("Rust"), true, "async", false)
        .await
        .expect("smoke test: API should return valid JSON and deserialize successfully");

    assert!(
        !repos.is_empty(),
        "smoke test: searching for 'async' in tokio repos should return results"
    );
    assert!(
        repos[0].hits > 0,
        "smoke test: top result should have positive hit count"
    );
}

// ── VCR cassette recorders (hit real API, save responses) ──────────────────
// Run with: cargo test -p wits --test integration -- --ignored record

async fn record_cassette(name: &str, repo_pattern: &str, lang: Option<&str>, query: &str) {
    let Some(body) = fetch_live_search_body(repo_pattern, lang, query).await else {
        return;
    };

    // Validate it's valid JSON
    let value: Value = serde_json::from_str(&body).expect("response should be valid JSON");

    // Pretty-print for readability
    let pretty = serde_json::to_string_pretty(&value).unwrap();

    let dir = cassette_dir();
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.json"));
    fs::write(&path, pretty).unwrap();
    eprintln!("Recorded cassette: {}", path.display());
}

#[tokio::test]
#[ignore = "hits real grep.app API - run to record cassettes"]
async fn record_repo_search_basic() {
    record_cassette("repo_search_basic", "ratatui", None, ".*").await;
}

#[tokio::test]
#[ignore = "hits real grep.app API - run to record cassettes"]
async fn record_repo_search_with_lang() {
    record_cassette("repo_search_with_lang", "ratatui", Some("Rust"), ".*").await;
}

#[tokio::test]
#[ignore = "hits real grep.app API - run to record cassettes"]
async fn record_repo_search_with_snippets() {
    record_cassette(
        "repo_search_with_snippets",
        "ratatui",
        Some("Rust"),
        "impl Widget",
    )
    .await;
}
