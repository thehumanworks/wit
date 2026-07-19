//! Time-boxed child-process QuickJS feasibility spike.
//!
//! This crate is deliberately not part of wit's public API. It proves the process and IPC
//! boundary that a later production Code Mode implementation may adopt.

extern crate self as wit_quickjs_spike;

use std::{
    collections::HashSet,
    future::Future,
    path::Path,
    pin::Pin,
    process::{ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::Command,
    sync::{Mutex, Semaphore, oneshot},
    task::{JoinHandle, JoinSet},
    time,
};

#[doc(hidden)]
pub mod worker {
    include!("bin/worker.rs");
}

pub const PROTOCOL_VERSION: u8 = 1;
// Host operation pages may use their complete 64 KiB service budget; leave bounded room for the
// typed IPC envelope without forcing their JSON through a second, escaping serialization pass.
pub const MAX_FRAME_BYTES: usize = 72 * 1024;
pub const MAX_SCRIPT_BYTES: usize = 32 * 1024;
pub const MAX_RESULT_BYTES: usize = 48 * 1024;
pub const MAX_HOST_RESULT_BYTES: usize = 64 * 1024;
pub const MAX_CUMULATIVE_HOST_RESULT_BYTES: usize = 256 * 1024;
pub const MAX_LOG_BYTES: usize = 8 * 1024;
pub const MAX_HOST_CALLS: u32 = 16;
pub const MAX_CONCURRENT_HOST_CALLS: u32 = 4;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Limits {
    pub wall_time_ms: u64,
    pub memory_bytes: usize,
    pub stack_bytes: usize,
    pub max_host_calls: u32,
    pub max_concurrent_host_calls: u32,
    pub max_host_result_bytes: usize,
    pub max_cumulative_host_result_bytes: usize,
    pub max_result_bytes: usize,
    pub max_log_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            wall_time_ms: 10_000,
            memory_bytes: 16 * 1024 * 1024,
            stack_bytes: 256 * 1024,
            max_host_calls: MAX_HOST_CALLS,
            max_concurrent_host_calls: MAX_CONCURRENT_HOST_CALLS,
            max_host_result_bytes: MAX_HOST_RESULT_BYTES,
            max_cumulative_host_result_bytes: MAX_CUMULATIVE_HOST_RESULT_BYTES,
            max_result_bytes: MAX_RESULT_BYTES,
            max_log_bytes: MAX_LOG_BYTES,
        }
    }
}

impl Limits {
    pub fn validate(&self) -> Result<()> {
        if self.wall_time_ms == 0 || self.wall_time_ms > 10_000 {
            bail!("wall_time_ms must be in 1..=10000");
        }
        if self.memory_bytes < 1024 * 1024 || self.memory_bytes > 64 * 1024 * 1024 {
            bail!("memory_bytes must be in 1 MiB..=64 MiB");
        }
        if self.stack_bytes < 64 * 1024 || self.stack_bytes > 1024 * 1024 {
            bail!("stack_bytes must be in 64 KiB..=1 MiB");
        }
        if self.max_host_calls == 0 || self.max_host_calls > MAX_HOST_CALLS {
            bail!("max_host_calls must be in 1..={MAX_HOST_CALLS}");
        }
        if self.max_concurrent_host_calls == 0
            || self.max_concurrent_host_calls > MAX_CONCURRENT_HOST_CALLS
        {
            bail!("max_concurrent_host_calls must be in 1..={MAX_CONCURRENT_HOST_CALLS}");
        }
        if self.max_host_result_bytes == 0 || self.max_host_result_bytes > MAX_HOST_RESULT_BYTES {
            bail!("max_host_result_bytes must be in 1..={MAX_HOST_RESULT_BYTES}");
        }
        if self.max_cumulative_host_result_bytes < self.max_host_result_bytes
            || self.max_cumulative_host_result_bytes > MAX_CUMULATIVE_HOST_RESULT_BYTES
        {
            bail!(
                "max_cumulative_host_result_bytes must be in max_host_result_bytes..={MAX_CUMULATIVE_HOST_RESULT_BYTES}"
            );
        }
        if self.max_result_bytes == 0 || self.max_result_bytes > MAX_RESULT_BYTES {
            bail!("max_result_bytes must be in 1..={MAX_RESULT_BYTES}");
        }
        if self.max_log_bytes > MAX_LOG_BYTES {
            bail!("max_log_bytes must be in 0..={MAX_LOG_BYTES}");
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ExecuteRequest {
    pub version: u8,
    pub invocation_id: u64,
    pub script: String,
    pub limits: Limits,
    #[serde(default)]
    pub expose_test_host_call: bool,
    #[serde(default)]
    pub test_action: TestAction,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TestAction {
    #[default]
    Execute,
    Crash,
    EmitZeroCallId,
    EmitDuplicateCallId,
    EmitUnknownCallId,
    EmitUnknownMessage,
    EmitDiagnostic,
    EmitSecretDiagnostic,
    ForgeOversizedSuccess,
    ForgeMissingSuccessValue,
    ForgeContradictorySuccess,
    ForgeContradictoryFailure,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerMessage {
    HostCall {
        invocation_id: u64,
        call_id: u32,
        operation: String,
        argument_json: String,
    },
    Result {
        invocation_id: u64,
        ok: bool,
        #[serde(default)]
        value_present: bool,
        value: Option<Value>,
        error: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostError {
    pub code: String,
    pub operation: String,
    pub message: String,
}

type HostCallFuture = Pin<Box<dyn Future<Output = Result<Value, HostError>> + Send>>;
type HostHandler = Arc<dyn Fn(String, Value) -> HostCallFuture + Send + Sync>;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParentMessage {
    HostResult {
        invocation_id: u64,
        call_id: u32,
        ok: bool,
        value: Option<Value>,
        error: Option<HostError>,
    },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct InvocationStats {
    pub host_calls: u32,
    pub max_concurrent_host_calls: u32,
    pub host_result_bytes: usize,
    pub diagnostic_bytes: usize,
    pub diagnostics_truncated: bool,
}

#[derive(Debug)]
pub enum InvocationOutcome {
    Success(Value, InvocationStats),
    Rejected(String),
    TimedOut,
    Cancelled { pid: u32, exit_code: Option<i32> },
    WorkerExited(Option<i32>),
    ProtocolError(String),
    LimitExceeded { code: &'static str, message: String },
}

pub struct StartedInvocation {
    pid: u32,
    cancel: oneshot::Sender<()>,
    task: JoinHandle<Result<InvocationOutcome>>,
}

impl StartedInvocation {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub async fn wait(self) -> Result<InvocationOutcome> {
        self.task.await.context("invocation task panicked")?
    }

    pub async fn cancel(self) -> Result<InvocationOutcome> {
        let _ = self.cancel.send(());
        self.task.await.context("invocation task panicked")?
    }
}

pub async fn invoke(worker: &Path, script: &str, limits: Limits) -> Result<InvocationOutcome> {
    start_with_action(worker, script, limits, TestAction::Execute)
        .await?
        .wait()
        .await
}

pub async fn invoke_with_action(
    worker: &Path,
    script: &str,
    limits: Limits,
    test_action: TestAction,
) -> Result<InvocationOutcome> {
    start_with_action(worker, script, limits, test_action)
        .await?
        .wait()
        .await
}

pub async fn start(worker: &Path, script: &str, limits: Limits) -> Result<StartedInvocation> {
    start_with_action(worker, script, limits, TestAction::Execute).await
}

pub async fn start_with_action(
    worker: &Path,
    script: &str,
    limits: Limits,
    test_action: TestAction,
) -> Result<StartedInvocation> {
    start_with_handler(
        worker,
        script,
        limits,
        test_action,
        true,
        Arc::new(|operation, argument| {
            Box::pin(async move {
                if operation != "__spike_echo" {
                    return Err(HostError {
                        code: "unknown_operation".into(),
                        operation,
                        message: "unknown spike operation".into(),
                    });
                }
                // A real async suspension proves that JS promises and both IPC pumps progress.
                time::sleep(Duration::from_millis(10)).await;
                Ok(serde_json::json!({ "echo": argument }))
            })
        }),
    )
    .await
}

pub async fn invoke_with_host_handler<F, Fut>(
    worker: &Path,
    script: &str,
    limits: Limits,
    handler: F,
) -> Result<InvocationOutcome>
where
    F: Fn(String, Value) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, HostError>> + Send + 'static,
{
    invoke_with_host_handler_and_action(worker, script, limits, TestAction::Execute, handler).await
}

#[doc(hidden)]
pub async fn invoke_with_host_handler_and_action<F, Fut>(
    worker: &Path,
    script: &str,
    limits: Limits,
    test_action: TestAction,
    handler: F,
) -> Result<InvocationOutcome>
where
    F: Fn(String, Value) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, HostError>> + Send + 'static,
{
    let handler: HostHandler =
        Arc::new(move |operation, arguments| Box::pin(handler(operation, arguments)));
    start_with_handler(worker, script, limits, test_action, false, handler)
        .await?
        .wait()
        .await
}

async fn start_with_handler(
    worker: &Path,
    script: &str,
    limits: Limits,
    test_action: TestAction,
    expose_test_host_call: bool,
    handler: HostHandler,
) -> Result<StartedInvocation> {
    limits.validate()?;
    if script.len() > MAX_SCRIPT_BYTES {
        bail!("script exceeds {MAX_SCRIPT_BYTES}-byte limit");
    }

    let scratch = tempfile::tempdir().context("create isolated worker directory")?;
    let mut child = Command::new(worker)
        .arg("__codemode-worker")
        .env_clear()
        .current_dir(scratch.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawn QuickJS worker {}", worker.display()))?;
    let pid = child.id().context("spawned worker has no process id")?;
    #[cfg(debug_assertions)]
    record_adversarial_pid(pid)?;

    let stdin = Arc::new(Mutex::new(
        child.stdin.take().context("worker stdin was not piped")?,
    ));
    let stdout = child.stdout.take().context("worker stdout was not piped")?;
    let stderr = child.stderr.take().context("worker stderr was not piped")?;
    let invocation_id = 1;
    let request = ExecuteRequest {
        version: PROTOCOL_VERSION,
        invocation_id,
        script: script.to_owned(),
        limits: limits.clone(),
        expose_test_host_call,
        test_action,
    };
    write_frame(&mut *stdin.lock().await, &request).await?;

    let (cancel, cancelled) = oneshot::channel();
    let task = tokio::spawn(async move {
        let _scratch = scratch;
        let diagnostic_task = tokio::spawn(capture_diagnostics(stderr, limits.max_log_bytes));
        let deadline = Duration::from_millis(limits.wall_time_ms.saturating_add(250));
        let outcome: Result<InvocationOutcome> = tokio::select! {
            _ = cancelled => {
                let status = terminate_and_reap(&mut child).await?;
                Ok(InvocationOutcome::Cancelled { pid, exit_code: status.code() })
            }
            result = time::timeout(
                deadline,
                drive_parent(stdout, stdin, invocation_id, &limits, handler),
            ) => match result {
                Err(_) => {
                    let _ = terminate_and_reap(&mut child).await?;
                    Ok(InvocationOutcome::TimedOut)
                }
                Ok(result) => {
                    let outcome = result?;
                    let closed_without_result = matches!(outcome, InvocationOutcome::ProtocolError(ref message) if message == "worker closed IPC without a result");
                    if matches!(outcome, InvocationOutcome::ProtocolError(_)) && !closed_without_result {
                        let _ = child.start_kill();
                    }
                    let status = match time::timeout(Duration::from_millis(250), child.wait()).await {
                        Ok(status) => status.context("wait for worker")?,
                        Err(_) => {
                            terminate_and_reap(&mut child).await?
                        }
                    };
                    if !status.success()
                        && (closed_without_result
                            || !matches!(outcome, InvocationOutcome::ProtocolError(_)))
                    {
                        Ok(InvocationOutcome::WorkerExited(status.code()))
                    } else {
                        Ok(outcome)
                    }
                }
            }
        };
        let mut outcome = outcome?;
        let diagnostics = diagnostic_task.await.context("diagnostic task panicked")?;
        if let InvocationOutcome::Success(_, stats) = &mut outcome {
            stats.diagnostic_bytes = diagnostics.bytes;
            stats.diagnostics_truncated = diagnostics.truncated;
        }
        Ok(outcome)
    });
    Ok(StartedInvocation { pid, cancel, task })
}

#[cfg(debug_assertions)]
fn record_adversarial_pid(pid: u32) -> Result<()> {
    use std::io::Write;

    let Some(path) = std::env::var_os("WIT_CODEMODE_TEST_PID_LOG") else {
        return Ok(());
    };
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .context("open Code Mode adversarial PID log")?;
    writeln!(log, "{pid}").context("write Code Mode adversarial PID log")
}

#[derive(Clone, Copy, Debug, Default)]
struct DiagnosticStats {
    bytes: usize,
    truncated: bool,
}

async fn capture_diagnostics(mut stderr: impl AsyncRead + Unpin, limit: usize) -> DiagnosticStats {
    let mut buffer = [0_u8; 4096];
    let mut total = 0_usize;
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => total = total.saturating_add(read),
        }
    }
    DiagnosticStats {
        bytes: total.min(limit),
        truncated: total > limit,
    }
}

async fn terminate_and_reap(child: &mut tokio::process::Child) -> Result<ExitStatus> {
    let _ = child.start_kill();
    time::timeout(Duration::from_secs(2), child.wait())
        .await
        .context("worker did not exit after kill")?
        .context("reap killed worker")
}

async fn drive_parent<R, W>(
    mut stdout: R,
    stdin: Arc<Mutex<W>>,
    invocation_id: u64,
    limits: &Limits,
    handler: HostHandler,
) -> Result<InvocationOutcome>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let semaphore = Arc::new(Semaphore::new(limits.max_concurrent_host_calls as usize));
    let active = Arc::new(AtomicU32::new(0));
    let max_active = Arc::new(AtomicU32::new(0));
    let cumulative_result_bytes = Arc::new(AtomicUsize::new(0));
    let mut host_tasks = JoinSet::new();
    let mut observed_calls = 0_u32;
    let mut seen_call_ids = HashSet::new();

    loop {
        let frame = match read_frame(&mut stdout).await {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                return Ok(InvocationOutcome::ProtocolError(
                    "worker closed IPC without a result".into(),
                ));
            }
            Err(error) => return Ok(InvocationOutcome::ProtocolError(error.to_string())),
        };
        let message: WorkerMessage = match serde_json::from_slice(&frame) {
            Ok(message) => message,
            Err(error) => return Ok(InvocationOutcome::ProtocolError(error.to_string())),
        };
        match message {
            WorkerMessage::HostCall {
                invocation_id: message_invocation_id,
                call_id,
                operation,
                argument_json,
            } => {
                if message_invocation_id != invocation_id {
                    return Ok(InvocationOutcome::ProtocolError(
                        "host call invocation id mismatch".into(),
                    ));
                }
                let expected_call_id = observed_calls + 1;
                if call_id != expected_call_id || !seen_call_ids.insert(call_id) {
                    return Ok(InvocationOutcome::ProtocolError(
                        "worker emitted a zero, duplicate, or unknown call id".into(),
                    ));
                }
                observed_calls += 1;
                if observed_calls > limits.max_host_calls {
                    return Ok(InvocationOutcome::LimitExceeded {
                        code: "host_calls_limit",
                        message: "host-call budget exhausted".into(),
                    });
                }
                let argument = match serde_json::from_str::<Value>(&argument_json) {
                    Ok(value) => value,
                    Err(error) => {
                        return Ok(InvocationOutcome::ProtocolError(format!(
                            "invalid host-call JSON: {error}"
                        )));
                    }
                };
                let stdin = Arc::clone(&stdin);
                let semaphore = Arc::clone(&semaphore);
                let active = Arc::clone(&active);
                let max_active = Arc::clone(&max_active);
                let handler = Arc::clone(&handler);
                let cumulative_result_bytes = Arc::clone(&cumulative_result_bytes);
                let max_host_result_bytes = limits.max_host_result_bytes;
                let max_cumulative_host_result_bytes = limits.max_cumulative_host_result_bytes;
                host_tasks.spawn(async move {
                    let _permit = semaphore
                        .acquire_owned()
                        .await
                        .map_err(|error| anyhow!(error))?;
                    let now_active = active.fetch_add(1, Ordering::SeqCst) + 1;
                    max_active.fetch_max(now_active, Ordering::SeqCst);
                    let response = match handler(operation.clone(), argument).await {
                        Ok(value) => host_value_response(
                            invocation_id,
                            call_id,
                            operation,
                            value,
                            &cumulative_result_bytes,
                            max_host_result_bytes,
                            max_cumulative_host_result_bytes,
                        ),
                        Err(error) => ParentMessage::HostResult {
                            invocation_id,
                            call_id,
                            ok: false,
                            value: None,
                            error: Some(error),
                        },
                    };
                    let result = write_frame(&mut *stdin.lock().await, &response).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    result
                });
            }
            WorkerMessage::Result {
                invocation_id: message_invocation_id,
                ok,
                value_present,
                value,
                error,
            } => {
                if message_invocation_id != invocation_id {
                    return Ok(InvocationOutcome::ProtocolError(
                        "result invocation id mismatch".into(),
                    ));
                }
                // A rejected Promise.all may produce a final value while sibling calls are still
                // in flight. Once the worker has finalized, cancel those privileged futures and
                // never let a late write to its closing stdin turn a contained rejection into a
                // parent failure.
                host_tasks.abort_all();
                while host_tasks.join_next().await.is_some() {}
                let stats = InvocationStats {
                    host_calls: observed_calls,
                    max_concurrent_host_calls: max_active.load(Ordering::SeqCst),
                    host_result_bytes: cumulative_result_bytes.load(Ordering::SeqCst),
                    diagnostic_bytes: 0,
                    diagnostics_truncated: false,
                };
                return Ok(match (ok, value_present, value, error) {
                    (true, true, value, None) => {
                        let value = value.unwrap_or(Value::Null);
                        validated_worker_result(value, stats, limits.max_result_bytes)
                    }
                    (false, false, None, Some(error)) => InvocationOutcome::Rejected(error),
                    (true, false, None, None) => InvocationOutcome::ProtocolError(
                        "worker returned ok=true without a value".into(),
                    ),
                    _ => InvocationOutcome::ProtocolError(
                        "worker returned a contradictory result envelope".into(),
                    ),
                });
            }
        }
    }
}

fn host_value_response(
    invocation_id: u64,
    call_id: u32,
    operation: String,
    value: Value,
    cumulative_bytes: &AtomicUsize,
    per_call_limit: usize,
    cumulative_limit: usize,
) -> ParentMessage {
    let bytes = match serde_json::to_vec(&value) {
        Ok(serialized) => serialized.len(),
        Err(_) => {
            return host_limit_response(
                invocation_id,
                call_id,
                operation,
                "host_result_invalid",
                "host result is not valid JSON",
            );
        }
    };
    if bytes > per_call_limit {
        return host_limit_response(
            invocation_id,
            call_id,
            operation,
            "host_result_bytes_limit",
            "host result exceeds the per-call byte budget",
        );
    }
    if !charge_bytes(cumulative_bytes, bytes, cumulative_limit) {
        return host_limit_response(
            invocation_id,
            call_id,
            operation,
            "cumulative_host_bytes_limit",
            "cumulative host-result byte budget exhausted",
        );
    }
    ParentMessage::HostResult {
        invocation_id,
        call_id,
        ok: true,
        value: Some(value),
        error: None,
    }
}

fn host_limit_response(
    invocation_id: u64,
    call_id: u32,
    operation: String,
    code: &str,
    message: &str,
) -> ParentMessage {
    ParentMessage::HostResult {
        invocation_id,
        call_id,
        ok: false,
        value: None,
        error: Some(HostError {
            code: code.into(),
            operation,
            message: message.into(),
        }),
    }
}

fn charge_bytes(total: &AtomicUsize, amount: usize, limit: usize) -> bool {
    total
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            current.checked_add(amount).filter(|next| *next <= limit)
        })
        .is_ok()
}

fn validated_worker_result(
    value: Value,
    stats: InvocationStats,
    max_result_bytes: usize,
) -> InvocationOutcome {
    match serde_json::to_vec(&value) {
        Ok(serialized) if serialized.len() <= max_result_bytes => {
            InvocationOutcome::Success(value, stats)
        }
        Ok(_) => InvocationOutcome::LimitExceeded {
            code: "final_result_bytes_limit",
            message: format!("worker result exceeds {max_result_bytes}-byte limit"),
        },
        Err(_) => InvocationOutcome::ProtocolError("worker result is not valid JSON".into()),
    }
}

pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        match reader.read(&mut byte).await? {
            0 if frame.is_empty() => return Ok(None),
            0 => bail!("truncated IPC frame"),
            _ if byte[0] == b'\n' => return Ok(Some(frame)),
            _ => {
                if frame.len() == MAX_FRAME_BYTES {
                    bail!("IPC frame exceeds {MAX_FRAME_BYTES}-byte limit");
                }
                frame.push(byte[0]);
            }
        }
    }
}

pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    message: &T,
) -> Result<()> {
    let frame = serde_json::to_vec(message)?;
    if frame.len() > MAX_FRAME_BYTES {
        bail!("IPC frame exceeds {MAX_FRAME_BYTES}-byte limit");
    }
    writer.write_all(&frame).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_parent_rejects_forged_oversized_success() {
        let value = Value::String("x".repeat(MAX_RESULT_BYTES));
        assert!(matches!(
            validated_worker_result(value, InvocationStats::default(), MAX_RESULT_BYTES),
            InvocationOutcome::LimitExceeded { code: "final_result_bytes_limit", ref message }
                if message == &format!("worker result exceeds {MAX_RESULT_BYTES}-byte limit")
        ));
    }
}
