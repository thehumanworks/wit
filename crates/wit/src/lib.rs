pub mod gitops;
pub mod search;
pub mod search_run;
pub mod sed;
mod tls;

pub use tls::ensure_rustls_provider;

#[cfg(test)]
mod tests {
    #[test]
    fn cli_force_cache_invalidation_source_contract() {
        let cli = include_str!("cli.rs");
        assert!(
            cli.contains("long = \"refresh-cache\""),
            "repo-scoped read commands should expose --refresh-cache"
        );
        assert!(
            cli.contains("CacheAcquisitionMode::ForceInvalidate"),
            "--refresh-cache should route to force invalidation"
        );
        assert!(
            cli.contains("CacheAcquisitionMode::ServeStaleAndRevalidate"),
            "normal reads should keep stale-while-revalidate mode"
        );
        assert!(
            !cli.contains("long = \"branch\""),
            "public branch reads are intentionally deferred"
        );
    }

    #[test]
    fn cli_cache_help_text_source_and_docs_contract() {
        let cli = include_str!("cli.rs");
        assert!(
            cli.contains("branch-keyed stale-while-revalidate cache"),
            "CLI help should describe the default cache freshness mode"
        );
        assert!(
            cli.contains("--refresh-cache"),
            "CLI help should describe force cache invalidation"
        );
        assert!(
            cli.contains("No public TTL/max-age or branch-selection option is exposed."),
            "CLI help should avoid promising TTL or branch selection flags"
        );
        assert!(
            !cli.contains("long = \"branch\""),
            "public branch reads are intentionally deferred"
        );

        for (name, text) in [
            ("README.md", include_str!("../../../README.md")),
            (
                "crates/wit/src/skill/SKILL.md",
                include_str!("skill/SKILL.md"),
            ),
        ] {
            assert!(
                text.contains("stale-while-revalidate"),
                "{name} should document normal cached reads"
            );
            assert!(
                text.contains("`--refresh-cache`"),
                "{name} should document force cache invalidation"
            );
            assert!(
                text.contains("No public `--max-age` or TTL option"),
                "{name} should document the absence of TTL controls"
            );
            assert!(
                text.contains("Public branch selection is not exposed"),
                "{name} should document that branch-targeted reads are deferred"
            );
        }
    }
}
