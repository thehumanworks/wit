//! Shared cloud pack cache (ADR 0009): a fill source tried before the GitHub clone.
//!
//! The cache is untrusted for integrity. The caller has already resolved the commit with
//! `git ls-remote` against GitHub; the pack is only accepted when `git index-pack --strict`
//! (object hashes, fsck, connectivity from the shallow boundary) succeeds and the resolved
//! commit is present, connected, and becomes HEAD. The client writes its own `shallow` file,
//! refs, HEAD, and config, so the server can never supply hooks or configuration. Requests
//! carry no credentials. Any failure returns `false` and the caller clones from GitHub.

use anyhow::{Context, bail, ensure};
use reqwest::{StatusCode, Url};
use std::{
    io::Read,
    net::IpAddr,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub const WIT_CACHE_URL_ENV: &str = "WIT_CACHE_URL";
pub const WIT_CACHE_TIMEOUT_MS_ENV: &str = "WIT_CACHE_TIMEOUT_MS";
pub const WIT_CACHE_MAX_BYTES_ENV: &str = "WIT_CACHE_MAX_BYTES";
/// Tomas's hosted instance (`services/wit-cache`), the default for release builds.
pub const HOSTED_CACHE_URL: &str = "https://wit-cache.rodat-human-ada.workers.dev";

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Matches the hosted Worker's `MAX_PACK_BYTES`.
const DEFAULT_MAX_BYTES: u64 = 512 * 1024 * 1024;
const GITHUB_PREFIX: &str = "https://github.com/";

/// Release builds use the hosted instance unless `WIT_CACHE_URL` overrides it; debug builds
/// (tests, `cargo run`) stay offline. Packagers can bake another default, or disable it with
/// an empty value, through `WIT_DEFAULT_CACHE_URL` at build time.
fn built_in_default() -> Option<&'static str> {
    match option_env!("WIT_DEFAULT_CACHE_URL") {
        Some(url) => Some(url),
        None if cfg!(debug_assertions) => None,
        None => Some(HOSTED_CACHE_URL),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudCacheConfig {
    pub base_url: Url,
    pub timeout: Duration,
    pub max_bytes: u64,
}

impl CloudCacheConfig {
    pub fn from_env() -> Option<Self> {
        let url = std::env::var(WIT_CACHE_URL_ENV).ok();
        let timeout = std::env::var(WIT_CACHE_TIMEOUT_MS_ENV).ok();
        let max_bytes = std::env::var(WIT_CACHE_MAX_BYTES_ENV).ok();
        Self::from_values(
            url.as_deref(),
            timeout.as_deref(),
            max_bytes.as_deref(),
            built_in_default(),
        )
    }

    /// `url` wins over `default_url`; empty, `off`, `0`, `false`, `no`, `none`, and
    /// `disabled` turn the cache off. Only `https://` (or `http://` on loopback) URLs without
    /// embedded credentials, query, or fragment are accepted.
    pub fn from_values(
        url: Option<&str>,
        timeout_ms: Option<&str>,
        max_bytes: Option<&str>,
        default_url: Option<&str>,
    ) -> Option<Self> {
        let raw = url.or(default_url)?.trim();
        if is_disabled(raw) {
            return None;
        }
        let base_url = parse_base_url(raw)?;
        let timeout = parse_positive(timeout_ms)
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_TIMEOUT);
        let max_bytes = parse_positive(max_bytes).unwrap_or(DEFAULT_MAX_BYTES);
        Some(Self {
            base_url,
            timeout,
            max_bytes,
        })
    }

    pub fn pack_url(&self, owner: &str, repo: &str, commit: &str, branch: &str) -> Option<Url> {
        let mut url = self.base_url.clone();
        url.path_segments_mut().ok()?.pop_if_empty().extend([
            "v1",
            "github",
            owner,
            repo,
            &format!("{commit}.pack"),
        ]);
        url.query_pairs_mut().append_pair("branch", branch);
        Some(url)
    }
}

fn parse_positive(value: Option<&str>) -> Option<u64> {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
}

fn is_disabled(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "" | "off" | "0" | "false" | "no" | "none" | "disabled"
    )
}

fn parse_base_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    match url.scheme() {
        "https" => Some(url),
        "http" if is_loopback(&url) => Some(url),
        _ => None,
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host_str() {
        Some(host) if host.eq_ignore_ascii_case("localhost") => true,
        Some(host) => host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        None => false,
    }
}

fn github_owner_repo(remote_url: &str) -> Option<(&str, &str)> {
    let (owner, repo) = remote_url.strip_prefix(GITHUB_PREFIX)?.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty() && !repo.contains('/')).then_some((owner, repo))
}

fn is_commit_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
thread_local! {
    static TEST_CONFIG: std::cell::RefCell<Option<Option<CloudCacheConfig>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_test_config(config: Option<CloudCacheConfig>) {
    TEST_CONFIG.with(|slot| *slot.borrow_mut() = Some(config));
}

fn active_config() -> Option<CloudCacheConfig> {
    #[cfg(test)]
    if let Some(config) = TEST_CONFIG.with(|slot| slot.borrow().clone()) {
        return config;
    }
    CloudCacheConfig::from_env()
}

/// Build the bare repository `dest` from the cloud pack for `commit` of a GitHub remote.
/// Returns `false` (with `dest` removed) when the cache is disabled, misses, or serves
/// anything that does not verify; the caller then clones from GitHub.
pub(crate) fn fill_from_cloud(
    remote_url: &str,
    branch: &str,
    commit: &str,
    dest: &Path,
    deadline: Option<Instant>,
) -> bool {
    fill_with(active_config(), remote_url, branch, commit, dest, deadline)
}

fn fill_with(
    config: Option<CloudCacheConfig>,
    remote_url: &str,
    branch: &str,
    commit: &str,
    dest: &Path,
    deadline: Option<Instant>,
) -> bool {
    let Some(mut config) = config else {
        return false;
    };
    let Some((owner, repo)) = github_owner_repo(remote_url) else {
        return false;
    };
    if let Some(deadline) = deadline {
        match deadline.checked_duration_since(Instant::now()) {
            Some(left) if !left.is_zero() => config.timeout = config.timeout.min(left),
            _ => return false,
        }
    }
    let result = std::thread::scope(|scope| {
        scope
            .spawn(|| fetch_pack_into(&config, owner, repo, branch, commit, remote_url, dest))
            .join()
            .unwrap_or_else(|_| Err(anyhow::anyhow!("cloud cache worker panicked")))
    });
    match result {
        Ok(()) => {
            tracing::debug!(%owner, %repo, %commit, "filled cache from cloud pack");
            true
        }
        Err(err) => {
            tracing::debug!(error = %format!("{err:#}"), "cloud cache unavailable; cloning from GitHub");
            let _ = std::fs::remove_dir_all(dest);
            false
        }
    }
}

/// Download and verify the pack into a fresh bare repository at `dest`. Runs on its own
/// thread because the blocking HTTP client must not live on an async runtime thread.
pub fn fetch_pack_into(
    config: &CloudCacheConfig,
    owner: &str,
    repo: &str,
    branch: &str,
    commit: &str,
    remote_url: &str,
    dest: &Path,
) -> anyhow::Result<()> {
    ensure!(
        is_commit_sha(commit),
        "expected a 40-hex commit, got '{commit}'"
    );
    let url = config
        .pack_url(owner, repo, commit, branch)
        .context("cloud cache URL cannot hold a path")?;
    crate::ensure_rustls_provider();
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT.min(config.timeout))
        .timeout(config.timeout)
        .user_agent(concat!("wit/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build cloud cache HTTP client")?;
    let mut response = client
        .get(url)
        .send()
        .context("cloud cache request failed")?;
    if response.status() != StatusCode::OK {
        bail!("cloud cache answered HTTP {}", response.status());
    }
    if let Some(len) = response.content_length()
        && len > config.max_bytes
    {
        bail!(
            "cloud pack is {len} bytes, over the {} byte limit",
            config.max_bytes
        );
    }
    if let Some(served) = response.headers().get("x-wit-commit")
        && served.as_bytes() != commit.as_bytes()
    {
        bail!("cloud cache served a pack for a different commit");
    }

    if dest.exists() {
        std::fs::remove_dir_all(dest)
            .with_context(|| format!("failed to clear '{}'", dest.display()))?;
    }
    git(None, &["init", "--bare", "--quiet"], Some(dest))?;
    git(
        Some(dest),
        &["config", "remote.origin.url", remote_url],
        None,
    )?;
    std::fs::write(dest.join("shallow"), format!("{commit}\n"))
        .context("failed to write shallow file")?;
    index_pack(dest, &mut response, config.max_bytes)?;

    let head_ref = format!("refs/heads/{branch}");
    git(Some(dest), &["update-ref", &head_ref, commit], None)
        .context("cloud pack does not contain the resolved commit")?;
    git(Some(dest), &["symbolic-ref", "HEAD", &head_ref], None)?;
    git(
        Some(dest),
        &["rev-list", "--objects", "--missing=error", commit],
        None,
    )
    .context("cloud pack is not connected")?;

    let repo = gix::open(dest).context("failed to open cloud-filled cache")?;
    let head = repo
        .head_commit()
        .context("cloud-filled cache has no HEAD commit")?;
    ensure!(
        head.id().to_string() == commit,
        "cloud-filled HEAD does not match the resolved commit"
    );
    Ok(())
}

/// Stream `body` into `git index-pack --strict --stdin`, capped at `max_bytes`. The `shallow`
/// file must already exist so `--strict` treats the commit's missing parents as the boundary.
fn index_pack(dest: &Path, body: &mut impl Read, max_bytes: u64) -> anyhow::Result<()> {
    let mut child = Command::new("git")
        .arg("--git-dir")
        .arg(dest)
        .args(["index-pack", "--strict", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start git index-pack")?;
    let mut stdin = child.stdin.take().context("git index-pack has no stdin")?;
    let copied = std::io::copy(&mut body.take(max_bytes + 1), &mut stdin);
    drop(stdin);
    let fail = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    match copied {
        Ok(n) if n > max_bytes => {
            fail(&mut child);
            bail!("cloud pack exceeded the {max_bytes} byte limit");
        }
        Ok(_) => {}
        Err(err) => {
            fail(&mut child);
            return Err(err).context("cloud pack download failed");
        }
    }
    let output = child
        .wait_with_output()
        .context("failed to wait for git index-pack")?;
    if !output.status.success() {
        bail!(
            "git index-pack rejected the cloud pack: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn git(git_dir: Option<&Path>, args: &[&str], path_arg: Option<&Path>) -> anyhow::Result<()> {
    let mut command = Command::new("git");
    if let Some(dir) = git_dir {
        command.arg("--git-dir").arg(dir);
    }
    command.args(args);
    if let Some(path) = path_arg {
        command.arg(path);
    }
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("failed to run git {}", args[0]))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    fn run_git(args: &[&str], dir: &Path) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "wit")
            .env("GIT_AUTHOR_EMAIL", "wit@example.com")
            .env("GIT_COMMITTER_NAME", "wit")
            .env("GIT_COMMITTER_EMAIL", "wit@example.com")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// A two-commit repository and the depth-1 pack GitHub would serve for its tip.
    pub(crate) struct Fixture {
        _temp: tempfile::TempDir,
        pub(crate) commit: String,
        pub(crate) pack: Vec<u8>,
    }

    pub(crate) fn fixture(content: &str) -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        std::fs::create_dir(&src).unwrap();
        run_git(&["init", "--quiet", "--initial-branch=main"], &src);
        std::fs::write(src.join("README.md"), "first\n").unwrap();
        run_git(&["add", "."], &src);
        run_git(&["commit", "--quiet", "-m", "first"], &src);
        std::fs::create_dir(src.join("src")).unwrap();
        std::fs::write(src.join("src/lib.rs"), content).unwrap();
        run_git(&["add", "."], &src);
        run_git(&["commit", "--quiet", "-m", "second"], &src);
        let commit = run_git(&["rev-parse", "HEAD"], &src);
        let shallow = temp.path().join("shallow.git");
        let url = format!("file://{}", src.display());
        run_git(
            &[
                "clone",
                "--quiet",
                "--bare",
                "--depth",
                "1",
                "--no-local",
                &url,
                shallow.to_str().unwrap(),
            ],
            temp.path(),
        );
        let pack_dir = shallow.join("objects/pack");
        let pack_path = std::fs::read_dir(&pack_dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|x| x == "pack"))
            .unwrap();
        Fixture {
            pack: std::fs::read(pack_path).unwrap(),
            commit,
            _temp: temp,
        }
    }

    fn config_for(server: &MockServer) -> CloudCacheConfig {
        CloudCacheConfig::from_values(Some(&server.uri()), Some("5000"), None, None).unwrap()
    }

    fn pack_path_for(commit: &str) -> String {
        format!("/v1/github/octo/demo/{commit}.pack")
    }

    async fn fill(config: CloudCacheConfig, commit: &str) -> (bool, PathBuf, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("repo.git");
        let commit = commit.to_string();
        let dest_clone = dest.clone();
        let ok = tokio::task::spawn_blocking(move || {
            fill_with(
                Some(config),
                "https://github.com/octo/demo",
                "main",
                &commit,
                &dest_clone,
                None,
            )
        })
        .await
        .unwrap();
        (ok, dest, temp)
    }

    async fn serve(server: &MockServer, commit: &str, response: ResponseTemplate) {
        Mock::given(method("GET"))
            .and(path(pack_path_for(commit)))
            .and(query_param("branch", "main"))
            .respond_with(response)
            .mount(server)
            .await;
    }

    fn pack_response(bytes: &[u8], commit: &str) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/x-git-packfile")
            .insert_header("x-wit-commit", commit)
            .set_body_bytes(bytes.to_vec())
    }

    #[test]
    fn config_disable_values_and_url_rules() {
        let default = Some(HOSTED_CACHE_URL);
        assert_eq!(
            CloudCacheConfig::from_values(None, None, None, default)
                .unwrap()
                .base_url
                .as_str(),
            "https://wit-cache.rodat-human-ada.workers.dev/"
        );
        assert!(CloudCacheConfig::from_values(None, None, None, None).is_none());
        for off in [
            "", "  ", "off", "OFF", "0", "false", "none", "disabled", "no",
        ] {
            assert!(
                CloudCacheConfig::from_values(Some(off), None, None, default).is_none(),
                "{off:?} should disable the cache"
            );
        }
        for rejected in [
            "http://cache.example.com",
            "ftp://cache.example.com",
            "https://user:secret@cache.example.com",
            "https://cache.example.com/?key=value",
            "not a url",
        ] {
            assert!(
                CloudCacheConfig::from_values(Some(rejected), None, None, None).is_none(),
                "{rejected} must be rejected"
            );
        }
        for loopback in [
            "http://127.0.0.1:8787",
            "http://localhost:8787",
            "http://[::1]:8787",
        ] {
            assert!(CloudCacheConfig::from_values(Some(loopback), None, None, None).is_some());
        }
        let tuned = CloudCacheConfig::from_values(
            Some("https://cache.example.com/base/"),
            Some("2500"),
            Some("1024"),
            None,
        )
        .unwrap();
        assert_eq!(tuned.timeout, Duration::from_millis(2500));
        assert_eq!(tuned.max_bytes, 1024);
        let fallback =
            CloudCacheConfig::from_values(Some("https://c.example"), Some("x"), Some("0"), None)
                .unwrap();
        assert_eq!(fallback.timeout, DEFAULT_TIMEOUT);
        assert_eq!(fallback.max_bytes, DEFAULT_MAX_BYTES);
    }

    #[test]
    fn pack_url_keeps_base_path_and_encodes_branch() {
        let config = CloudCacheConfig::from_values(
            Some("https://cache.example.com/base/"),
            None,
            None,
            None,
        )
        .unwrap();
        let url = config
            .pack_url("octo", "demo", &"a".repeat(40), "feature/x y")
            .unwrap();
        assert_eq!(
            url.as_str(),
            format!(
                "https://cache.example.com/base/v1/github/octo/demo/{}.pack?branch=feature%2Fx+y",
                "a".repeat(40)
            )
        );
    }

    #[test]
    fn only_github_remotes_use_the_cloud() {
        assert_eq!(
            github_owner_repo("https://github.com/openai/codex"),
            Some(("openai", "codex"))
        );
        assert_eq!(github_owner_repo("/tmp/local.git"), None);
        assert_eq!(github_owner_repo("https://gitlab.com/o/r"), None);
        assert_eq!(github_owner_repo("https://github.com/o/r/extra"), None);
    }

    #[tokio::test]
    async fn hit_builds_a_verified_shallow_cache() {
        let fx = fixture("pub fn cloud() {}\n");
        let server = MockServer::builder().start().await;
        serve(&server, &fx.commit, pack_response(&fx.pack, &fx.commit)).await;

        let (ok, dest, _temp) = fill(config_for(&server), &fx.commit).await;
        assert!(ok, "valid pack should fill the cache");

        let repo = gix::open(&dest).unwrap();
        assert_eq!(repo.head_commit().unwrap().id().to_string(), fx.commit);
        assert_eq!(
            crate::gitops::ops::read_file(&repo, "src/lib.rs").unwrap(),
            "pub fn cloud() {}\n"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("shallow")).unwrap(),
            format!("{}\n", fx.commit)
        );
        let origin = Command::new("git")
            .arg("--git-dir")
            .arg(&dest)
            .args(["config", "remote.origin.url"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&origin.stdout).trim(),
            "https://github.com/octo/demo"
        );
    }

    #[tokio::test]
    async fn requests_carry_no_authorization_header() {
        let fx = fixture("x\n");
        let server = MockServer::builder().start().await;
        serve(&server, &fx.commit, pack_response(&fx.pack, &fx.commit)).await;
        let (ok, _dest, _temp) = fill(config_for(&server), &fx.commit).await;
        assert!(ok);

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert!(request.headers.get("authorization").is_none());
        assert!(request.headers.get("cookie").is_none());
        let agent = request.headers.get("user-agent").unwrap().to_str().unwrap();
        assert!(agent.starts_with("wit/"), "{agent}");
    }

    #[tokio::test]
    async fn miss_falls_back_and_leaves_no_directory() {
        let fx = fixture("x\n");
        let server = MockServer::builder().start().await;
        serve(
            &server,
            &fx.commit,
            ResponseTemplate::new(404).set_body_string(r#"{"fill":"queued"}"#),
        )
        .await;
        let (ok, dest, _temp) = fill(config_for(&server), &fx.commit).await;
        assert!(!ok);
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn corrupt_pack_falls_back() {
        let fx = fixture("x\n");
        let mut corrupt = fx.pack.clone();
        let mid = corrupt.len() / 2;
        corrupt[mid] ^= 0xff;
        let server = MockServer::builder().start().await;
        serve(&server, &fx.commit, pack_response(&corrupt, &fx.commit)).await;
        let (ok, dest, _temp) = fill(config_for(&server), &fx.commit).await;
        assert!(!ok);
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn truncated_pack_falls_back() {
        let fx = fixture("x\n");
        let server = MockServer::builder().start().await;
        let half = &fx.pack[..fx.pack.len() / 2];
        serve(&server, &fx.commit, pack_response(half, &fx.commit)).await;
        let (ok, _dest, _temp) = fill(config_for(&server), &fx.commit).await;
        assert!(!ok);
    }

    #[tokio::test]
    async fn pack_for_another_commit_falls_back() {
        let wanted = fixture("wanted\n");
        let other = fixture("other\n");
        let server = MockServer::builder().start().await;
        let response = ResponseTemplate::new(200).set_body_bytes(other.pack.clone());
        serve(&server, &wanted.commit, response).await;
        let (ok, dest, _temp) = fill(config_for(&server), &wanted.commit).await;
        assert!(
            !ok,
            "a valid pack that lacks the resolved commit must be rejected"
        );
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn wrong_commit_header_falls_back_before_indexing() {
        let fx = fixture("x\n");
        let server = MockServer::builder().start().await;
        serve(
            &server,
            &fx.commit,
            pack_response(&fx.pack, &"f".repeat(40)),
        )
        .await;
        let (ok, dest, _temp) = fill(config_for(&server), &fx.commit).await;
        assert!(!ok);
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn oversize_pack_falls_back() {
        let fx = fixture("x\n");
        let server = MockServer::builder().start().await;
        serve(&server, &fx.commit, pack_response(&fx.pack, &fx.commit)).await;
        let mut config = config_for(&server);
        config.max_bytes = (fx.pack.len() as u64) - 1;
        let (ok, dest, _temp) = fill(config, &fx.commit).await;
        assert!(!ok);
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn slow_cache_times_out_and_falls_back() {
        let fx = fixture("x\n");
        let server = MockServer::builder().start().await;
        serve(
            &server,
            &fx.commit,
            pack_response(&fx.pack, &fx.commit).set_delay(Duration::from_secs(3)),
        )
        .await;
        let mut config = config_for(&server);
        config.timeout = Duration::from_millis(300);
        let started = Instant::now();
        let (ok, _dest, _temp) = fill(config, &fx.commit).await;
        assert!(!ok);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn expired_deadline_skips_the_request() {
        let fx = fixture("x\n");
        let server = MockServer::builder().start().await;
        serve(&server, &fx.commit, pack_response(&fx.pack, &fx.commit)).await;
        let config = config_for(&server);
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("repo.git");
        let commit = fx.commit.clone();
        let dest_clone = dest.clone();
        let ok = tokio::task::spawn_blocking(move || {
            fill_with(
                Some(config),
                "https://github.com/octo/demo",
                "main",
                &commit,
                &dest_clone,
                Some(Instant::now()),
            )
        })
        .await
        .unwrap();
        assert!(!ok);
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn disabled_or_non_github_never_calls_the_cache() {
        let server = MockServer::builder().start().await;
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("repo.git");
        let commit = "a".repeat(40);
        let config = config_for(&server);
        let (d1, d2, c1, c2) = (dest.clone(), dest.clone(), commit.clone(), commit.clone());
        let results = tokio::task::spawn_blocking(move || {
            (
                fill_with(None, "https://github.com/octo/demo", "main", &c1, &d1, None),
                fill_with(Some(config), "/srv/git/demo.git", "main", &c2, &d2, None),
            )
        })
        .await
        .unwrap();
        assert_eq!(results, (false, false));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
