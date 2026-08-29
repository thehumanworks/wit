use std::{
    collections::HashMap,
    io::Write,
    process,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use rquickjs::{AsyncContext, AsyncRuntime, Function, Promise, prelude::Async};
use serde::Serialize;
use serde_json::Value;
use tokio::{
    io,
    sync::{Mutex, Semaphore, oneshot},
};
use wit_quickjs_spike::{
    ExecuteRequest, MAX_FRAME_BYTES, MAX_RESULT_BYTES, MAX_SCRIPT_BYTES, PROTOCOL_VERSION,
    ParentMessage, TestAction, WorkerMessage, read_frame,
};

type Pending = Arc<Mutex<HashMap<u32, oneshot::Sender<String>>>>;

#[derive(Clone)]
struct HostRpc {
    invocation_id: u64,
    next_call_id: Arc<AtomicU32>,
    pending: Pending,
    stdout: Arc<StdMutex<std::io::Stdout>>,
    call_slots: Arc<Semaphore>,
    max_calls: u32,
}

impl HostRpc {
    async fn call(&self, operation: String, argument_json: String) -> rquickjs::Result<String> {
        let call_id = self.next_call_id.fetch_add(1, Ordering::SeqCst) + 1;
        if call_id > self.max_calls {
            return Ok(serde_json::json!({
                "ok": false,
                "error": {
                    "code": "host_calls_limit",
                    "operation": operation,
                    "message": "host-call budget exhausted",
                }
            })
            .to_string());
        }
        let _permit = self.call_slots.acquire().await.map_err(|_| {
            rquickjs::Error::new_from_js_message("hostCall", "Promise", "worker is shutting down")
        })?;
        let (sender, receiver) = oneshot::channel();
        if self.pending.lock().await.insert(call_id, sender).is_some() {
            return Err(rquickjs::Error::new_from_js_message(
                "hostCall",
                "Promise",
                "duplicate generated call id",
            ));
        }
        let message = WorkerMessage::HostCall {
            invocation_id: self.invocation_id,
            call_id,
            operation,
            argument_json,
        };
        {
            let mut stdout = self.stdout.lock().map_err(|_| {
                rquickjs::Error::new_from_js_message(
                    "hostCall",
                    "Promise",
                    "worker stdout lock poisoned",
                )
            })?;
            write_frame_sync(&mut *stdout, &message).map_err(|error| {
                rquickjs::Error::new_from_js_message("hostCall", "Promise", error.to_string())
            })?;
        }
        receiver.await.map_err(|_| {
            rquickjs::Error::new_from_js_message(
                "hostCall",
                "Promise",
                "parent response channel closed",
            )
        })
    }
}

#[tokio::main(flavor = "current_thread")]
#[allow(dead_code)]
async fn main() {
    run_worker_process().await
}

#[doc(hidden)]
pub async fn run_worker_process() -> ! {
    match run().await {
        Ok(()) => process::exit(0),
        Err(error) => {
            eprintln!("wit Code Mode worker failed: {error:#}");
            let message = WorkerMessage::Result {
                invocation_id: 0,
                ok: false,
                value_present: false,
                value: None,
                error: Some(error.to_string()),
            };
            let _ = write_frame_sync(&mut std::io::stdout(), &message);
            process::exit(2);
        }
    }
}

/// Touch a phase marker file in the worker's scratch cwd when spawn tracing
/// is enabled, so the parent can name a wedged startup's phase at deadline.
/// Markers are empty files with fixed names — no content crosses the boundary.
fn trace_marker(name: &str) {
    if std::env::var_os("WIT_CODEMODE_SPAWN_TRACE").is_some() {
        let _ = std::fs::write(name, b"");
    }
}

async fn run() -> Result<()> {
    trace_marker("worker-entered");
    let mut stdin = io::stdin();
    let request_frame = read_frame(&mut stdin)
        .await?
        .context("parent closed stdin before request")?;
    let request: ExecuteRequest =
        serde_json::from_slice(&request_frame).context("malformed execute request")?;
    validate_request(&request)?;
    trace_marker("worker-request-read");

    match request.test_action {
        TestAction::Crash => process::exit(86),
        TestAction::EmitZeroCallId => {
            emit_test_call(request.invocation_id, 0)?;
            std::future::pending::<()>().await;
        }
        TestAction::EmitDuplicateCallId => {
            emit_test_call(request.invocation_id, 1)?;
            emit_test_call(request.invocation_id, 1)?;
            std::future::pending::<()>().await;
        }
        TestAction::EmitUnknownCallId => {
            emit_test_call(request.invocation_id, 999)?;
            std::future::pending::<()>().await;
        }
        TestAction::EmitUnknownMessage => {
            let mut stdout = std::io::stdout();
            stdout.write_all(b"{\"type\":\"unknown_worker_message\"}\n")?;
            stdout.flush()?;
            std::future::pending::<()>().await;
        }
        TestAction::EmitDiagnostic => {
            eprintln!("wit Code Mode worker diagnostic framing probe");
        }
        TestAction::EmitSecretDiagnostic => {
            eprintln!(
                "{}",
                "GITHUB_TOKEN=should-never-cross-worker-boundary ".repeat(512)
            );
        }
        TestAction::ForgeOversizedSuccess => {
            write_frame_sync(
                &mut std::io::stdout(),
                &WorkerMessage::Result {
                    invocation_id: request.invocation_id,
                    ok: true,
                    value_present: true,
                    value: Some(Value::String("x".repeat(MAX_RESULT_BYTES))),
                    error: None,
                },
            )?;
            return Ok(());
        }
        TestAction::ForgeMissingSuccessValue => {
            write_frame_sync(
                &mut std::io::stdout(),
                &WorkerMessage::Result {
                    invocation_id: request.invocation_id,
                    ok: true,
                    value_present: false,
                    value: None,
                    error: None,
                },
            )?;
            return Ok(());
        }
        TestAction::ForgeContradictorySuccess => {
            write_frame_sync(
                &mut std::io::stdout(),
                &WorkerMessage::Result {
                    invocation_id: request.invocation_id,
                    ok: true,
                    value_present: true,
                    value: Some(Value::Bool(true)),
                    error: Some("must not accompany success".into()),
                },
            )?;
            return Ok(());
        }
        TestAction::ForgeContradictoryFailure => {
            write_frame_sync(
                &mut std::io::stdout(),
                &WorkerMessage::Result {
                    invocation_id: request.invocation_id,
                    ok: false,
                    value_present: true,
                    value: Some(Value::Bool(true)),
                    error: Some("failure must not carry a value".into()),
                },
            )?;
            return Ok(());
        }
        TestAction::Execute => {}
    }

    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let reader_pending = Arc::clone(&pending);
    let invocation_id = request.invocation_id;
    tokio::spawn(async move {
        loop {
            let frame = match read_frame(&mut stdin).await {
                Ok(Some(frame)) => frame,
                Ok(None) => {
                    fail_pending(&reader_pending, "parent closed IPC").await;
                    break;
                }
                Err(error) => {
                    fail_pending(&reader_pending, &format!("malformed parent frame: {error}"))
                        .await;
                    break;
                }
            };
            let message = match serde_json::from_slice::<ParentMessage>(&frame) {
                Ok(message) => message,
                Err(error) => {
                    fail_pending(&reader_pending, &format!("unknown parent message: {error}"))
                        .await;
                    break;
                }
            };
            let ParentMessage::HostResult {
                invocation_id: response_invocation_id,
                call_id,
                ok,
                value,
                error,
            } = message;
            if response_invocation_id != invocation_id || call_id == 0 {
                fail_pending(&reader_pending, "invalid invocation or zero call id").await;
                break;
            }
            if let Some(sender) = reader_pending.lock().await.remove(&call_id) {
                let result = if ok {
                    value
                        .map(|value| {
                            serde_json::to_string(&serde_json::json!({
                                "ok": true,
                                "value": value,
                            }))
                            .unwrap_or_else(|_| {
                                r#"{"ok":false,"error":{"code":"worker_protocol_error","operation":"","message":"host result could not be serialized"}}"#.into()
                            })
                        })
                        .unwrap_or_else(|| {
                            r#"{"ok":false,"error":{"code":"worker_protocol_error","operation":"","message":"host result omitted value"}}"#.into()
                        })
                } else {
                    serde_json::to_string(&serde_json::json!({
                        "ok": false,
                        "error": error.unwrap_or_else(|| wit_quickjs_spike::HostError {
                            code: "operation_failed".into(),
                            operation: String::new(),
                            message: "host operation failed".into(),
                        })
                    }))
                    .unwrap_or_else(|_| r#"{"ok":false,"error":{"code":"worker_protocol_error","operation":"","message":"host error could not be serialized"}}"#.into())
                };
                let _ = sender.send(result);
            } else {
                fail_pending(&reader_pending, "unknown or duplicate host-result call id").await;
                break;
            }
        }
    });

    let stdout = Arc::new(StdMutex::new(std::io::stdout()));
    let rpc = HostRpc {
        invocation_id,
        next_call_id: Arc::new(AtomicU32::new(0)),
        pending,
        stdout: Arc::clone(&stdout),
        call_slots: Arc::new(Semaphore::new(
            request.limits.max_concurrent_host_calls as usize,
        )),
        max_calls: request.limits.max_host_calls,
    };
    let result = execute_script(&request, rpc).await;
    let message = match result {
        Ok(value) => WorkerMessage::Result {
            invocation_id,
            ok: true,
            value_present: true,
            value: Some(value),
            error: None,
        },
        Err(error) => WorkerMessage::Result {
            invocation_id,
            ok: false,
            value_present: false,
            value: None,
            error: Some(error.to_string()),
        },
    };
    write_frame_sync(&mut *lock_stdout(&stdout)?, &message)
}

fn emit_test_call(invocation_id: u64, call_id: u32) -> Result<()> {
    write_frame_sync(
        &mut std::io::stdout(),
        &WorkerMessage::HostCall {
            invocation_id,
            call_id,
            operation: "__spike_echo".into(),
            argument_json: "null".into(),
        },
    )
}

fn lock_stdout(
    stdout: &StdMutex<std::io::Stdout>,
) -> Result<std::sync::MutexGuard<'_, std::io::Stdout>> {
    stdout
        .lock()
        .map_err(|_| anyhow!("worker stdout lock poisoned"))
}

fn write_frame_sync<T: Serialize>(writer: &mut impl Write, message: &T) -> Result<()> {
    let frame = serde_json::to_vec(message)?;
    if frame.len() > MAX_FRAME_BYTES {
        bail!("IPC frame exceeds {MAX_FRAME_BYTES}-byte limit");
    }
    writer.write_all(&frame)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

async fn fail_pending(pending: &Pending, message: &str) {
    let senders = pending
        .lock()
        .await
        .drain()
        .map(|(_, sender)| sender)
        .collect::<Vec<_>>();
    for sender in senders {
        let envelope = serde_json::json!({
            "ok": false,
            "error": {
                "code": "worker_protocol_error",
                "operation": "",
                "message": message,
            }
        })
        .to_string();
        let _ = sender.send(envelope);
    }
}

fn validate_request(request: &ExecuteRequest) -> Result<()> {
    if request.version != PROTOCOL_VERSION {
        bail!("unsupported protocol version {}", request.version);
    }
    if request.invocation_id == 0 {
        bail!("invocation_id must be non-zero");
    }
    if request.script.len() > MAX_SCRIPT_BYTES {
        bail!("script exceeds {MAX_SCRIPT_BYTES}-byte limit");
    }
    request.limits.validate()
}

async fn execute_script(request: &ExecuteRequest, rpc: HostRpc) -> Result<Value> {
    trace_marker("worker-js-start");
    let runtime = AsyncRuntime::new().context("create QuickJS runtime")?;
    runtime.set_memory_limit(request.limits.memory_bytes).await;
    runtime.set_max_stack_size(request.limits.stack_bytes).await;
    let deadline = Instant::now() + Duration::from_millis(request.limits.wall_time_ms);
    let interrupt_deadline = deadline;
    runtime
        .set_interrupt_handler(Some(Box::new(move || Instant::now() >= interrupt_deadline)))
        .await;
    let context = AsyncContext::full(&runtime)
        .await
        .context("create QuickJS context")?;
    let source = format!(
        "(async () => {{\ntry {{\nconst __result = await (async () => {{\n{}\n}})();\n__validateFinal(__result);\nreturn JSON.stringify({{ ok: true, value: __result }});\n}} catch (error) {{\nreturn JSON.stringify({{ ok: false, error: {{ code: error && error.code || 'code_rejected', operation: error && error.operation || '', message: error && error.message || String(error) }} }});\n}}\n}})()",
        request.script
    );

    context
        .async_with(async move |ctx| {
            let function = Function::new(
                ctx.clone(),
                Async(move |operation: String, argument_json: String| {
                    let rpc = rpc.clone();
                    async move { rpc.call(operation, argument_json).await }
                }),
            )?;
            ctx.globals().set("__hostCallJson", function)?;
            ctx.globals().set(
                "__maxConcurrentHostCalls",
                if request.expose_test_host_call {
                    i32::MAX as u32
                } else {
                    request.limits.max_concurrent_host_calls
                },
            )?;
            let prelude = if request.expose_test_host_call {
                CODEMODE_PRELUDE.replace(
                    "/* TEST_HOST_CALL */",
                    "globalThis.hostCall = value => __hostOperation(\"__spike_echo\", value);",
                )
            } else {
                CODEMODE_PRELUDE.replace("/* TEST_HOST_CALL */", "")
            };
            if let Err(error) = ctx.eval::<(), _>(prelude) {
                eprintln!(
                    "Code Mode prelude failed: {error}; caught={:?}",
                    ctx.catch()
                );
                return Err(error);
            }
            let promise: Promise = ctx.eval(source.as_str())?;
            let json = promise.into_future::<String>().await?;
            Ok::<String, rquickjs::Error>(json)
        })
        .await
        .map_err(|error| {
            if Instant::now() >= deadline {
                anyhow!("execution deadline exceeded")
            } else {
                anyhow!(error.to_string())
            }
        })
        .and_then(|json| decode_execution_envelope(&json, request.limits.max_result_bytes))
}

fn decode_execution_envelope(json: &str, max_result_bytes: usize) -> Result<Value> {
    let mut envelope: Value = serde_json::from_str(json).context("final result is not JSON")?;
    if envelope["ok"] == true {
        let value = envelope
            .get_mut("value")
            .map(Value::take)
            .context("successful execution omitted its value")?;
        let serialized_bytes = serde_json::to_vec(&value)?.len();
        if serialized_bytes > max_result_bytes {
            bail!(
                "final JSON is {serialized_bytes} bytes and exceeds the {max_result_bytes}-byte limit; return fewer fields or items, or use compact read/list formats"
            );
        }
        return Ok(value);
    }
    let code = envelope["error"]["code"]
        .as_str()
        .unwrap_or("code_rejected");
    let message = envelope["error"]["message"]
        .as_str()
        .unwrap_or("JavaScript execution failed");
    if code == "host_calls_limit" {
        bail!("host-call budget exhausted");
    }
    bail!("{message}")
}

const CODEMODE_PRELUDE: &str = r#"
(() => {
const __bridge = globalThis.__hostCallJson;
delete globalThis.__hostCallJson;
const __maxConcurrentHostCalls = globalThis.__maxConcurrentHostCalls;
delete globalThis.__maxConcurrentHostCalls;
let __activeHostCalls = 0;
const __hostOperation = async (operation, input) => {
  if (__activeHostCalls >= __maxConcurrentHostCalls) {
    const error = new Error("concurrent host-call budget exhausted");
    Object.defineProperties(error, {
      code: { value: "host_concurrency_limit", enumerable: true },
      operation: { value: operation, enumerable: true }
    });
    throw error;
  }
  __activeHostCalls += 1;
  try {
    const response = JSON.parse(await __bridge(operation, JSON.stringify(input)));
    if (!response.ok) {
      const error = new Error(response.error.message);
      Object.defineProperties(error, {
        code: { value: response.error.code, enumerable: true },
        operation: { value: response.error.operation, enumerable: true }
      });
      throw error;
    }
    return response.value;
  } finally {
    __activeHostCalls -= 1;
  }
};
/* TEST_HOST_CALL */
const __helpEntries = Object.freeze([
  Object.freeze({
    name: "findRepositories",
    signature: "findRepositories({ pattern?, lang?, query?, cursor?, max_items?, max_bytes? })",
    description: "Find GitHub repositories when owner/repo is unknown.",
    example: "await codemode.wit.findRepositories({ pattern: 'ratatuizilla', max_items: 5 })"
  }),
  Object.freeze({
    name: "refs",
    signature: "refs({ repo, ref?, cursor?, max_items?, max_bytes? })",
    description: "List or resolve repository branches, tags, and commits before open().",
    example: "await codemode.wit.refs({ repo: 'owner/repo', max_items: 20 })"
  }),
  Object.freeze({
    name: "open",
    signature: "open({ repo, ref?, freshness? })",
    description: "Open an immutable snapshot and reuse its snapshot_id.",
    example: "await codemode.wit.open({ repo: 'owner/repo' })"
  }),
  Object.freeze({
    name: "list",
    signature: "list({ snapshot_id, path?, depth?, format?: 'structured' | 'paths', cursor?, max_items?, max_bytes? })",
    description: "List bounded repository structure; format: 'paths' removes repeated entry metadata.",
    example: "await codemode.wit.list({ snapshot_id, path: 'src', depth: 2, format: 'paths' })"
  }),
  Object.freeze({
    name: "searchCode",
    signature: "searchCode({ snapshot_id, queries, path_prefix?, glob?, globs?, exclude?, context_lines?, cursor?, max_results?, max_bytes? })",
    description: "Regex-search one snapshot with optional include, prefix, and exclude path filters.",
    example: "await codemode.wit.searchCode({ snapshot_id, queries: ['pub struct'], path_prefix: 'src', exclude: ['**/tests/**'] })"
  }),
  Object.freeze({
    name: "read",
    signature: "read({ snapshot_id, path, start_line?, end_line?, format?: 'text' | 'lines' | 'structured', cursor?, max_lines?, max_bytes? })",
    description: "Read a line range. Code Mode defaults to compact text; use lines for numbered pairs or structured for per-line provenance.",
    example: "await codemode.wit.read({ snapshot_id, path: 'README.md', start_line: 1, end_line: 80 })"
  }),
  Object.freeze({
    name: "context",
    signature: "context({ snapshot_id, queries, globs?, context_lines?, cursor?, max_results?, max_bytes? })",
    description: "Gather deterministic ranked multi-file evidence.",
    example: "await codemode.wit.context({ snapshot_id, queries: ['backend'], max_results: 10 })"
  })
]);
const __methodNames = __helpEntries.map(entry => entry.name).concat(["help"]);
const __levenshtein = (left, right) => {
  const row = Array.from({ length: right.length + 1 }, (_, index) => index);
  for (let leftIndex = 1; leftIndex <= left.length; leftIndex += 1) {
    let diagonal = row[0];
    row[0] = leftIndex;
    for (let rightIndex = 1; rightIndex <= right.length; rightIndex += 1) {
      const above = row[rightIndex];
      row[rightIndex] = Math.min(
        row[rightIndex] + 1,
        row[rightIndex - 1] + 1,
        diagonal + (left[leftIndex - 1] === right[rightIndex - 1] ? 0 : 1)
      );
      diagonal = above;
    }
  }
  return row[right.length];
};
const __unknownMethod = method => {
  const suggestion = __methodNames
    .map(name => ({ name, distance: __levenshtein(method.toLowerCase(), name.toLowerCase()) }))
    .sort((left, right) => left.distance - right.distance || left.name.localeCompare(right.name))[0].name;
  const error = new Error(`Unknown codemode.wit method '${method}'. Did you mean '${suggestion}'? Call codemode.wit.help() for signatures.`);
  Object.defineProperties(error, {
    code: { value: "unknown_method", enumerable: true },
    operation: { value: `codemode.wit.${method}`, enumerable: true }
  });
  return error;
};
const __help = method => {
  if (method === undefined) {
    return {
      namespace: "codemode.wit",
      methods: __helpEntries,
      limits: { final_result_bytes: 49152, host_result_bytes: 65536 },
      guidance: "Return only needed fields. Prefer read text/lines and list paths formats for compact results."
    };
  }
  const entry = __helpEntries.find(candidate => candidate.name === method);
  if (!entry) throw __unknownMethod(String(method));
  return entry;
};
const __witTarget = Object.freeze({
  help: __help,
  findRepositories: input => __hostOperation("wit_find_repositories", input),
  refs: input => __hostOperation("wit_refs", input),
  open: input => __hostOperation("wit_open", input),
  list: input => __hostOperation("wit_list", input),
  searchCode: input => __hostOperation("wit_search_code", input),
  read: input => __hostOperation("wit_read", { ...input, format: input && input.format || "text" }),
  context: input => __hostOperation("wit_context", input)
});
const __wit = new Proxy(__witTarget, {
  get: (target, property) => {
    if (typeof property !== "string" || Object.prototype.hasOwnProperty.call(target, property)) {
      return target[property];
    }
    throw __unknownMethod(property);
  }
});
Object.defineProperty(globalThis, "codemode", {
  value: Object.freeze({ wit: __wit }),
  writable: false,
  configurable: false
});
const __validateFinalValue = value => {
  const seen = new Set();
  const visit = current => {
    if (current === null || typeof current === "string" || typeof current === "boolean") return;
    if (typeof current === "number") {
      if (!Number.isFinite(current)) throw new Error("final value contains a non-finite number");
      return;
    }
    if (typeof current !== "object") throw new Error("final value is not JSON-serializable");
    if (seen.has(current)) throw new Error("final value contains a cycle");
    seen.add(current);
    if (Array.isArray(current)) {
      for (let index = 0; index < current.length; index += 1) {
        if (!Object.prototype.hasOwnProperty.call(current, index)) throw new Error("final value contains an array hole");
        visit(current[index]);
      }
    } else {
      for (const key of Object.keys(current)) visit(current[key]);
    }
    seen.delete(current);
  };
  visit(value);
};
Object.defineProperty(globalThis, "__validateFinal", {
  value: __validateFinalValue,
  writable: false,
  configurable: false
});
})();
"#;
