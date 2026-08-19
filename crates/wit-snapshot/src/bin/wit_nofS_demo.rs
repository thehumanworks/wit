//! No-filesystem demo for the in-memory snapshot backend.
//!
//! Live mode talks to the GitHub API and never writes under WIT_CACHE_DIR.
//! Fixture mode reads tree/blob JSON from files and exercises the same memory
//! index used by `--backend memory` (wasmtime-friendly once fixtures are local).
//!
//! Full `wit` CLI WASM is out of scope: `gix` bare clones, `fs2` locks, and the
//! disk cache still require a filesystem. This binary is the honest no-FS slice.

use clap::{Parser, Subcommand, ValueEnum};
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use wit_snapshot::{
    DirEntry, EntryKind, GitHubHttpClient, MemoryBackend, MemoryBackendLimits, RepoSnapshot,
    SnapshotBackend, SnapshotError, SnapshotProvenance, SnapshotResult, snapshot_from_tree_json,
};

#[derive(Debug, Parser)]
#[command(
    name = "wit-nofS-demo",
    about = "Demonstrate wit memory snapshots with zero disk-cache writes"
)]
struct DemoCli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Open a public repo via GitHub HTTP and run tree/ls/cat equivalents.
    Live {
        /// Repository in owner/repo form
        #[arg(short = 'r', long = "repo")]
        repo: String,
        #[arg(long = "branch")]
        branch: Option<String>,
        /// Optional path for ls/tree/cat (cat requires a file path)
        path: Option<String>,
        #[arg(long = "op", value_enum, default_value_t = DemoOp::All)]
        op: DemoOp,
        /// Fail if WIT_CACHE_DIR (or the default probe dir) gains any files
        #[arg(long = "assert-no-cache-writes", default_value_t = true)]
        assert_no_cache_writes: bool,
        /// Directory monitored for accidental cache writes
        #[arg(long = "cache-probe-dir")]
        cache_probe_dir: Option<PathBuf>,
    },
    /// Run list/read against file fixtures (no GitHub network).
    Fixture {
        #[arg(long = "tree-json")]
        tree_json: PathBuf,
        #[arg(long = "blob-json")]
        blob_json: Option<PathBuf>,
        #[arg(long = "repo", default_value = "demo/repo")]
        repo: String,
        #[arg(long = "ref", default_value = "main")]
        git_ref: String,
        #[arg(long = "commit", default_value = "deadbeef")]
        commit: String,
        #[arg(long = "tree-sha", default_value = "treesha")]
        tree_sha: String,
        #[arg(long = "read-path")]
        read_path: Option<String>,
        #[arg(long = "list-path")]
        list_path: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
enum DemoOp {
    Tree,
    Ls,
    Cat,
    #[default]
    All,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    install_rustls_provider();
    let cli = DemoCli::parse();
    match cli.command {
        Commands::Live {
            repo,
            branch,
            path,
            op,
            assert_no_cache_writes,
            cache_probe_dir,
        } => {
            let probe = prepare_cache_probe(cache_probe_dir)?;
            let before = snapshot_dir(&probe)?;
            let backend = MemoryBackend::from_env()?;
            let snap = backend.open(&repo, branch.as_deref()).await?;
            print_provenance(snap.provenance());
            run_ops(&snap, op, path.as_deref()).await?;
            if assert_no_cache_writes {
                let after = snapshot_dir(&probe)?;
                if after != before {
                    return Err(format!(
                        "memory backend wrote under cache probe dir {}: before={before:?} after={after:?}",
                        probe.display()
                    )
                    .into());
                }
                println!("cache_probe: ok (no writes under {})", probe.display());
            }
        }
        Commands::Fixture {
            tree_json,
            blob_json,
            repo,
            git_ref,
            commit,
            tree_sha,
            read_path,
            list_path,
        } => {
            let tree = fs::read_to_string(&tree_json)?;
            let blob = match blob_json {
                Some(path) => Some(fs::read_to_string(path)?),
                None => None,
            };
            let client = FixtureClient { blob_json: blob };
            let snap = snapshot_from_tree_json(
                Arc::new(client),
                &repo,
                &git_ref,
                &commit,
                &tree_sha,
                &tree,
                MemoryBackendLimits::default(),
            )?;
            print_provenance(snap.provenance());
            let entries = snap.list(list_path.as_deref())?;
            print_ls(&entries);
            let tree_view = snap.tree(list_path.as_deref())?;
            println!("tree_root: {}", tree_view.root);
            for entry in &tree_view.entries {
                println!("{}", entry.path);
            }
            if let Some(path) = read_path {
                let file = snap.read(&path).await?;
                print!("{}", file.text);
                if !file.text.ends_with('\n') {
                    println!();
                }
            }
        }
    }
    Ok(())
}

fn install_rustls_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
}

fn prepare_cache_probe(explicit: Option<PathBuf>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = if let Some(path) = explicit {
        fs::create_dir_all(&path)?;
        path
    } else if let Ok(path) = env::var("WIT_CACHE_DIR") {
        let path = PathBuf::from(path);
        fs::create_dir_all(&path)?;
        path
    } else {
        let path = env::temp_dir().join(format!("wit-nofS-probe-{}", std::process::id()));
        fs::create_dir_all(&path)?;
        // Point WIT_CACHE_DIR at the probe so any accidental cache use is visible.
        // Safety: single-threaded demo main before other work.
        unsafe {
            env::set_var("WIT_CACHE_DIR", &path);
        }
        path
    };
    Ok(dir)
}

fn snapshot_dir(path: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut entries = Vec::new();
    if !path.exists() {
        return Ok(entries);
    }
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                out.push(format!("{rel}/"));
                walk(base, &path, out)?;
            } else {
                out.push(rel);
            }
        }
        Ok(())
    }
    walk(path, path, &mut entries)?;
    entries.sort();
    Ok(entries)
}

fn print_provenance(p: &SnapshotProvenance) {
    println!(
        "provenance: backend={} repo={} ref={} commit={} tree={} cache={}",
        p.backend, p.repo, p.resolved_ref, p.commit_sha, p.tree_sha, p.cache_state
    );
}

async fn run_ops<S: RepoSnapshot>(snap: &S, op: DemoOp, path: Option<&str>) -> SnapshotResult<()> {
    match op {
        DemoOp::Ls => {
            let entries = snap.list(path)?;
            print_ls(&entries);
        }
        DemoOp::Tree => {
            let tree = snap.tree(path)?;
            println!("{}", tree.root);
            for entry in tree.entries {
                println!("  {}", entry.path);
            }
        }
        DemoOp::Cat => {
            let path = path.ok_or_else(|| {
                SnapshotError::Other("cat requires a file path argument".to_string())
            })?;
            let file = snap.read(path).await?;
            print!("{}", file.text);
            if !file.text.ends_with('\n') {
                println!();
            }
        }
        DemoOp::All => {
            println!("== tree ==");
            let tree = snap.tree(None)?;
            for entry in &tree.entries {
                println!("{}", entry.path);
            }
            println!("== ls ==");
            print_ls(&snap.list(None)?);
            let cat_path = path.unwrap_or("README");
            println!("== cat {cat_path} ==");
            match snap.read(cat_path).await {
                Ok(file) => {
                    let preview: String = file.text.chars().take(400).collect();
                    println!("{preview}");
                }
                Err(SnapshotError::MissingPath(_)) => match snap.read("README.md").await {
                    Ok(file) => {
                        let preview: String = file.text.chars().take(400).collect();
                        println!("{preview}");
                    }
                    Err(SnapshotError::MissingPath(_)) => {
                        println!("(no README/README.md; skipped)");
                    }
                    Err(err) => return Err(err),
                },
                Err(err) => return Err(err),
            }
        }
    }
    Ok(())
}

fn print_ls(entries: &[DirEntry]) {
    for entry in entries {
        match entry.kind {
            EntryKind::Dir => println!("{}/", entry.name),
            EntryKind::File => println!("{}", entry.name),
        }
    }
}

struct FixtureClient {
    blob_json: Option<String>,
}

impl GitHubHttpClient for FixtureClient {
    async fn get_json(&self, path: &str) -> SnapshotResult<(u16, String)> {
        if path.contains("/git/blobs/") {
            if let Some(body) = &self.blob_json {
                return Ok((200, body.clone()));
            }
            return Err(SnapshotError::Other(
                "fixture blob JSON not provided".to_string(),
            ));
        }
        Err(SnapshotError::Other(format!(
            "fixture client has no handler for {path}"
        )))
    }
}
