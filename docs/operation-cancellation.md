# Operation cancellation

`WitOperations` receives an `OperationContext` for every typed operation. The context owns an
optional monotonic deadline and a clonable cancellation token. Dependency results are checked at
publication boundaries, so a result that races with cancellation is discarded with exactly one of
these adapter-stable messages:

- `operation cancelled`
- `operation deadline exceeded`

Every dynamic MCP tool route bridges rmcp's per-request cancellation token into this context and
applies a 120-second server-side deadline. The bridge is transport-independent; the stdio
integration test sends a real request-ID-scoped `notifications/cancelled` message and verifies that
blocked Git work terminates while the server remains usable.

GitHub HTTP requests race the response future against both boundaries. Cache-lock waits poll the
same context. Cache clones and snapshot repositories are created under temporary paths and are
only renamed or entered into the snapshot registry after a final context check; cancellation drops
the temporary directory. Stale-cache revalidation inherits the parent token and is additionally
bounded to 30 seconds.

## Child-process termination

Git and its helpers run in a separate process group:

- Linux and macOS use `std::os::unix::process::CommandExt::process_group(0)`, then signal the
  negative process-group ID with `SIGTERM`. After a 250 ms grace period, any remaining group is
  sent `SIGKILL`. The grace-period probe checks the process group rather than the root process, so
  a descendant that ignores `SIGTERM` is killed even when its parent exits first.
- Windows uses `CREATE_NEW_PROCESS_GROUP`, then the operating-system `taskkill /PID <pid> /T /F`
  facility to terminate the root and descendants. A direct `Child::kill` is retained as fallback.

The cancellation CI matrix runs process and adapter-context tests on Linux, macOS, and Windows. It
also runs the real stdio cancellation test on Linux and macOS. Unix has an additional regression
fixture whose root exits while a descendant ignores `SIGTERM`; Windows exercises the documented
`taskkill` path natively in CI, though that path cannot be run on a macOS development host.
