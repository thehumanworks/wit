//! GitHub repository-search URL used by the wasm `get_json` wrap.
//!
//! This is a path helper only — not a [`crate::SnapshotBackend`] method and
//! not GitHub code search.

/// Same default as native `wit search` (`DEFAULT_GITHUB_REPO_LIMIT`).
pub const DEFAULT_SEARCH_PER_PAGE: u8 = 10;

/// `/search/repositories?q=...&sort=stars&order=desc&per_page=10`
pub fn search_repositories_path(query: &str) -> String {
    format!(
        "/search/repositories?q={}&sort=stars&order=desc&per_page={DEFAULT_SEARCH_PER_PAGE}",
        form_encode(query)
    )
}

/// application/x-www-form-urlencoded (space as `+`, hex uppercase).
pub fn form_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_ratatui_name_query() {
        assert_eq!(form_encode("ratatui in:name"), "ratatui+in%3Aname");
        assert_eq!(
            search_repositories_path("ratatui in:name"),
            "/search/repositories?q=ratatui+in%3Aname&sort=stars&order=desc&per_page=10"
        );
    }
}
