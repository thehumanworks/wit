use serde::{Deserialize, Serialize};

/// Provenance carried with every open snapshot (matches disk/MCP identity shape).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotProvenance {
    pub repo: String,
    pub requested_ref: String,
    pub resolved_ref: String,
    pub commit_sha: String,
    pub tree_sha: String,
    /// `memory` or `disk`.
    pub backend: String,
    /// Cache freshness label; memory always reports `fresh` (fetched for this open).
    pub cache_state: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Dir,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub kind: EntryKind,
    pub path: String,
    pub blob_sha: Option<String>,
    pub size_bytes: Option<u64>,
    pub is_binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    pub kind: EntryKind,
    pub blob_sha: Option<String>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeView {
    pub root: String,
    pub entries: Vec<TreeEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContent {
    pub path: String,
    pub blob_sha: String,
    pub size_bytes: u64,
    pub text: String,
    pub is_binary: bool,
}
