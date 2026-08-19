//! Native wasmtime host: load `wit_snapshot` wasm32 module with fixture-backed
//! `wit_snapshot_host.http_get`, then exercise `open` / `list` / `read`.
//!
//! This is CI evidence that the wasm module runs. It is **not** a browser-ready
//! product (see `demo/browser` and `docs/adr/0004-wasm-fetch-howto.md`).

use anyhow::{Context, Result, bail};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use wasmtime::{Caller, Engine, Linker, Module, Store};

const ERR_OK: i32 = 0;

fn cassette_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cassettes")
}

fn default_wasm_path() -> PathBuf {
    // Prefer release-like target dir used by the check script; fall back to debug.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let release = workspace.join("target/wasm32-unknown-unknown/release/wit_snapshot.wasm");
    let debug = workspace.join("target/wasm32-unknown-unknown/debug/wit_snapshot.wasm");
    if release.exists() { release } else { debug }
}

fn load_fixtures(dir: &Path) -> Result<HashMap<String, (u16, String)>> {
    let mut map = HashMap::new();
    let repo = std::fs::read_to_string(dir.join("demo_repo.json"))?;
    let commit = std::fs::read_to_string(dir.join("demo_commit.json"))?;
    let tree = std::fs::read_to_string(dir.join("demo_tree.json"))?;
    let blob = std::fs::read_to_string(dir.join("demo_blob.json"))?;
    let blob_main = std::fs::read_to_string(dir.join("demo_blob_main.json"))?;

    for key in ["/repos/demo/repo", "https://api.github.com/repos/demo/repo"] {
        map.insert(key.to_string(), (200, repo.clone()));
    }
    for key in [
        "/repos/demo/repo/commits/main",
        "https://api.github.com/repos/demo/repo/commits/main",
    ] {
        map.insert(key.to_string(), (200, commit.clone()));
    }
    for key in [
        "/repos/demo/repo/git/trees/treesha?recursive=1",
        "https://api.github.com/repos/demo/repo/git/trees/treesha?recursive=1",
    ] {
        map.insert(key.to_string(), (200, tree.clone()));
    }
    for key in [
        "/repos/demo/repo/git/blobs/blob-readme",
        "https://api.github.com/repos/demo/repo/git/blobs/blob-readme",
    ] {
        map.insert(key.to_string(), (200, blob.clone()));
    }
    for key in [
        "/repos/demo/repo/git/blobs/blob-main",
        "https://api.github.com/repos/demo/repo/git/blobs/blob-main",
    ] {
        map.insert(key.to_string(), (200, blob_main.clone()));
    }
    Ok(map)
}

struct HostState {
    fixtures: Arc<HashMap<String, (u16, String)>>,
}

fn main() -> Result<()> {
    let wasm_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_wasm_path);
    if !wasm_path.exists() {
        bail!(
            "wasm module not found at {}. Build with:\n  cargo build -p wit-snapshot --target wasm32-unknown-unknown --no-default-features",
            wasm_path.display()
        );
    }

    let fixtures = Arc::new(load_fixtures(&cassette_dir())?);
    let engine = Engine::default();
    let module = Module::from_file(&engine, &wasm_path)
        .with_context(|| format!("load {}", wasm_path.display()))?;
    let mut linker: Linker<HostState> = Linker::new(&engine);
    let mut store = Store::new(
        &engine,
        HostState {
            fixtures: Arc::clone(&fixtures),
        },
    );

    linker.func_wrap(
        "wit_snapshot_host",
        "wit_snapshot_host_http_get",
        |mut caller: Caller<'_, HostState>,
         path_ptr: u32,
         path_len: u32,
         status_out: u32,
         body_ptr_out: u32,
         body_len_out: u32|
         -> i32 {
            let memory = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(mem) => mem,
                None => return 1,
            };
            let path = {
                let data = memory.data(&caller);
                let start = path_ptr as usize;
                let end = start.saturating_add(path_len as usize);
                match data.get(start..end) {
                    Some(bytes) => String::from_utf8_lossy(bytes).into_owned(),
                    None => return 2,
                }
            };
            let Some((status, body)) = caller.data().fixtures.get(&path).cloned() else {
                eprintln!("fixture miss for path: {path}");
                return 3;
            };
            let alloc = match caller
                .get_export("wit_snapshot_alloc")
                .and_then(|e| e.into_func())
            {
                Some(f) => f,
                None => return 4,
            };
            let alloc = match alloc.typed::<u32, u32>(&caller) {
                Ok(f) => f,
                Err(_) => return 4,
            };
            let body_bytes = body.as_bytes();
            let body_ptr = match alloc.call(&mut caller, body_bytes.len() as u32) {
                Ok(ptr) if !body_bytes.is_empty() && ptr != 0 => ptr,
                Ok(ptr) if body_bytes.is_empty() => ptr,
                _ => return 5,
            };
            {
                let data = memory.data_mut(&mut caller);
                if !body_bytes.is_empty() {
                    let start = body_ptr as usize;
                    let end = start + body_bytes.len();
                    if let Some(slot) = data.get_mut(start..end) {
                        slot.copy_from_slice(body_bytes);
                    } else {
                        return 6;
                    }
                }
                // status_out: u16 little-endian
                let s = status_out as usize;
                if let Some(slot) = data.get_mut(s..s + 2) {
                    slot.copy_from_slice(&status.to_le_bytes());
                } else {
                    return 7;
                }
                // body_ptr_out: *mut u8 (wasm32 pointer)
                let bp = body_ptr_out as usize;
                if let Some(slot) = data.get_mut(bp..bp + 4) {
                    slot.copy_from_slice(&body_ptr.to_le_bytes());
                } else {
                    return 8;
                }
                let bl = body_len_out as usize;
                if let Some(slot) = data.get_mut(bl..bl + 4) {
                    slot.copy_from_slice(&(body_bytes.len() as u32).to_le_bytes());
                } else {
                    return 9;
                }
            }
            0
        },
    )?;

    let instance = linker.instantiate(&mut store, &module)?;
    let memory = instance
        .get_memory(&mut store, "memory")
        .context("module memory")?;
    let alloc = instance
        .get_typed_func::<u32, u32>(&mut store, "wit_snapshot_alloc")
        .context("wit_snapshot_alloc")?;
    let dealloc = instance
        .get_typed_func::<(u32, u32), ()>(&mut store, "wit_snapshot_dealloc")
        .context("wit_snapshot_dealloc")?;
    let open = instance
        .get_typed_func::<(u32, u32, u32, u32), i32>(&mut store, "wit_snapshot_open")
        .context("wit_snapshot_open")?;
    let list = instance
        .get_typed_func::<(u32, u32, u32, u32), i32>(&mut store, "wit_snapshot_list")
        .context("wit_snapshot_list")?;
    let read = instance
        .get_typed_func::<(u32, u32, u32, u32), i32>(&mut store, "wit_snapshot_read")
        .context("wit_snapshot_read")?;

    let repo = b"demo/repo";
    let repo_ptr = alloc.call(&mut store, repo.len() as u32)?;
    memory.data_mut(&mut store)[repo_ptr as usize..repo_ptr as usize + repo.len()]
        .copy_from_slice(repo);

    let rc = open.call(&mut store, (repo_ptr, repo.len() as u32, 0, 0))?;
    dealloc.call(&mut store, (repo_ptr, repo.len() as u32))?;
    if rc != ERR_OK {
        bail!("open failed with code {rc}");
    }
    println!("open: ok");

    // list root → write out ptr/len slots
    let out_ptr_slot = alloc.call(&mut store, 4)?;
    let out_len_slot = alloc.call(&mut store, 4)?;
    let rc = list.call(&mut store, (0, 0, out_ptr_slot, out_len_slot))?;
    if rc != ERR_OK {
        bail!("list failed with code {rc}");
    }
    let (list_ptr, list_len) = {
        let data = memory.data(&store);
        let ptr =
            u32::from_le_bytes(data[out_ptr_slot as usize..out_ptr_slot as usize + 4].try_into()?);
        let len =
            u32::from_le_bytes(data[out_len_slot as usize..out_len_slot as usize + 4].try_into()?);
        (ptr, len)
    };
    let list_json = {
        let data = memory.data(&store);
        String::from_utf8(data[list_ptr as usize..list_ptr as usize + list_len as usize].to_vec())?
    };
    dealloc.call(&mut store, (list_ptr, list_len))?;
    dealloc.call(&mut store, (out_ptr_slot, 4))?;
    dealloc.call(&mut store, (out_len_slot, 4))?;
    println!("list: {list_json}");
    if !list_json.contains("README.md") {
        bail!("list JSON missing README.md");
    }

    let path = b"README.md";
    let path_ptr = alloc.call(&mut store, path.len() as u32)?;
    memory.data_mut(&mut store)[path_ptr as usize..path_ptr as usize + path.len()]
        .copy_from_slice(path);
    let out_ptr_slot = alloc.call(&mut store, 4)?;
    let out_len_slot = alloc.call(&mut store, 4)?;
    let rc = read.call(
        &mut store,
        (path_ptr, path.len() as u32, out_ptr_slot, out_len_slot),
    )?;
    dealloc.call(&mut store, (path_ptr, path.len() as u32))?;
    if rc != ERR_OK {
        bail!("read failed with code {rc}");
    }
    let (read_ptr, read_len) = {
        let data = memory.data(&store);
        let ptr =
            u32::from_le_bytes(data[out_ptr_slot as usize..out_ptr_slot as usize + 4].try_into()?);
        let len =
            u32::from_le_bytes(data[out_len_slot as usize..out_len_slot as usize + 4].try_into()?);
        (ptr, len)
    };
    let read_json = {
        let data = memory.data(&store);
        String::from_utf8(data[read_ptr as usize..read_ptr as usize + read_len as usize].to_vec())?
    };
    dealloc.call(&mut store, (read_ptr, read_len))?;
    dealloc.call(&mut store, (out_ptr_slot, 4))?;
    dealloc.call(&mut store, (out_len_slot, 4))?;
    println!("read: {read_json}");
    if !read_json.contains("Hello, memory!") {
        bail!("read JSON missing expected blob text");
    }

    println!("wasmtime fixture smoke: ok");
    Ok(())
}
