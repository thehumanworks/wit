pub mod codemode;
pub mod codemode_policy;
pub mod gitops;
pub mod mcp;
pub mod operation_context;
pub mod operation_registry;
pub mod operations;
pub mod search;
pub mod search_run;
pub mod sed;
pub mod snapshot;
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

    #[test]
    fn cli_branches_command_parses() {
        let cli = include_str!("cli.rs");
        assert!(
            cli.contains("name = \"branches\""),
            "CLI should expose a public branches subcommand"
        );
        assert!(
            cli.contains("Commands::Branches { repo }"),
            "branches handler should route the repo argument"
        );
        assert!(
            cli.contains("list_remote_branches(&repo)"),
            "branches should use reusable branch metadata collection"
        );
        assert!(
            cli.contains("override_usage = \"wit branches -r <REPO>\""),
            "branches help should require -r/--repo"
        );
        assert!(
            cli.contains("branches should not have a short alias"),
            "parser tests should assert no short alias was added"
        );
        assert!(
            cli.contains("cat --branch should still parse"),
            "parser tests should preserve existing --branch reads"
        );
    }

    #[test]
    fn cli_branches_help_text() {
        let cli = include_str!("cli.rs");
        assert!(
            cli.contains("wit branches -r <REPO>"),
            "CLI help should show branches usage"
        );
        assert!(
            cli.contains("ahead/behind"),
            "CLI help should mention ahead/behind metadata"
        );
        assert!(
            cli.contains("merged"),
            "CLI help should mention merged metadata"
        );
        assert!(
            cli.contains("first commit unique to the branch"),
            "CLI help should document created-time inference"
        );
        assert!(
            cli.contains("branch tip commit time"),
            "CLI help should document created-time fallback"
        );

        for (name, text) in [
            ("README.md", include_str!("../../../README.md")),
            (
                "crates/wit/src/skill/SKILL.md",
                include_str!("skill/SKILL.md"),
            ),
        ] {
            assert!(
                text.contains("`wit branches -r"),
                "{name} should document the branches command"
            );
            assert!(
                text.contains("ahead") && text.contains("behind"),
                "{name} should document ahead/behind columns"
            );
            assert!(
                text.contains("graph-merged") || text.contains("merged"),
                "{name} should document merged semantics"
            );
            assert!(
                text.contains("first unique commit"),
                "{name} should document created-time inference"
            );
            assert!(
                text.contains("tip commit fallback"),
                "{name} should document created-time fallback"
            );
            assert!(
                text.contains("`--branch BRANCH`"),
                "{name} should preserve existing branch-read docs"
            );
        }
    }

    #[test]
    fn code_mode_readme_and_skill_contract_stays_synchronized() {
        for (name, text) in [
            ("README.md", include_str!("../../../README.md")),
            (
                "crates/wit/src/skill/SKILL.md",
                include_str!("skill/SKILL.md"),
            ),
        ] {
            for required in [
                "direct mode",
                "Code Mode",
                "experimental",
                "codemode.wit.help()",
                "codemode.wit.open",
                "format: \"paths\"",
                "path_prefix",
                "snapshot_id",
                "next_cursor",
                "JSON-serializable",
                "no filesystem",
                "Rust parent",
                "cancel",
                "killed and reaped",
                "not persisted or logged",
                "fail-closed recommendation",
            ] {
                assert!(
                    text.to_ascii_lowercase()
                        .contains(&required.to_ascii_lowercase()),
                    "{name} omits {required}"
                );
            }
            assert!(!text.to_ascii_lowercase().contains("compat-v1"));
            assert!(!text.to_ascii_lowercase().contains("mcp v1"));
        }
    }
}
