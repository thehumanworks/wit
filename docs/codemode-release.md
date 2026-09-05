# Native Code Mode release contract

Code Mode is compiled into the existing `wit` and `wit-mcp` executables. Both
entrypoints default to the direct eight-tool MCP surface and accept `--mode code`
for the one-tool Code Mode surface. Each Code Mode invocation starts a hidden child
mode of the same executable; it does not install or launch a third public binary or
require Node.js, npm, Wrangler, Cloudflare, or another JavaScript runtime.

Release archives contain exactly `wit` and `wit-mcp` (`.exe` on Windows). The npm
package installs launchers for those same two native files. `install.sh`,
`npm/targets.json`, the release matrix, and package assertions use the same six
artifact names.

## Mandatory target evidence

The release workflow builds and packages these targets without an optional or
continue-on-error path:

- Linux musl x64 and ARM64
- macOS x64 and ARM64
- Windows MSVC x64 and ARM64

The workflow runs the direct/Code Mode MCP smoke test on native hosted x64 Linux,
ARM64 macOS, and x64 Windows runners. The npm post-publish matrix repeats that
tools/list check on Linux, macOS, and Windows. Cross-compilation is build/package
evidence only; it is not native runtime evidence for Linux ARM64, Intel macOS, or
Windows ARM64.

As of 2026-07-18, the workflow and mandatory matrix are encoded locally but have
not been executed remotely for this revision. A release must not treat this
document or workflow presence as a green six-target result.

## Dependency, license, and security ownership

The native runtime pins `rquickjs`, `rquickjs-core`, and `rquickjs-sys` 0.12.1;
those crates and the vendored QuickJS-NG engine are MIT licensed. The required
notices are in `THIRD-PARTY-LICENSES.md`, attached beside GitHub release archives,
and included in the npm package.

The release workflow emits `wit-sbom.spdx.json` from the locked source tree. Wit
maintainers own reviewing that SBOM and `Cargo.lock` for each release. They also
own monitoring RustSec and upstream rquickjs/QuickJS-NG security notices. An engine
update is security-sensitive: review the Rust wrapper and vendored engine revision,
then rerun containment tests, all six mandatory builds, native runtime smoke tests,
and package assertions before release.

## Local size and build observation

On an ARM64 macOS checkout on 2026-07-18, the pre-integration release artifacts were
22,475,568 bytes for `wit` and 19,469,616 bytes for `wit-mcp`. With native Code Mode
linked, they were 24,654,688 bytes and 22,297,584 bytes respectively: increases of
2,179,120 bytes (9.7%) and 2,827,968 bytes (14.5%).

The observed integration build was `cargo build --locked --release -p wit --bins`:
Cargo reported 52.65 seconds (52.69 seconds wall). That build recompiled much of the
dependency graph and is not a clean before/after benchmark. The earlier warm-cache
QuickJS worker-only spike reported 10.63 seconds. Treat both as local observations;
the six release jobs are the authoritative per-target build-time evidence.
