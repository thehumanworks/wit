//! Keeps `wit --help`, `wit --version`, and the docs that restate them in sync with the binary.
//!
//! * Every command's `--help` (found by walking the `Commands:` sections, so a new subcommand
//!   needs a snapshot) must match `tests/snapshots/help/<command path>.txt`. After an intended
//!   help change, regenerate them with
//!   `WIT_UPDATE_HELP_SNAPSHOTS=1 cargo test -p wit --test cli_help` and review the diff.
//! * `wit --version` / `-V` (and `wit-mcp --version`) print `CARGO_PKG_VERSION`, the
//!   `crates/wit` version the auto-tag workflow bumps; the release and npm workflows check the
//!   built binaries against the tag and the npm version.
//! * The README command table and the AGENTS.md subcommand table list exactly the commands
//!   (and aliases) of `wit --help`, and the README restates the cloud cache claims that
//!   `formal/Wit/CacheSource.lean` proves against the code.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const UPDATE_ENV: &str = "WIT_UPDATE_HELP_SNAPSHOTS";
const README: &str = include_str!("../../../README.md");
const AGENTS: &str = include_str!("../../../AGENTS.md");

fn run(binary: &str, args: &[&str]) -> String {
    let output = Command::new(binary)
        .args(args)
        .env("NO_COLOR", "1")
        .env_remove("CLICOLOR_FORCE")
        .output()
        .unwrap_or_else(|err| panic!("failed to run {binary} {args:?}: {err}"));
    assert!(
        output.status.success(),
        "{binary} {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("help output is UTF-8")
        .replace("\r\n", "\n")
}

fn wit(args: &[&str]) -> String {
    run(env!("CARGO_BIN_EXE_wit"), args)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Listed {
    name: String,
    aliases: Vec<String>,
    about: String,
}

/// Entries of the `Commands:` section of a clap help page, without `help`.
fn listed_commands(help: &str) -> Vec<Listed> {
    let Some(section) = help.split("\nCommands:\n").nth(1) else {
        return Vec::new();
    };
    section
        .lines()
        .take_while(|line| line.starts_with("  "))
        .filter_map(|line| {
            let line = line.trim();
            let (name, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let rest = rest.trim();
            let (about, aliases) = match rest.rsplit_once(" [alias") {
                Some((about, tail)) => {
                    let list = tail
                        .trim_start_matches("es:")
                        .trim_start_matches(':')
                        .trim_end_matches(']');
                    (
                        about.trim(),
                        list.split(',').map(|a| a.trim().to_string()).collect(),
                    )
                }
                None => (rest, Vec::new()),
            };
            (name != "help").then(|| Listed {
                name: name.to_string(),
                aliases,
                about: about.to_string(),
            })
        })
        .collect()
}

/// Every command path reachable from `wit --help`, including `[]` for the root.
fn command_paths() -> Vec<Vec<String>> {
    let mut paths = vec![Vec::new()];
    let mut index = 0;
    while index < paths.len() {
        let path = paths[index].clone();
        let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
        args.push("--help");
        for listed in listed_commands(&wit(&args)) {
            let mut child = path.clone();
            child.push(listed.name);
            paths.push(child);
        }
        index += 1;
    }
    paths
}

fn snapshot_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/help")
}

fn snapshot_name(path: &[String]) -> String {
    std::iter::once("wit")
        .chain(path.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("-")
        + ".txt"
}

#[test]
fn help_output_matches_snapshots() {
    let dir = snapshot_dir();
    let update = std::env::var_os(UPDATE_ENV).is_some();
    if update {
        std::fs::create_dir_all(&dir).unwrap();
    }
    let mut expected_files = BTreeSet::new();
    let mut failures = Vec::new();
    for path in command_paths() {
        let name = snapshot_name(&path);
        let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
        args.push("--help");
        let actual = wit(&args);
        let file = dir.join(&name);
        if update {
            std::fs::write(&file, &actual).unwrap();
        } else {
            match std::fs::read_to_string(&file) {
                Ok(stored) if stored.replace("\r\n", "\n") == actual => {}
                Ok(stored) => failures.push(format!(
                    "{name}: `wit {}` differs from the snapshot\n{}",
                    args.join(" "),
                    first_difference(&stored, &actual)
                )),
                Err(_) => {
                    failures.push(format!("{name}: no snapshot for `wit {}`", args.join(" ")))
                }
            }
        }
        expected_files.insert(name);
    }
    let stale: Vec<_> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .filter(|name| name.ends_with(".txt") && !expected_files.contains(name))
                .collect()
        })
        .unwrap_or_default();
    for name in stale {
        if update {
            std::fs::remove_file(dir.join(&name)).unwrap();
        } else {
            failures.push(format!(
                "{name}: snapshot for a command that no longer exists"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "help drifted from tests/snapshots/help:\n\n{}\n\nIf the change is intended, run `{UPDATE_ENV}=1 cargo test -p wit --test cli_help`, review the diff, and update README.md / AGENTS.md (and the ROOT_AFTER_HELP claims in scripts/gen_formal_constants.mjs) to match.",
        failures.join("\n\n")
    );
}

fn first_difference(stored: &str, actual: &str) -> String {
    let (stored, actual): (Vec<_>, Vec<_>) = (stored.lines().collect(), actual.lines().collect());
    for i in 0..stored.len().max(actual.len()) {
        let (s, a) = (stored.get(i), actual.get(i));
        if s != a {
            return format!(
                "  line {}:\n  - snapshot: {}\n  + actual:   {}",
                i + 1,
                s.unwrap_or(&"<end>"),
                a.unwrap_or(&"<end>")
            );
        }
    }
    String::new()
}

#[test]
fn version_is_the_crate_version() {
    let expected = format!("wit {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(wit(&["--version"]), expected);
    assert_eq!(wit(&["-V"]), expected);
    assert_eq!(
        run(env!("CARGO_BIN_EXE_wit-mcp"), &["--version"]),
        format!("wit-mcp {}\n", env!("CARGO_PKG_VERSION")),
        "wit and wit-mcp ship in one archive and must report the same version"
    );
    assert!(
        wit(&["--help"]).lines().any(|line| {
            line.trim_start().starts_with("-V, --version") && line.ends_with(" Print version")
        }),
        "wit --help must list -V/--version"
    );
}

/// Rows `| `name` | `alias` | about |` of the first table after `heading`.
fn doc_table(doc: &str, heading: &str) -> Vec<Vec<String>> {
    let after = doc
        .split_once(heading)
        .unwrap_or_else(|| panic!("missing heading {heading:?}"))
        .1;
    after
        .lines()
        .skip_while(|line| !line.starts_with('|'))
        .take_while(|line| line.starts_with('|'))
        .skip(2)
        .map(|row| {
            row.trim()
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().trim_matches('`').to_string())
                .collect()
        })
        .collect()
}

fn root_commands() -> Vec<Listed> {
    let mut commands = listed_commands(&wit(&["--help"]));
    commands.sort();
    commands
}

#[test]
fn readme_command_table_matches_help() {
    let mut rows: Vec<Listed> = doc_table(README, "\n## Commands\n")
        .into_iter()
        .map(|cells| Listed {
            name: cells[0].clone(),
            aliases: (!cells[1].is_empty())
                .then(|| cells[1].clone())
                .into_iter()
                .collect(),
            about: cells[2].clone(),
        })
        .collect();
    rows.sort();
    assert_eq!(
        rows,
        root_commands(),
        "README.md `## Commands` table must list every `wit --help` command with its alias and about text"
    );
}

#[test]
fn agents_command_table_matches_help() {
    let mut rows: Vec<(String, Vec<String>)> = doc_table(AGENTS, "\n## CLI Subcommands\n")
        .into_iter()
        .map(|cells| {
            let aliases = (!cells[1].is_empty()).then(|| cells[1].clone());
            (cells[0].clone(), aliases.into_iter().collect())
        })
        .collect();
    rows.sort();
    let expected: Vec<_> = root_commands()
        .into_iter()
        .map(|listed| (listed.name, listed.aliases))
        .collect();
    assert_eq!(
        rows, expected,
        "AGENTS.md `## CLI Subcommands` table must list every `wit --help` command with its alias"
    );
}

/// The root help names the commands that take `--backend`; check that list against the
/// commands whose help (or whose subcommands' help) actually offers the flag.
#[test]
fn backend_claim_lists_the_commands_with_backend() {
    let root = wit(&["--help"]);
    let claimed: BTreeSet<String> = root
        .split_once("repo commands (")
        .and_then(|(_, rest)| rest.split_once(')'))
        .expect("root help lists the repo commands")
        .0
        .split(", ")
        .map(str::to_string)
        .collect();
    let with_backend: BTreeSet<String> = command_paths()
        .into_iter()
        .filter(|path| !path.is_empty())
        .filter(|path| {
            let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
            args.push("--help");
            wit(&args).contains("--backend <disk|memory>")
        })
        .map(|path| path[0].clone())
        .collect();
    assert_eq!(claimed, with_backend);
}

/// The README restates the cloud cache sentence of `wit --help`, which the Lean proofs check
/// against `crates/wit/src/gitops/cloud.rs`.
#[test]
fn readme_restates_cloud_cache_claims() {
    let root = wit(&["--help"]);
    let between = |start: &str, end: char| -> &str {
        let from = root
            .split_once(start)
            .unwrap_or_else(|| panic!("root help lost {start:?}"))
            .1;
        from.split_once(end).map_or(from, |(s, _)| s)
    };
    let readme_row = |var: &str| {
        README
            .lines()
            .find(|line| line.starts_with(&format!("| `{var}` |")))
            .unwrap_or_else(|| panic!("README cloud cache table has no {var} row"))
    };

    let (first, rest) = between("Disable it with WIT_CACHE_URL=", ')')
        .split_once(" (also: ")
        .expect("disable sentence");
    let url_row = readme_row("WIT_CACHE_URL");
    for value in std::iter::once(first).chain(rest.trim_end_matches("; any case").split(", ")) {
        let spelled = if value == "empty" {
            value.to_string()
        } else {
            format!("`{value}`")
        };
        assert!(
            url_row.contains(&spelled),
            "README WIT_CACHE_URL row must list the disable value {spelled}: {url_row}"
        );
    }
    assert!(url_row.contains("any case"), "{url_row}");

    let release_url = between("Release builds default WIT_CACHE_URL to ", ';');
    assert!(README.contains(&format!(
        "Release builds use the hosted instance (`{release_url}`"
    )));
    assert!(root.contains("debug builds (cargo run, cargo test) default to off"));
    assert!(README.contains("debug builds leave it off"));

    for var in ["WIT_CACHE_TIMEOUT_MS", "WIT_CACHE_MAX_BYTES"] {
        let default = between(&format!("{var} (default "), ')');
        assert!(
            readme_row(var).contains(&format!("| `{default}`")),
            "README {var} default must be {default}"
        );
    }

    assert!(root.contains(
        "Disk read order: local cache, then shared cloud pack cache, then GitHub clone."
    ));
    assert!(README.contains("local cache, then the shared cloud pack cache, then a GitHub clone"));
}
