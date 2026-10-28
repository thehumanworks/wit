//! Native, local-process GitHub configuration. No credential persistence.
use crate::{SnapshotError, SnapshotResult};
use std::sync::OnceLock;

static HOST: OnceLock<String> = OnceLock::new();
static TOKEN: OnceLock<Option<String>> = OnceLock::new();
fn invalid() -> SnapshotError {
    SnapshotError::Other(
        "invalid GitHub hostname (expected a DNS hostname, without URL or path)".into(),
    )
}

pub fn resolve_host(
    flag: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
) -> SnapshotResult<String> {
    let host = flag
        .map(str::to_owned)
        .or_else(|| env("GH_HOST").filter(|s| !s.is_empty()))
        .or_else(|| env("GITHUB_HOST").filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "github.com".into())
        .to_ascii_lowercase();
    if host.len() > 253
        || host.split('.').any(|part| {
            part.is_empty()
                || part.len() > 63
                || part.starts_with('-')
                || part.ends_with('-')
                || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(invalid());
    }
    Ok(host)
}
pub fn hostname() -> SnapshotResult<String> {
    HOST.get()
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| resolve_host(None, |k| std::env::var(k).ok()))
}
pub fn configure_host(flag: Option<&str>) -> SnapshotResult<()> {
    let host = resolve_host(flag, |k| std::env::var(k).ok())?;
    let credential = token(&host);
    HOST.set(host)
        .map_err(|_| SnapshotError::Other("GitHub host already configured".into()))?;
    let _ = TOKEN.set(credential);
    Ok(())
}
pub fn api_base(host: &str) -> String {
    if host == "github.com" {
        "https://api.github.com".into()
    } else if host.ends_with(".ghe.com") {
        format!("https://api.{host}")
    } else {
        format!("https://{host}/api/v3")
    }
}
pub fn environment_token(host: &str, env: impl Fn(&str) -> Option<String>) -> Option<String> {
    let keys = if host == "github.com" || host.ends_with(".ghe.com") {
        ["GH_TOKEN", "GITHUB_TOKEN"]
    } else {
        ["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"]
    };
    keys.into_iter().find_map(|k| {
        env(k)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    })
}
/// Read an existing local gh login. Never log in, refresh, or write credentials.
pub fn token(host: &str) -> Option<String> {
    if HOST.get().is_some_and(|selected| selected == host)
        && let Some(token) = TOKEN.get()
    {
        return token.clone();
    }
    environment_token(host, |k| std::env::var(k).ok())
        .or_else(|| cached_token(host, std::path::Path::new("gh")))
}
fn cached_token(host: &str, executable: &std::path::Path) -> Option<String> {
    let mut child = std::process::Command::new(executable)
        .args(["auth", "token", "--hostname", host])
        .env_remove("GH_DEBUG")
        .env_remove("DEBUG")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    // Bound both child execution and pipe draining: a wrapper may leave stdout
    // open in a descendant after the direct child has exited.
    let stdout = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        use std::io::{BufRead, Read};
        let mut bytes = Vec::new();
        let read = std::io::BufReader::new(stdout.take(8192)).read_until(b'\n', &mut bytes);
        let _ = sender.send(read.map(|_| bytes));
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let bytes = receiver
        .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        .ok()?
        .ok()?;
    if bytes.len() == 8192 {
        return None;
    }
    String::from_utf8(bytes)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn inherited_stdout_does_not_extend_lookup_deadline() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let gh = dir.path().join("gh");
        std::fs::write(&gh, "#!/bin/sh\n/bin/sleep 4 &\nprintf synthetic\n").unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
        let start = std::time::Instant::now();
        assert!(cached_token("git.example", &gh).is_none());
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
    }
    #[cfg(unix)]
    #[test]
    fn stalled_local_login_lookup_is_bounded() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let gh = dir.path().join("gh");
        std::fs::write(&gh, "#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
        let start = std::time::Instant::now();
        assert!(cached_token("git.example", &gh).is_none());
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }
    #[cfg(unix)]
    #[test]
    fn local_login_is_host_scoped_and_logout_is_observed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let gh = dir.path().join("gh");
        std::fs::write(&gh, "#!/bin/sh\n[ \"$1 $2 $3 $4\" = \"auth token --hostname git.example\" ] || exit 1\nprintf synthetic-cached\n").unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            cached_token("git.example", &gh).as_deref(),
            Some("synthetic-cached")
        );
        assert!(cached_token("other.example", &gh).is_none());
        std::fs::write(&gh, "#!/bin/sh\nexit 1\n").unwrap();
        assert!(cached_token("git.example", &gh).is_none());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn precedence_and_validation() {
        let env = |k: &str| {
            Some(
                if k == "GH_HOST" {
                    "one.example"
                } else {
                    "two.example"
                }
                .into(),
            )
        };
        assert_eq!(
            resolve_host(Some("THREE.example"), env).unwrap(),
            "three.example"
        );
        assert_eq!(resolve_host(None, env).unwrap(), "one.example");
        assert_eq!(
            resolve_host(None, |k| if k == "GITHUB_HOST" {
                Some("alias.example".into())
            } else {
                Some(String::new())
            })
            .unwrap(),
            "alias.example"
        );
        assert_eq!(resolve_host(None, |_| None).unwrap(), "github.com");
        assert!(resolve_host(Some("https://secret@example.com/path"), env).is_err());
    }
    #[test]
    fn tokens_are_host_scoped() {
        let env = |k: &str| match k {
            "GH_TOKEN" => Some("synthetic-public".into()),
            "GITHUB_ENTERPRISE_TOKEN" => Some("synthetic-enterprise".into()),
            _ => None,
        };
        assert_eq!(
            environment_token("github.com", env).as_deref(),
            Some("synthetic-public")
        );
        assert_eq!(
            environment_token("company.ghe.com", env).as_deref(),
            Some("synthetic-public")
        );
        assert_eq!(
            environment_token("git.example", env).as_deref(),
            Some("synthetic-enterprise")
        );
        assert_eq!(api_base("git.example"), "https://git.example/api/v3");
    }
}
