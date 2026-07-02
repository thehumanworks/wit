pub mod gitops;
pub mod mcp;
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
            cli.contains("long = \"branch\""),
            "repo-scoped cache/read commands should expose --branch"
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
            cli.contains("cache_branch_selection(branch)"),
            "--branch should route to cache branch selection"
        );
    }

    #[test]
    fn cli_branch_flag_parses_and_routes() {
        let cli = include_str!("cli.rs");
        assert!(
            cli.matches("long = \"branch\"").count() >= 8,
            "cache and repo-reading commands should expose --branch"
        );
        assert!(
            cli.matches("branch: Option<String>").count() >= 8,
            "branch values should be parsed into command variants"
        );
        assert!(
            cli.matches("cache_branch_selection(branch)").count() >= 8,
            "branch values should be routed to cache acquisition"
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
            cli.contains("--branch BRANCH"),
            "CLI help should describe named branch selection"
        );
        assert!(
            cli.contains("No public TTL/max-age option is exposed."),
            "CLI help should avoid promising TTL controls"
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
                text.contains("`--branch BRANCH`"),
                "{name} should document public branch-targeted reads"
            );
        }
    }

    #[test]
    fn cli_branch_help_text() {
        let cli = include_str!("cli.rs");
        assert!(cli.contains("--branch BRANCH"));
        assert!(cli.contains("repository default"));
        assert!(cli.contains("--refresh-cache"));
        assert!(cli.contains("No public TTL/max-age option is exposed."));

        for (name, text) in [
            ("README.md", include_str!("../../../README.md")),
            (
                "crates/wit/src/skill/SKILL.md",
                include_str!("skill/SKILL.md"),
            ),
        ] {
            assert!(
                text.contains("`--branch BRANCH`"),
                "{name} should document --branch"
            );
            assert!(
                text.contains("repository default branch"),
                "{name} should document default branch behavior"
            );
            assert!(
                text.contains("`--refresh-cache`"),
                "{name} should document selected-branch refresh"
            );
            assert!(
                text.contains("No public `--max-age` or TTL option"),
                "{name} should preserve the no TTL/max-age contract"
            );
        }
    }
}
