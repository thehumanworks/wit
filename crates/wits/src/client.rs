use reqwest::{Client, Url, header};
use scraper::{Html, Selector};
use std::collections::HashMap;

use crate::types::{CodeLine, GrepSearchResult, Hit, ParsedSnippet, RepoMatch};

pub const BASE_API_ENDPOINT: &str = "https://grep.app/api/search2";

pub struct GrepClient {
    client: Client,
    base_url: String,
}

impl Default for GrepClient {
    fn default() -> Self {
        Self::new()
    }
}

impl GrepClient {
    pub fn new() -> Self {
        Self::with_base_url(BASE_API_ENDPOINT)
    }

    /// Create a client pointing at a custom base URL (useful for testing).
    pub fn with_base_url(base_url: &str) -> Self {
        let client = Client::builder()
            .default_headers({
                let mut headers = header::HeaderMap::new();
                headers.insert(header::ACCEPT, "application/json".parse().unwrap());
                headers
            })
            .build()
            .expect("Failed to build HTTP client");
        Self {
            client,
            base_url: base_url.to_string(),
        }
    }

    pub fn parse_snippet(&self, hit: &Hit) -> ParsedSnippet {
        let html = Html::parse_fragment(&hit.content.snippet);
        let row_selector = Selector::parse("tr[data-line]").unwrap();
        let code_selector = Selector::parse(".highlight pre").unwrap();
        let jump_selector = Selector::parse(".jump").unwrap();

        let mut lines = Vec::new();
        let mut prev_line_num: Option<u32> = None;

        for row in html.select(&row_selector) {
            let line_num: u32 = row
                .value()
                .attr("data-line")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            // Check for jump (non-contiguous lines)
            let is_jump = if let Some(prev) = prev_line_num {
                line_num > prev + 1
            } else {
                false
            };

            // Check if this row has a jump div (end of contiguous section)
            let has_jump_div = row.select(&jump_selector).next().is_some();

            // Get code content - extract text, preserving structure
            let code_content = if let Some(pre) = row.select(&code_selector).next() {
                // Get inner HTML to check for marks
                let inner_html = pre.inner_html();
                let has_match = inner_html.contains("<mark>");

                // Extract text content
                let text: String = pre.text().collect();

                (text, has_match)
            } else {
                (String::new(), false)
            };

            if is_jump && prev_line_num.is_some() {
                lines.push(CodeLine {
                    line_number: 0,
                    content: String::new(),
                    has_match: false,
                    is_jump: true,
                });
            }

            lines.push(CodeLine {
                line_number: line_num,
                content: code_content.0,
                has_match: code_content.1,
                is_jump: false,
            });

            prev_line_num = Some(line_num);

            if has_jump_div {
                // The jump will be detected by the line number gap
            }
        }

        ParsedSnippet {
            path: hit.path.clone(),
            lines,
            total_matches: hit.total_matches.parse().unwrap_or(0),
        }
    }

    /// Returns repos matching the pattern with their match counts
    pub async fn repo_search(
        &self,
        repo_pattern: &str,
        lang_pattern: Option<&str>,
        regex: bool,
        query: &str,
        with_snippets: bool,
    ) -> anyhow::Result<Vec<RepoMatch>> {
        let mut url = Url::parse(&self.base_url)?;

        let mut pairs = vec![
            ("f.repo.pattern", repo_pattern),
            ("regexp", if regex { "true" } else { "false" }),
            ("q", query),
        ];

        if let Some(lang_pattern) = lang_pattern {
            pairs.push(("f.lang.pattern", lang_pattern));
        }

        url.query_pairs_mut().extend_pairs(pairs);

        let result = self
            .client
            .get(url)
            .send()
            .await?
            .json::<GrepSearchResult>()
            .await?;

        // Group hits by repo
        let mut repo_hits: HashMap<String, Vec<ParsedSnippet>> = HashMap::new();

        if with_snippets {
            for hit in &result.hits.hits {
                let snippet = self.parse_snippet(hit);
                repo_hits.entry(hit.repo.clone()).or_default().push(snippet);
            }
        }

        let mut repos: Vec<RepoMatch> = result
            .facets
            .repo
            .buckets
            .into_iter()
            .map(|b| {
                let files = repo_hits.remove(&b.val).unwrap_or_default();
                RepoMatch {
                    name: b.val,
                    hits: b.count,
                    files,
                }
            })
            .collect();

        repos.sort_by_key(|r| std::cmp::Reverse(r.hits));
        Ok(repos)
    }
}
