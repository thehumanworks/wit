# Code Mode security boundary

Code Mode is selected explicitly with `--mode code`; direct mode remains the default.
Model-written JavaScript and all worker output are untrusted. The MCP parent remains the only process that owns GitHub
credentials, the repository cache, snapshots, and operation implementations. A fresh child runs
one invocation through a hidden mode of the same installed executable, with an empty environment
and an isolated temporary working directory. It receives
only bounded stdin/stdout IPC and the seven registered `codemode.wit` methods. QuickJS has no
filesystem, network, environment, module loader, process, subprocess, cache, token, or generic host
call API. There is no permissive fallback: an unknown operation, malformed frame, unavailable
capacity, or invalid policy fails the invocation before privileged dispatch.

The trusted parent enforces authorization, page/snapshot accounting, per-call and cumulative data
movement, final framing, and server fairness. The worker complements those controls with QuickJS
memory and stack limits plus an interrupt deadline. The parent also has a slightly longer kill and
reap deadline, so a wedged or crashed worker is discarded and the next invocation starts a new
process. Host-operation cancellation/deadline semantics remain owned by `OperationContext`; these
budgets do not bypass or replace them.

## Budgets

All values are bytes unless noted. Defaults are fixed server policy, never invocation-controlled.
The absolute maximum is a code invariant checked by `CodeModePolicy::validate` and
`wit_quickjs_spike::Limits::validate`.

| Resource | Default | Absolute maximum | Enforcer / stable error |
|---|---:|---:|---|
| JavaScript source | 32 KiB | 32 KiB | parent and worker / `source_bytes_limit` |
| IPC frame | 72 KiB | 72 KiB | both endpoints / `worker_protocol_error` |
| JavaScript wall/interrupt time | 10 s | 10 s | QuickJS interrupt plus parent kill / `deadline_exceeded` |
| QuickJS heap | 16 MiB | 64 MiB | worker VM / `code_rejected` or worker exit |
| QuickJS stack | 256 KiB | 1 MiB | worker VM / `code_rejected` or worker exit |
| Host calls | 16 | 16 | worker and parent / `host_calls_limit` |
| Concurrent host calls per invocation | 4 | 4 | worker guard plus parent semaphore / `host_concurrency_limit` |
| Page-producing operations | 8 | 16 | parent before dispatch / `pages_limit` |
| Snapshot opens | 2 | 4 | parent before dispatch / `snapshots_limit` |
| One host result | 64 KiB | 64 KiB | parent before IPC / `host_result_bytes_limit` |
| Cumulative host results | 256 KiB | 256 KiB | parent before IPC / `cumulative_host_bytes_limit` |
| Final result | 48 KiB | 48 KiB | worker and parent / `final_result_bytes_limit` |
| Captured worker diagnostics | 8 KiB | 8 KiB | parent drain; content is never reflected or logged |
| Simultaneous workers | 4 | 16 | parent / `server_workers_limit` |
| Simultaneous invocations | 4 | 16 | parent / `server_invocations_limit` |
| Server host operations | 8 | 32 | parent semaphore / `server_host_operations_limit` |

Page and snapshot units are reserved before dispatch and remain consumed after a failed operation.
This keeps repeated pagination, fan-out, and capability probing bounded. Host-result byte accounting
uses serialized JSON bytes; a result which exceeds a byte budget becomes a catchable JavaScript
error and its value never enters the worker. Final JSON is rejected atomically, never truncated.

Worker stderr is continuously drained so pipe backpressure cannot wedge the parent. Only capped
byte counts and a truncation bit are retained; diagnostic content is never copied into MCP results,
errors, tracing, or debug statistics. MCP protocol output remains stdout-only. Parent credentials,
environment values, cache paths, operation handles, and raw IPC bridge functions are neither sent
to the child nor exposed through reflection. The direct API's `OpenResponse.cache.last_error` is
replaced with `null` at the Code Mode boundary because successful cache diagnostics may contain
absolute paths or token-shaped backend text.

## Stable error shape

Code-tool policy failures use structured content:

```json
{ "code": "pages_limit", "message": "page budget exhausted" }
```

Allowed-operation failures and parent budget failures inside a script are catchable `Error` objects
with enumerable `code` and `operation` fields and a stable, redacted message. Protocol failures use
`worker_protocol_error`; worker crashes use `worker_exited`; cancellation and deadlines preserve
the existing `cancelled` and `deadline_exceeded` codes. Successful result frames carry an explicit
value-presence bit so valid JSON `null` remains distinct from a malformed missing value; missing or
contradictory result fields are always `worker_protocol_error`, never `worker_start_failed`.

The adversarial suite covers ambient capability and raw-bridge probing, host-call fan-out, repeated
page/snapshot use, per-call and cumulative result bytes, cyclic and oversized output, malformed IPC,
infinite execution, heap/stack pressure, worker crash/restart, capped diagnostics, and secret
redaction. See `crates/wit/src/codemode_policy.rs`, `crates/wit/src/codemode.rs`, and
`crates/wit-quickjs-spike/tests/containment.rs`.
