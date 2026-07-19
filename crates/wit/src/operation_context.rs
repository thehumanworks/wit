use anyhow::Context as _;
use std::{
    future::Future,
    io::Read,
    process::{Child, Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::Notify;

pub const OPERATION_CANCELLED: &str = "operation cancelled";
pub const OPERATION_DEADLINE_EXCEEDED: &str = "operation deadline exceeded";
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);
const PROCESS_TERMINATION_GRACE: Duration = Duration::from_millis(250);

#[derive(Debug, Default)]
struct CancellationState {
    cancelled: AtomicBool,
    notify: Notify,
}

#[derive(Debug, Clone, Default)]
pub struct OperationCancellation(Arc<CancellationState>);

impl OperationCancellation {
    pub fn cancel(&self) {
        if !self.0.cancelled.swap(true, Ordering::AcqRel) {
            self.0.notify.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    async fn cancelled(&self) {
        loop {
            let notified = self.0.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct OperationContext {
    deadline: Option<Instant>,
    cancellation: OperationCancellation,
}

impl OperationContext {
    pub fn new(deadline: Option<Instant>, cancellation: OperationCancellation) -> Self {
        Self {
            deadline,
            cancellation,
        }
    }

    pub fn with_deadline(deadline: Instant) -> Self {
        Self::new(Some(deadline), OperationCancellation::default())
    }

    pub fn cancellation(&self) -> OperationCancellation {
        self.cancellation.clone()
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub fn bounded(&self, timeout: Duration) -> Self {
        let bound = Instant::now() + timeout;
        Self::new(
            Some(self.deadline.map_or(bound, |deadline| deadline.min(bound))),
            self.cancellation.clone(),
        )
    }

    pub fn check(&self) -> Result<(), String> {
        if self.cancellation.is_cancelled() {
            return Err(OPERATION_CANCELLED.to_string());
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(OPERATION_DEADLINE_EXCEEDED.to_string());
        }
        Ok(())
    }

    /// Wait for one async dependency while racing the parent cancellation and deadline.
    /// A dependency result that becomes ready after either boundary is deliberately discarded.
    pub async fn wait<F, T>(&self, future: F) -> Result<T, String>
    where
        F: Future<Output = T>,
    {
        self.check()?;
        let result = if let Some(deadline) = self.deadline {
            tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => return Err(OPERATION_CANCELLED.to_string()),
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                    return Err(OPERATION_DEADLINE_EXCEEDED.to_string());
                }
                result = future => result,
            }
        } else {
            tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => return Err(OPERATION_CANCELLED.to_string()),
                result = future => result,
            }
        };
        self.check()?;
        Ok(result)
    }
}

/// Run a child in its own process group and terminate the whole group when the parent operation
/// is cancelled. Unix uses a new process group plus `kill(2)`; Windows uses a new process group
/// plus the system `taskkill /T /F` tree-termination facility.
pub fn command_output(
    context: &OperationContext,
    command: &mut Command,
    action: &str,
) -> anyhow::Result<Output> {
    context.check().map_err(anyhow::Error::msg)?;
    configure_process_group(command);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to invoke process to {action}"))?;
    let stdout = child.stdout.take().expect("stdout was configured as piped");
    let stderr = child.stderr.take().expect("stderr was configured as piped");
    let stdout_reader = thread::spawn(move || read_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_pipe(stderr));

    let status = loop {
        if let Err(error) = context.check() {
            terminate_process_tree(&mut child);
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(anyhow::Error::msg(error));
        }
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("failed while waiting for process to {action}"))?
        {
            break status;
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    };

    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow::anyhow!("stdout reader panicked while waiting to {action}"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("stderr reader panicked while waiting to {action}"))??;
    context.check().map_err(anyhow::Error::msg)?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn read_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn configure_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_process_tree(child: &mut Child) {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    const SIGTERM: i32 = 15;
    const SIGKILL: i32 = 9;
    let group = -(child.id() as i32);
    // SAFETY: `kill` is called with the process group id created above and constant signals.
    unsafe {
        kill(group, SIGTERM);
    }
    let deadline = Instant::now() + PROCESS_TERMINATION_GRACE;
    while Instant::now() < deadline {
        // Probe the whole group, not just the root. The root may exit on SIGTERM while a helper
        // deliberately ignores it; returning on `child.try_wait()` would orphan that descendant.
        // SAFETY: signal 0 probes the process group without delivering a signal.
        if unsafe { kill(group, 0) } == -1 {
            return;
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    // SAFETY: same process group and a constant signal; ESRCH is harmless if it just exited. This
    // is intentionally attempted even when the root already exited.
    unsafe {
        kill(group, SIGKILL);
    }
}

#[cfg(windows)]
fn terminate_process_tree(child: &mut Child) {
    let _ = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
}

#[cfg(not(any(unix, windows)))]
fn terminate_process_tree(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn async_wait_discards_a_late_result_after_cancellation() {
        let cancellation = OperationCancellation::default();
        let context = OperationContext::new(None, cancellation.clone());
        let task = tokio::spawn(async move {
            context
                .wait(async {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    "late"
                })
                .await
        });
        tokio::task::yield_now().await;
        cancellation.cancel();
        assert_eq!(task.await.unwrap().unwrap_err(), OPERATION_CANCELLED);
    }

    #[test]
    fn command_is_terminated_at_deadline() {
        let context = OperationContext::with_deadline(Instant::now() + Duration::from_millis(50));
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "ping -n 30 127.0.0.1 >NUL"]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 30 & wait"]);
            command
        };
        let started = Instant::now();
        let error = command_output(&context, &mut command, "run cancellation fixture").unwrap_err();
        assert_eq!(error.to_string(), OPERATION_DEADLINE_EXCEEDED);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_terminates_descendant_processes() {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("child.pid");
        let cancellation = OperationCancellation::default();
        let context = OperationContext::new(None, cancellation.clone());
        let worker = thread::spawn(move || {
            let mut command = Command::new("sh");
            command
                .args(["-c", "sleep 30 & echo $! > \"$WIT_CHILD_PID_FILE\"; wait"])
                .env("WIT_CHILD_PID_FILE", &pid_file);
            command_output(&context, &mut command, "run process-tree fixture")
        });

        let deadline = Instant::now() + Duration::from_secs(1);
        let pid = loop {
            let path = temp.path().join("child.pid");
            if let Ok(value) = std::fs::read_to_string(path)
                && let Ok(pid) = value.trim().parse::<i32>()
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "fixture child did not start");
            thread::sleep(PROCESS_POLL_INTERVAL);
        };
        cancellation.cancel();
        let result = worker.join().unwrap();
        assert_eq!(result.unwrap_err().to_string(), OPERATION_CANCELLED);
        let gone_deadline = Instant::now() + Duration::from_secs(1);
        loop {
            // SAFETY: signal 0 only probes existence of the fixture pid.
            if unsafe { kill(pid, 0) } == -1 {
                break;
            }
            assert!(
                Instant::now() < gone_deadline,
                "cancelled child process {pid} remained alive"
            );
            thread::sleep(PROCESS_POLL_INTERVAL);
        }
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_sigkills_resistant_descendant_after_root_exits() {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("resistant.pid");
        let cancellation = OperationCancellation::default();
        let context = OperationContext::new(None, cancellation.clone());
        let worker = thread::spawn(move || {
            let mut command = Command::new("sh");
            command
                .args([
                    "-c",
                    "sh -c 'trap \"\" TERM; while :; do sleep 1; done' & echo $! > \"$WIT_CHILD_PID_FILE\"; wait",
                ])
                .env("WIT_CHILD_PID_FILE", &pid_file);
            command_output(&context, &mut command, "run resistant process-tree fixture")
        });

        let start_deadline = Instant::now() + Duration::from_secs(1);
        let pid = loop {
            let path = temp.path().join("resistant.pid");
            if let Ok(value) = std::fs::read_to_string(path)
                && let Ok(pid) = value.trim().parse::<i32>()
            {
                break pid;
            }
            assert!(
                Instant::now() < start_deadline,
                "resistant fixture child did not start"
            );
            thread::sleep(PROCESS_POLL_INTERVAL);
        };
        cancellation.cancel();
        let result = worker.join().unwrap();
        assert_eq!(result.unwrap_err().to_string(), OPERATION_CANCELLED);

        let gone_deadline = Instant::now() + Duration::from_secs(1);
        loop {
            // SAFETY: signal 0 only probes existence of the fixture pid.
            if unsafe { kill(pid, 0) } == -1 {
                break;
            }
            assert!(
                Instant::now() < gone_deadline,
                "SIGTERM-resistant child process {pid} remained alive"
            );
            thread::sleep(PROCESS_POLL_INTERVAL);
        }
    }
}
