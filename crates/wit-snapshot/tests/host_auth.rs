use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, path},
};
use wit_snapshot::{GitHubHttpClient, ReqwestGitHubClient};

#[tokio::test]
async fn enterprise_prefix_and_redirect_boundary() {
    let server = MockServer::start().await;
    let other = MockServer::start().await;
    let client = ReqwestGitHubClient::new(
        format!("{}/api/v3", server.uri()),
        Some("synthetic-only".into()),
    )
    .unwrap();
    Mock::given(path("/api/v3/repos/o/r"))
        .and(header("Authorization", "Bearer synthetic-only"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&server)
        .await;
    assert_eq!(client.get_json("/repos/o/r").await.unwrap().0, 200);
    assert!(
        client
            .get_json(&format!("{}/api/v3/repos/o/r", other.uri()))
            .await
            .is_err()
    );
    assert!(
        client
            .get_json(&format!("{}/outside", server.uri()))
            .await
            .is_err()
    );
    Mock::given(path("/api/v3/redirect"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/stolen", other.uri())),
        )
        .mount(&server)
        .await;
    assert_eq!(client.get_json("/redirect").await.unwrap().0, 302);
    assert!(other.received_requests().await.unwrap().is_empty());
}
