---
status: proposed
date: 2026-07-18
decision-makers: wit maintainers
consulted: GitHub issue 15
---

# Child-process QuickJS feasibility

## Decision: NO-GO pending release-matrix evidence

Do not approve `rquickjs` for production Code Mode yet. The macOS ARM64 spike is a
positive API and containment result, but the mandatory six-target release build has not
run in this checkout and three advertised architectures lack native GitHub-hosted runtime
coverage. The dedicated `QuickJS feasibility gate` workflow is mandatory: a failed build
or native containment test keeps this decision at NO-GO and requires an architecture
reassessment issue or a superseding ADR. It must never cause a fallback to in-process
execution of untrusted JavaScript.

If every release-build job passes and the native validation plan below is completed, the
decision may be superseded with a conditional GO for a **child-process** worker only. A
child process is the crash and cancellation boundary; QuickJS must not be linked into the
long-lived MCP parent.

Issue #17 may proceed with experimental, non-shipping integration against this bounded
protocol so the shared operation registry and cancellation contracts can be exercised.
It may not enable Code Mode in production artifacts or describe the engine as supported
until this ADR is superseded after the mandatory build and native-runtime gates.

## Context

Issue #15 asks whether asynchronous, model-written JavaScript can be executed with
QuickJS on all six release targets without giving JavaScript ambient host capabilities or
letting engine failure terminate the MCP parent. The production Code Mode tool and real
Git/GitHub operations are deliberately outside this spike.

The tested dependency is `rquickjs` 0.12.1 with its `futures` feature, which embeds
QuickJS-NG through `rquickjs-sys` 0.12.1. The local toolchain was Rust 1.95.0 on
`aarch64-apple-darwin`; the crate's declared MSRV is Rust 1.87. `rquickjs` 0.12.1 does
not include pre-generated bindings for `aarch64-pc-windows-msvc`, so that target enables
the crate's `bindgen` feature and makes a working Clang/libclang installation part of the
Windows ARM64 build gate.

## Spike design and observed behavior

`wit-quickjs-spike-worker` is a separate executable. The parent starts a fresh worker for
each invocation with a cleared environment, a new empty working directory, piped stdin and
stdout, null stderr, and kill-on-drop. It sends newline-delimited JSON frames with a hard
64 KiB frame limit and a 32 KiB script limit. Both sides validate invocation and call IDs.
The parent permits at most four concurrent mock host operations and sixteen total calls.

The worker uses `AsyncRuntime`, exposes one JSON-only async `hostCall`, and resumes its
JavaScript promise after a real parent round trip. The final value crosses IPC only after
`JSON.stringify`. QuickJS memory, stack, and interrupt-handler limits are set before eval;
the parent also enforces an independent wall-clock timeout and kills the child. A
deterministic injected exit code 86 exercises hard worker disappearance and restart
without involving platform crash reporters.

The native macOS ARM64 containment test demonstrated:

- sequential calls and eight promise-concurrent calls through the four-call bound;
- final JSON result serialization;
- infinite-loop interruption, recursive stack exhaustion, and memory-pressure failure;
- malformed request and oversized-script rejection;
- parent task cancellation, hard worker exit, timeout, and successful next invocation;
- `process`, `require`, `fetch`, `WebSocket`, `Deno`, `Bun`, QuickJS `std`, and QuickJS
  `os` are absent, and dynamic module import is rejected.

This capability check proves the JavaScript global surface, not an OS sandbox. A native
engine exploit would still run with the worker's OS identity and could make syscalls. A
production design therefore still needs issue #18's platform sandboxing, privilege and
handle reduction, and resource budgets. No untrusted QuickJS bytecode may be accepted:
QuickJS-NG's security policy explicitly excludes adversarial bytecode from its threat
model.

## Build time and size

On the local ARM64 macOS host, `cargo build --locked --release -p
wit-quickjs-spike` completed in 10.63 seconds of Cargo-reported time (10.67 seconds wall)
with the dependency cache state present in this checkout. The standalone worker was
2,447,248 bytes (Mach-O ARM64). These are reproducible observations, not clean-build or
cross-target benchmarks.

The existing `wit` and `wit-mcp` binaries do not link `rquickjs`, so this spike adds zero
bytes to those two files. It does add QuickJS compilation to workspace release builds and
produces a third, currently unshipped spike executable. The eventual packaging design and
its two-binary size impact must be measured by the integration issue; this ADR does not
approve shipping a third public executable.

## License and security-update implications

`rquickjs` 0.12.1 and its vendored QuickJS-NG engine are MIT licensed. Distribution must
retain both MIT notices. The engine is C/C++ code behind unsafe FFI, so Rust memory-safety
guarantees do not contain engine defects. Version updates require reviewing both the Rust
wrapper changelog and the vendored QuickJS-NG revision, rerunning adversarial containment
tests, all six release builds, and native runtime checks. Dependabot/RustSec checks are
useful but cannot replace upstream QuickJS-NG security/advisory monitoring because an
engine fix may arrive through a new vendored revision rather than a Rust advisory.

Version 0.12.x contains fixes directly relevant to this decision, including interrupt
handler/GC behavior, promise polling after uncatchable errors, stack-limit clamping, an FFI
ABI layout vulnerability, and a runtime-free assertion leak. Production adoption must pin
the lockfile and treat a dependency update as security-sensitive rather than accepting an
untested semver-compatible engine update.

## Release and runtime evidence matrix

The workflow builds the exact release worker for every advertised target without
`continue-on-error`. A green job is required evidence; workflow presence alone is not a
pass.

| Advertised target | Release build gate | Native runtime evidence |
|---|---|---|
| Linux x64 musl | `cross build --locked --release` on Ubuntu | Run the release containment suite as x64 Linux; also execute the musl artifact in an x64 musl container before superseding this ADR. |
| Linux ARM64 musl | `cross build --locked --release` on Ubuntu | No native hosted runner. Run the release suite and built worker on an ARM64 Linux musl host (or release-candidate ARM64 device); emulation is build smoke only. |
| macOS ARM64 | `cargo build --locked --release` on macOS 14 | The workflow runs the release containment suite natively; local ARM64 evidence is recorded above. |
| macOS x64 | cross-build with Cargo on macOS 14 | Run the release suite and worker on Intel macOS hardware before superseding this ADR. Rosetta is supplementary, not native proof. |
| Windows x64 MSVC | `cargo build --locked --release` on Windows | The workflow runs the release containment suite natively on `windows-latest`. |
| Windows ARM64 MSVC | Cargo cross-build on Windows with bindgen | No native hosted runner. Run the release suite and worker on Windows ARM64 hardware; confirm process termination and handle inheritance with native diagnostics. |

## Consequences and promotion gates

- The proof remains isolated and has no user-facing CLI or MCP mode.
- Bounded JSON IPC and one-worker-per-invocation are the reference boundary for later work;
  the production protocol may evolve but may not become unbounded.
- The parent must remain authoritative for host-call concurrency, deadlines, cancellation,
  final-result size, worker replacement, and privileged operations.
- The worker may expose only an allowlisted typed host registry. It may not load modules,
  inherit environment variables, or add filesystem, network, or subprocess globals.
- Any mandatory release build, native runtime check, or security containment failure leaves
  the result NO-GO. The follow-up must document another engine/process strategy rather than
  silently linking QuickJS into the parent.

## Evidence commands

```text
cargo test --locked --release -p wit-quickjs-spike --test containment
cargo build --locked --release -p wit-quickjs-spike
bash scripts/check_quickjs_spike.sh
```

Cross-target results must come from `.github/workflows/quickjs-spike.yml`; do not infer them
from the local macOS build.
