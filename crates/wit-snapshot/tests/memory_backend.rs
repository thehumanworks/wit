use std::{collections::HashMap, sync::Arc};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, path_regex},
};
use wit_snapshot::{
    DirEntry, EntryKind, GitHubHttpClient, MemoryBackend, MemoryBackendLimits, RepoSnapshot,
    SnapshotBackend, SnapshotError, SnapshotResult,
};

struct MockClient {
    base: String,
    routes: HashMap<String, (u16, String)>,
}

impl MockClient {
    fn new(base: String) -> Self {
        Self {
            base,
            routes: HashMap::new(),
        }
    }

    fn insert(&mut self, path: &str, status: u16, body: impl Into<String>) {
        self.routes.insert(path.to_string(), (status, body.into()));
    }
}

impl GitHubHttpClient for MockClient {
    async fn get_json(&self, request_path: &str) -> SnapshotResult<(u16, String)> {
        let key = if request_path.starts_with("http") {
            request_path
                .strip_prefix(&self.base)
                .unwrap_or(request_path)
                .to_string()
        } else {
            request_path.to_string()
        };
        let key = if key.starts_with('/') {
            key
        } else {
            format!("/{key}")
        };
        self.routes
            .get(&key)
            .cloned()
            .ok_or_else(|| SnapshotError::Other(format!("unexpected path {key}")))
    }
}

fn sample_tree_json() -> String {
    serde_json::json!({
        "sha": "treesha",
        "truncated": false,
        "tree": [
            {"path": "README.md", "mode": "100644", "type": "blob", "sha": "blob-readme", "size": 12},
            {"path": "src", "mode": "040000", "type": "tree", "sha": "tree-src"},
            {"path": "src/lib.rs", "mode": "100644", "type": "blob", "sha": "blob-lib", "size": 5},
            {"path": "assets", "mode": "040000", "type": "tree", "sha": "tree-assets"},
            {"path": "assets/logo.bin", "mode": "100644", "type": "blob", "sha": "blob-bin", "size": 4}
        ]
    })
    .to_string()
}

fn public_repo_json() -> String {
    serde_json::json!({
        "private": false,
        "default_branch": "main"
    })
    .to_string()
}

fn commit_json() -> String {
    serde_json::json!({
        "sha": "abc123commit",
        "commit": {"tree": {"sha": "treesha"}}
    })
    .to_string()
}

fn text_blob(content: &str) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    serde_json::json!({
        "sha": "ignored",
        "size": content.len(),
        "encoding": "base64",
        "content": STANDARD.encode(content)
    })
    .to_string()
}

fn binary_blob() -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let bytes = [0x00u8, 0x01, 0x02, 0xff];
    serde_json::json!({
        "sha": "blob-bin",
        "size": bytes.len(),
        "encoding": "base64",
        "content": STANDARD.encode(bytes)
    })
    .to_string()
}

fn primed_backend(limits: MemoryBackendLimits) -> MemoryBackend<MockClient> {
    let mut client = MockClient::new(String::new());
    client.insert("/repos/acme/demo", 200, public_repo_json());
    client.insert("/repos/acme/demo/commits/main", 200, commit_json());
    client.insert(
        "/repos/acme/demo/git/trees/treesha?recursive=1",
        200,
        sample_tree_json(),
    );
    client.insert(
        "/repos/acme/demo/git/blobs/blob-readme",
        200,
        text_blob("hello world\n"),
    );
    client.insert(
        "/repos/acme/demo/git/blobs/blob-lib",
        200,
        text_blob("fn a\n"),
    );
    client.insert("/repos/acme/demo/git/blobs/blob-bin", 200, binary_blob());
    MemoryBackend::new(client, limits)
}

#[tokio::test]
async fn memory_open_list_read_roundtrip() {
    let backend = primed_backend(MemoryBackendLimits::default());
    let snap = backend.open("acme/demo", None).await.unwrap();
    assert_eq!(snap.provenance().backend, "memory");
    assert_eq!(snap.provenance().commit_sha, "abc123commit");
    assert_eq!(snap.provenance().cache_state, "memory");

    let root = snap.list(None).unwrap();
    let names: Vec<_> = root.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"README.md"));
    assert!(names.contains(&"src"));
    assert!(names.contains(&"assets"));

    let src = snap.list(Some("src")).unwrap();
    assert_eq!(
        src,
        vec![DirEntry {
            name: "lib.rs".into(),
            kind: EntryKind::File,
            path: "src/lib.rs".into(),
            blob_sha: Some("blob-lib".into()),
            size_bytes: Some(5),
            is_binary: false,
        }]
    );

    let tree = snap.tree(Some("src")).unwrap();
    assert_eq!(tree.entries.len(), 1);
    assert_eq!(tree.entries[0].path, "src/lib.rs");

    let file = snap.read("README.md").await.unwrap();
    assert_eq!(file.text, "hello world\n");
    assert_eq!(file.blob_sha, "blob-readme");
}

#[tokio::test]
async fn memory_missing_path_and_binary() {
    let backend = primed_backend(MemoryBackendLimits::default());
    let snap = backend.open("acme/demo", Some("main")).await.unwrap();
    let missing = snap.read("nope.txt").await.unwrap_err();
    assert!(matches!(missing, SnapshotError::MissingPath(_)));
    let binary = snap.read("assets/logo.bin").await.unwrap_err();
    assert!(matches!(binary, SnapshotError::BinaryFile(_)));
    let listed = snap.list(Some("missing-dir")).unwrap_err();
    assert!(matches!(listed, SnapshotError::MissingPath(_)));
}

#[tokio::test]
async fn memory_rate_limit_and_private_repo() {
    let mut client = MockClient::new(String::new());
    client.insert("/repos/acme/private", 404, r#"{"message":"Not Found"}"#);
    let backend = MemoryBackend::new(client, MemoryBackendLimits::default());
    let err = backend
        .open("acme/private", None)
        .await
        .err()
        .expect("private repo should fail");
    assert!(matches!(err, SnapshotError::PrivateRepo(_)));

    let mut client = MockClient::new(String::new());
    client.insert(
        "/repos/acme/demo",
        403,
        r#"{"message":"API rate limit exceeded"}"#,
    );
    let backend = MemoryBackend::new(client, MemoryBackendLimits::default());
    let err = backend
        .open("acme/demo", None)
        .await
        .err()
        .expect("rate limit should fail");
    assert!(matches!(err, SnapshotError::RateLimited(_)));
}

#[tokio::test]
async fn memory_private_flag_on_200() {
    let mut client = MockClient::new(String::new());
    client.insert(
        "/repos/acme/secret",
        200,
        serde_json::json!({"private": true, "default_branch": "main"}).to_string(),
    );
    let backend = MemoryBackend::new(client, MemoryBackendLimits::default());
    let err = backend
        .open("acme/secret", None)
        .await
        .err()
        .expect("private flag should fail");
    assert!(matches!(err, SnapshotError::PrivateRepo(_)));
}

#[tokio::test]
async fn memory_oversized_tree_and_blob() {
    let mut client = MockClient::new(String::new());
    client.insert("/repos/acme/demo", 200, public_repo_json());
    client.insert("/repos/acme/demo/commits/main", 200, commit_json());
    client.insert(
        "/repos/acme/demo/git/trees/treesha?recursive=1",
        200,
        serde_json::json!({
            "sha": "treesha",
            "truncated": true,
            "tree": [{"path": "a", "type": "blob", "sha": "x", "size": 1}]
        })
        .to_string(),
    );
    let backend = MemoryBackend::new(client, MemoryBackendLimits::default());
    let err = backend
        .open("acme/demo", None)
        .await
        .err()
        .expect("truncated tree should fail");
    assert!(matches!(err, SnapshotError::OversizedTree(_)));

    let limits = MemoryBackendLimits {
        max_blob_bytes: 4,
        ..MemoryBackendLimits::default()
    };
    let backend = primed_backend(limits);
    let snap = backend.open("acme/demo", None).await.unwrap();
    let err = snap.read("README.md").await.unwrap_err();
    assert!(matches!(err, SnapshotError::OversizedBlob(_)));
}

#[tokio::test]
async fn memory_memory_pressure_on_tiny_budget() {
    let limits = MemoryBackendLimits {
        memory_budget_bytes: 4,
        max_blob_bytes: 1024,
        ..MemoryBackendLimits::default()
    };
    let backend = primed_backend(limits);
    let snap = backend.open("acme/demo", None).await.unwrap();
    let err = snap.read("README.md").await.unwrap_err();
    assert!(matches!(err, SnapshotError::MemoryPressure(_)));
}

#[tokio::test]
async fn memory_backend_never_writes_cache_dir() {
    let dir = tempfile::tempdir().unwrap();
    // Safety: test isolation; restore not required because process-local.
    unsafe {
        std::env::set_var("WIT_CACHE_DIR", dir.path());
    }
    let backend = primed_backend(MemoryBackendLimits::default());
    let snap = backend.open("acme/demo", None).await.unwrap();
    let _ = snap.list(None).unwrap();
    let _ = snap.read("README.md").await.unwrap();
    assert!(
        std::fs::read_dir(dir.path()).unwrap().next().is_none(),
        "memory backend must not create cache files"
    );
}

/// Integration-style: real HTTP server (wiremock), still no live GitHub.
#[tokio::test]
async fn memory_wiremock_http_open_list_read() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/demo"))
        .respond_with(ResponseTemplate::new(200).set_body_string(public_repo_json()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/demo/commits/main"))
        .respond_with(ResponseTemplate::new(200).set_body_string(commit_json()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/demo/git/trees/treesha"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sample_tree_json()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"/repos/acme/demo/git/blobs/blob-readme"))
        .respond_with(ResponseTemplate::new(200).set_body_string(text_blob("hello world\n")))
        .mount(&server)
        .await;

    // Route through our mock client mapped to wiremock-served bodies by using
    // ReqwestGitHubClient against the mock server base URL.
    let client = wit_snapshot::ReqwestGitHubClient::new(server.uri(), None).unwrap();
    let backend = MemoryBackend::new(client, MemoryBackendLimits::default());
    let snap = backend.open("acme/demo", Some("main")).await.unwrap();
    assert_eq!(snap.list(None).unwrap().len(), 3);
    assert_eq!(snap.read("README.md").await.unwrap().text, "hello world\n");
}

#[tokio::test]
async fn fixture_snapshot_list_read_without_network_tree_fetch() {
    struct BlobOnly;
    impl GitHubHttpClient for BlobOnly {
        async fn get_json(&self, path: &str) -> SnapshotResult<(u16, String)> {
            if path.contains("blob-readme") {
                Ok((200, text_blob("fixture read\n")))
            } else {
                Err(SnapshotError::Other(path.to_string()))
            }
        }
    }
    let snap = wit_snapshot::snapshot_from_tree_json(
        Arc::new(BlobOnly),
        "acme/demo",
        "main",
        "abc",
        "treesha",
        &sample_tree_json(),
        MemoryBackendLimits::default(),
    )
    .unwrap();
    assert!(
        snap.list(None)
            .unwrap()
            .iter()
            .any(|e| e.name == "README.md")
    );
    assert_eq!(snap.read("README.md").await.unwrap().text, "fixture read\n");
}
