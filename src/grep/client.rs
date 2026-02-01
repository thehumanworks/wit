use reqwest::{Client, Url, header};

use crate::grep::types::{GrepSearchResult, RepoMatch};

const BASE_API_ENDPOINT: &str = "https://grep.app/api/search";

pub struct GrepClient {
    client: Client,
}

impl GrepClient {
    pub fn new() -> Self {
        let client = Client::builder()
            .default_headers({
                let mut headers = header::HeaderMap::new();
                headers.insert(header::ACCEPT, "application/json".parse().unwrap());
                headers
            })
            .build()
            .expect("Failed to build HTTP client");
        Self { client }
    }

    /// Returns repos matching the pattern with their match counts
    pub async fn find_repos(&self, repo_pattern: &str, lang_pattern: Option<&str>) -> anyhow::Result<Vec<RepoMatch>> {
        let mut url = Url::parse(BASE_API_ENDPOINT)?;

        let mut pairs = vec![
            ("f.repo.pattern", repo_pattern),
            ("regexp", "true"),
            ("q", ".*"),
        ];
        if let Some(lang_pattern) = lang_pattern {
            pairs.push(("f.lang.pattern", lang_pattern));
        }
        
        url.query_pairs_mut().extend_pairs(pairs);

        println!("{}", url);
        let result = self.client.get(url).send().await?.json::<GrepSearchResult>().await?;
        let mut repos: Vec<RepoMatch> = result
            .facets
            .repo
            .buckets
            .into_iter()
            .map(|b| RepoMatch { name: b.val, hits: b.count })
            .collect();
        repos.sort_by_key(|r| r.hits);
        repos.reverse();
        Ok(repos)
    }
}

