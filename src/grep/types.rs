use serde::{Deserialize, Serialize};

/// Top-level grep search response
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GrepSearchResult {
    #[serde(default)]
    pub time: u64,
    #[serde(default)]
    pub facets: Facets,
    #[serde(default)]
    pub hits: Hits,
}

/// Faceted search results for filtering
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Facets {
    #[serde(default)]
    pub path: FacetGroup<PathBucket>,
    #[serde(default)]
    pub repo: FacetGroup<RepoBucket>,
    #[serde(default)]
    pub lang: FacetGroup<LangBucket>,
}

/// Generic facet group containing buckets
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FacetGroup<T> {
    #[serde(default)]
    pub buckets: Vec<T>,
}

/// Path facet bucket
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PathBucket {
    #[serde(default)]
    pub val: String,
    #[serde(default)]
    pub count: u64,
}

/// Repository facet bucket
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepoBucket {
    #[serde(default)]
    pub val: String,
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub owner_id: String,
}

/// Language facet bucket
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LangBucket {
    #[serde(default)]
    pub val: String,
    #[serde(default)]
    pub count: u64,
}

/// Search hits container
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Hits {
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub hits: Vec<Hit>,
}

/// Individual search hit
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Hit {
    #[serde(default)]
    pub owner_id: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub content: HitContent,
    #[serde(default)]
    pub total_matches: String,
}

/// Content snippet from a search hit
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HitContent {
    #[serde(default)]
    pub snippet: String,
}

/// A repository with its match count
#[derive(Debug, Clone)]
pub struct RepoMatch {
    pub name: String,
    pub hits: u64,
}

impl std::fmt::Display for RepoMatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name, self.hits)
    }
}