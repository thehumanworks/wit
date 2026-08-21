//! wasm32 C ABI for `open` / `list` / `read` over [`MemoryBackend`] +
//! [`FetchGitHubClient`], plus a thin `get_json` wrap for repository search.
//!
//! Typed errors are returned as stable integer codes (see `ERR_*` constants).
//! Detail text is available via [`wit_snapshot_last_error`].

use crate::fetch::FetchGitHubClient;
use crate::memory::{GitHubHttpClient, MemoryBackend, MemoryBackendLimits, MemorySnapshot};
use crate::{
    RepoSnapshot, SnapshotBackend, SnapshotError, SnapshotResult, normalize_repo_path,
    search_repositories_path,
};
use std::{
    future::Future,
    sync::Mutex,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

/// Success.
pub const ERR_OK: i32 = 0;
/// GitHub rate limit (`SnapshotError::RateLimited`).
pub const ERR_RATE_LIMIT: i32 = 1;
/// Tree or blob too large (`OversizedTree` / `OversizedBlob`).
pub const ERR_OVERSIZED: i32 = 2;
/// Path missing (`MissingPath`).
pub const ERR_NOT_FOUND: i32 = 3;
/// Binary blob (`BinaryFile`).
pub const ERR_BINARY: i32 = 4;
/// Private / inaccessible repo (`PrivateRepo`).
pub const ERR_PRIVATE_REPO: i32 = 5;
/// In-process memory budget exceeded (`MemoryPressure`).
pub const ERR_OOM: i32 = 6;
/// Host/API failure (`Api` / other transport mapping).
pub const ERR_API: i32 = 7;
/// Catch-all (`InvalidRepo`, `NotAFile`, `Other`, …).
pub const ERR_OTHER: i32 = 8;

struct Session {
    snapshot: MemorySnapshot<FetchGitHubClient>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());

fn set_error(msg: impl Into<String>) {
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = msg.into();
    }
}

fn clear_error() {
    if let Ok(mut slot) = LAST_ERROR.lock() {
        slot.clear();
    }
}

pub(crate) fn error_code(err: &SnapshotError) -> i32 {
    match err {
        SnapshotError::RateLimited(_) => ERR_RATE_LIMIT,
        SnapshotError::OversizedTree(_) | SnapshotError::OversizedBlob(_) => ERR_OVERSIZED,
        SnapshotError::MissingPath(_) => ERR_NOT_FOUND,
        SnapshotError::BinaryFile(_) => ERR_BINARY,
        SnapshotError::PrivateRepo(_) => ERR_PRIVATE_REPO,
        SnapshotError::MemoryPressure(_) => ERR_OOM,
        SnapshotError::Api { .. } => ERR_API,
        SnapshotError::InvalidRepo(_) | SnapshotError::NotAFile(_) | SnapshotError::Other(_) => {
            ERR_OTHER
        }
    }
}

fn map_err(err: SnapshotError) -> i32 {
    let code = error_code(&err);
    set_error(err.to_string());
    code
}

/// Allocate `size` bytes in guest linear memory (host uses this for HTTP bodies).
#[unsafe(no_mangle)]
pub extern "C" fn wit_snapshot_alloc(size: usize) -> *mut u8 {
    let size = size.max(1);
    let layout = match std::alloc::Layout::from_size_align(size, 1) {
        Ok(layout) => layout,
        Err(_) => return std::ptr::null_mut(),
    };
    unsafe { std::alloc::alloc(layout) }
}

/// Free a buffer previously returned by [`wit_snapshot_alloc`] (or export outs).
#[unsafe(no_mangle)]
pub extern "C" fn wit_snapshot_dealloc(ptr: *mut u8, size: usize) {
    if ptr.is_null() {
        return;
    }
    let size = size.max(1);
    let Ok(layout) = std::alloc::Layout::from_size_align(size, 1) else {
        return;
    };
    unsafe { std::alloc::dealloc(ptr, layout) };
}

/// Copy UTF-8 detail for the last failure into `buf` (truncated to `buf_len`).
/// Returns the full message length in bytes (may be `> buf_len`).
#[unsafe(no_mangle)]
pub extern "C" fn wit_snapshot_last_error(buf: *mut u8, buf_len: usize) -> usize {
    let msg = LAST_ERROR.lock().map(|g| g.clone()).unwrap_or_default();
    let bytes = msg.as_bytes();
    if !buf.is_null() && buf_len > 0 {
        let n = bytes.len().min(buf_len);
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n);
        }
    }
    bytes.len()
}

fn read_guest_str(ptr: *const u8, len: usize) -> SnapshotResult<String> {
    if len == 0 {
        return Ok(String::new());
    }
    if ptr.is_null() {
        return Err(SnapshotError::Other("null string pointer".into()));
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf8(slice.to_vec())
        .map_err(|_| SnapshotError::Other("argument was not valid UTF-8".into()))
}

fn write_guest_string(s: &str, out_ptr: *mut *mut u8, out_len: *mut usize) -> SnapshotResult<()> {
    if out_ptr.is_null() || out_len.is_null() {
        return Err(SnapshotError::Other("null output pointer".into()));
    }
    let bytes = s.as_bytes();
    let buf = wit_snapshot_alloc(bytes.len());
    if buf.is_null() {
        return Err(SnapshotError::MemoryPressure(
            "failed to allocate export buffer".into(),
        ));
    }
    unsafe {
        if !bytes.is_empty() {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
        }
        *out_ptr = buf;
        *out_len = bytes.len();
    }
    Ok(())
}

/// Open `owner/repo` (optional branch) into the process-local memory snapshot.
///
/// `branch_len == 0` means default branch. On success the snapshot is retained
/// for subsequent [`wit_snapshot_list`] / [`wit_snapshot_read`] calls.
#[unsafe(no_mangle)]
pub extern "C" fn wit_snapshot_open(
    repo_ptr: *const u8,
    repo_len: usize,
    branch_ptr: *const u8,
    branch_len: usize,
) -> i32 {
    clear_error();
    let result = (|| -> SnapshotResult<()> {
        let repo = read_guest_str(repo_ptr, repo_len)?;
        let branch = read_guest_str(branch_ptr, branch_len)?;
        let branch = if branch.is_empty() {
            None
        } else {
            Some(branch)
        };
        let backend = MemoryBackend::new(
            FetchGitHubClient::from_host(),
            MemoryBackendLimits::default(),
        );
        let snapshot = block_on_ready(backend.open(&repo, branch.as_deref()))?;
        let mut guard = SESSION
            .lock()
            .map_err(|_| SnapshotError::Other("session lock poisoned".into()))?;
        *guard = Some(Session { snapshot });
        Ok(())
    })();
    match result {
        Ok(()) => ERR_OK,
        Err(err) => map_err(err),
    }
}

/// List immediate children of `path` (empty path = repo root). Writes JSON to
/// an alloc'd buffer (`out_ptr` / `out_len`); caller must
/// [`wit_snapshot_dealloc`].
#[unsafe(no_mangle)]
pub extern "C" fn wit_snapshot_list(
    path_ptr: *const u8,
    path_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    clear_error();
    let result = (|| -> SnapshotResult<()> {
        let path = read_guest_str(path_ptr, path_len)?;
        let path = normalize_repo_path(&path);
        let guard = SESSION
            .lock()
            .map_err(|_| SnapshotError::Other("session lock poisoned".into()))?;
        let session = guard
            .as_ref()
            .ok_or_else(|| SnapshotError::Other("no open snapshot; call open first".into()))?;
        let entries = session.snapshot.list(if path.is_empty() {
            None
        } else {
            Some(path.as_str())
        })?;
        let json = serde_json::to_string(&entries)
            .map_err(|err| SnapshotError::Other(format!("encode list json: {err}")))?;
        write_guest_string(&json, out_ptr, out_len)
    })();
    match result {
        Ok(()) => ERR_OK,
        Err(err) => map_err(err),
    }
}

/// Read a UTF-8 file. Writes JSON [`crate::FileContent`] to an alloc'd buffer.
#[unsafe(no_mangle)]
pub extern "C" fn wit_snapshot_read(
    path_ptr: *const u8,
    path_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    clear_error();
    let result = (|| -> SnapshotResult<()> {
        let path = read_guest_str(path_ptr, path_len)?;
        let guard = SESSION
            .lock()
            .map_err(|_| SnapshotError::Other("session lock poisoned".into()))?;
        let session = guard
            .as_ref()
            .ok_or_else(|| SnapshotError::Other("no open snapshot; call open first".into()))?;
        let file = block_on_ready(session.snapshot.read(&path))?;
        let json = serde_json::to_string(&file)
            .map_err(|err| SnapshotError::Other(format!("encode read json: {err}")))?;
        write_guest_string(&json, out_ptr, out_len)
    })();
    match result {
        Ok(()) => ERR_OK,
        Err(err) => map_err(err),
    }
}

/// Thin `get_json` wrap for `GET /search/repositories?q=`.
///
/// Not a [`SnapshotBackend`] method, not code search, not a second HTTP stack.
/// HTTP 403/429 map to [`ERR_RATE_LIMIT`]. Writes the GitHub JSON body.
#[unsafe(no_mangle)]
pub extern "C" fn wit_snapshot_search_repositories(
    query_ptr: *const u8,
    query_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    clear_error();
    let result = (|| -> SnapshotResult<()> {
        let query = read_guest_str(query_ptr, query_len)?;
        let query = query.trim();
        if query.is_empty() {
            return Err(SnapshotError::Other(
                "github search query cannot be empty".into(),
            ));
        }
        let path = search_repositories_path(query);
        let client = FetchGitHubClient::from_host();
        let (status, body) = block_on_ready(client.get_json(&path))?;
        if status == 403 || status == 429 {
            let detail = if body.trim().is_empty() {
                format!("HTTP {status}")
            } else {
                body.chars().take(240).collect()
            };
            return Err(SnapshotError::RateLimited(detail));
        }
        if status != 200 {
            return Err(SnapshotError::Api {
                status,
                message: if body.trim().is_empty() {
                    format!("HTTP {status}")
                } else {
                    body.chars().take(240).collect()
                },
            });
        }
        let _: serde_json::Value = serde_json::from_str(&body)
            .map_err(|err| SnapshotError::Other(format!("search response was not JSON: {err}")))?;
        write_guest_string(&body, out_ptr, out_len)
    })();
    match result {
        Ok(()) => ERR_OK,
        Err(err) => map_err(err),
    }
}

/// Drop the retained snapshot (optional; next `open` replaces it).
#[unsafe(no_mangle)]
pub extern "C" fn wit_snapshot_close() -> i32 {
    clear_error();
    match SESSION.lock() {
        Ok(mut guard) => {
            *guard = None;
            ERR_OK
        }
        Err(_) => {
            set_error("session lock poisoned");
            ERR_OTHER
        }
    }
}

/// Poll an immediately-ready future (host `http_get` is synchronous).
fn block_on_ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => {
                panic!(
                    "wit-snapshot wasm open/read expected ready futures; host http_get must be synchronous"
                );
            }
        }
    }
}

fn noop_waker() -> Waker {
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), VTABLE)
    }
    fn wake(_: *const ()) {}
    fn wake_by_ref(_: *const ()) {}
    fn drop(_: *const ()) {}
    static VTABLE: &RawWakerVTable = &RawWakerVTable::new(clone, wake, wake_by_ref, drop);
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), VTABLE)) }
}
