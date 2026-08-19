//! Host-supplied HTTP client for wasm32 (`FetchGitHubClient`).
//!
//! Implements the same [`GitHubHttpClient::get_json`] contract as
//! [`crate::ReqwestGitHubClient`]. The JS/WASI host provides
//! `wit_snapshot_host::http_get`; failures map to [`SnapshotError::Api`]
//! (or status-derived typed errors once the body is returned).

use crate::memory::GitHubHttpClient;
use crate::{SnapshotError, SnapshotResult};

/// GitHub HTTP client that delegates each `get_json` to the wasm host.
///
/// The host may satisfy the request via live `api.github.com`, a same-origin
/// CORS proxy, or an in-memory fixture map. This type is not a third snapshot
/// backend — it is only an HTTP adapter for [`crate::MemoryBackend`].
#[derive(Debug, Clone)]
pub struct FetchGitHubClient {
    base_url: String,
}

impl FetchGitHubClient {
    /// Create a client. When `base_url` is non-empty, relative API paths are
    /// prefixed before being handed to the host (e.g. `https://api.github.com`).
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// Default: empty base URL so the host receives the raw API path
    /// (`/repos/...`) and decides how to resolve it.
    pub fn from_host() -> Self {
        Self::new(String::new())
    }
}

impl Default for FetchGitHubClient {
    fn default() -> Self {
        Self::from_host()
    }
}

impl GitHubHttpClient for FetchGitHubClient {
    async fn get_json(&self, path: &str) -> SnapshotResult<(u16, String)> {
        let request = resolve_request(&self.base_url, path);
        host_http_get(&request)
    }
}

fn resolve_request(base_url: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else if base_url.is_empty() {
        if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        }
    } else {
        format!("{base_url}/{}", path.trim_start_matches('/'))
    }
}

fn host_http_get(path: &str) -> SnapshotResult<(u16, String)> {
    let mut status: u16 = 0;
    let mut body_ptr: *mut u8 = std::ptr::null_mut();
    let mut body_len: usize = 0;
    // SAFETY: host import writes status and an alloc'd body pointer on success.
    let rc = unsafe {
        wit_snapshot_host_http_get(
            path.as_ptr(),
            path.len(),
            &mut status,
            &mut body_ptr,
            &mut body_len,
        )
    };
    if rc != 0 {
        return Err(SnapshotError::Api {
            status: 0,
            message: format!("host fetch failed for {path} (host code {rc})"),
        });
    }
    if body_ptr.is_null() && body_len != 0 {
        return Err(SnapshotError::Api {
            status: 0,
            message: format!("host fetch returned null body for {path}"),
        });
    }
    // SAFETY: body was allocated by the host via `wit_snapshot_alloc`.
    let body = unsafe {
        let text = if body_len == 0 {
            String::new()
        } else {
            let slice = std::slice::from_raw_parts(body_ptr, body_len);
            let owned = slice.to_vec();
            crate::wasm_abi::wit_snapshot_dealloc(body_ptr, body_len);
            String::from_utf8(owned).map_err(|_| SnapshotError::Api {
                status,
                message: format!("host fetch body for {path} was not UTF-8"),
            })?
        };
        text
    };
    Ok((status, body))
}

#[link(wasm_import_module = "wit_snapshot_host")]
unsafe extern "C" {
    /// Host HTTP GET. On success returns `0`, writes HTTP status, and sets
    /// `body_ptr_out`/`body_len_out` to a buffer allocated with
    /// [`crate::wasm_abi::wit_snapshot_alloc`]. Non-zero means host/fetch
    /// failure (mapped to [`SnapshotError::Api`]).
    fn wit_snapshot_host_http_get(
        path_ptr: *const u8,
        path_len: usize,
        status_out: *mut u16,
        body_ptr_out: *mut *mut u8,
        body_len_out: *mut usize,
    ) -> i32;
}
