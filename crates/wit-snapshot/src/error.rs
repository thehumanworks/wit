use thiserror::Error;

pub type SnapshotResult<T> = Result<T, SnapshotError>;

/// Failures the memory (and disk adapter) snapshot path can surface.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    #[error("invalid repository: {0}")]
    InvalidRepo(String),

    #[error("path not found: {0}")]
    MissingPath(String),

    #[error("path is a directory, not a file: {0}")]
    NotAFile(String),

    #[error("file is binary (contains NUL bytes): {0}")]
    BinaryFile(String),

    #[error("repository appears private or inaccessible: {0}")]
    PrivateRepo(String),

    #[error("GitHub API rate limit exceeded: {0}")]
    RateLimited(String),

    #[error("repository tree exceeds size limits: {0}")]
    OversizedTree(String),

    #[error("blob exceeds size limits: {0}")]
    OversizedBlob(String),

    #[error("memory budget exceeded: {0}")]
    MemoryPressure(String),

    #[error("GitHub API error ({status}): {message}")]
    Api { status: u16, message: String },

    #[error("{0}")]
    Other(String),
}

impl SnapshotError {
    pub fn from_status(status: u16, body: &str, repo: &str) -> Self {
        let trimmed = body.trim();
        let message = if trimmed.is_empty() {
            format!("HTTP {status}")
        } else {
            trimmed.chars().take(240).collect::<String>()
        };
        match status {
            401 | 403 if message.to_ascii_lowercase().contains("rate limit") => {
                Self::RateLimited(message)
            }
            403 if message.to_ascii_lowercase().contains("api rate limit") => {
                Self::RateLimited(message)
            }
            429 => Self::RateLimited(message),
            401 | 403 | 404 => Self::PrivateRepo(format!(
                "{repo} (HTTP {status}); public repos only on the memory path"
            )),
            _ => Self::Api {
                status,
                message: format!("{repo}: {message}"),
            },
        }
    }
}
