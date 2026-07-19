use std::{path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Mutex,
    time,
};
use wit_quickjs_spike::{
    ExecuteRequest, InvocationOutcome, Limits, PROTOCOL_VERSION, ParentMessage, TestAction,
    WorkerMessage, invoke, invoke_with_action, invoke_with_host_handler, read_frame, write_frame,
};

// Resource-attack cases intentionally consume a whole worker budget. Keep native CI stable on
// small runners while still exercising concurrency inside the JavaScript/IPC test itself.
static TEST_LOCK: Mutex<()> = Mutex::const_new(());

fn worker() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_wit-quickjs-spike-worker"))
}

async fn success(script: &str) -> Result<Value> {
    match invoke(&worker(), script, Limits::default()).await? {
        InvocationOutcome::Success(value, _) => Ok(value),
        outcome => bail!("expected success, got {outcome:?}"),
    }
}

async fn assert_contained_failure(script: &str, limits: Limits) -> Result<()> {
    match invoke(&worker(), script, limits).await? {
        InvocationOutcome::Rejected(_)
        | InvocationOutcome::TimedOut
        | InvocationOutcome::WorkerExited(_)
        | InvocationOutcome::ProtocolError(_)
        | InvocationOutcome::LimitExceeded { .. } => Ok(()),
        InvocationOutcome::Cancelled { .. } => bail!("resource attack was externally cancelled"),
        InvocationOutcome::Success(value, _) => {
            bail!("resource attack unexpectedly succeeded: {value}")
        }
    }
}

#[tokio::test]
async fn async_host_calls_resume_sequentially_and_concurrently() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let limits = Limits {
        max_concurrent_host_calls: 2,
        ..Limits::default()
    };
    let (value, stats) = match invoke(
        &worker(),
        r#"
        const first = await hostCall({ step: 1 });
        const second = await hostCall({ step: 2, prior: first.echo.step });
        const concurrent = await Promise.all(
          Array.from({ length: 8 }, (_, index) => hostCall({ concurrent: index }))
        );
        return { first, second, concurrent };
        "#,
        limits,
    )
    .await?
    {
        InvocationOutcome::Success(value, stats) => (value, stats),
        outcome => bail!("expected success, got {outcome:?}"),
    };

    assert_eq!(value["first"]["echo"], json!({ "step": 1 }));
    assert_eq!(value["second"]["echo"]["prior"], 1);
    assert_eq!(value["concurrent"].as_array().map(Vec::len), Some(8));
    assert_eq!(stats.host_calls, 10);
    assert_eq!(stats.max_concurrent_host_calls, 2);
    Ok(())
}

#[tokio::test]
async fn javascript_has_no_ambient_host_capabilities_or_module_loader() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let value = success(
        r#"
        let importBlocked = false;
        try { await import("not-available"); } catch (_) { importBlocked = true; }
        return {
          process: typeof process,
          require: typeof require,
          fetch: typeof fetch,
          websocket: typeof WebSocket,
          deno: typeof Deno,
          bun: typeof Bun,
          quickjsStd: typeof std,
          quickjsOs: typeof os,
          importBlocked
        };
        "#,
    )
    .await?;

    for name in [
        "process",
        "require",
        "fetch",
        "websocket",
        "deno",
        "bun",
        "quickjsStd",
        "quickjsOs",
    ] {
        assert_eq!(value[name], "undefined", "ambient capability {name}");
    }
    assert_eq!(value["importBlocked"], true);
    Ok(())
}

#[tokio::test]
async fn production_bridge_exposes_no_generic_host_handle_or_parent_secrets() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let outcome = invoke_with_host_handler(
        &worker(),
        r#"
        let importBlocked = false;
        try { await import("not-available"); } catch (_) { importBlocked = true; }
        return {
          process: typeof process,
          require: typeof require,
          fetch: typeof fetch,
          hostCall: typeof hostCall,
          rawBridge: typeof __hostCallJson,
          closedBridge: typeof __bridge,
          env: typeof env,
          importBlocked,
          methods: Object.keys(codemode.wit).sort()
        };
        "#,
        Limits::default(),
        |_operation, _arguments| async move { panic!("capability probe reached the trusted host") },
    )
    .await?;
    let InvocationOutcome::Success(value, _) = outcome else {
        bail!("capability probe failed unexpectedly: {outcome:?}");
    };
    for name in [
        "process",
        "require",
        "fetch",
        "hostCall",
        "rawBridge",
        "closedBridge",
        "env",
    ] {
        assert_eq!(value[name], "undefined", "capability {name}");
    }
    assert_eq!(value["importBlocked"], true);
    assert_eq!(value["methods"].as_array().map(Vec::len), Some(7));
    Ok(())
}

#[tokio::test]
async fn production_fanout_returns_a_stable_concurrency_error() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let outcome = invoke_with_host_handler(
        &worker(),
        r#"
        try {
          await Promise.all(Array.from({ length: 3 }, (_, index) =>
            codemode.wit.read({ index })
          ));
          return "unexpected";
        } catch (error) {
          return { code: error.code, operation: error.operation };
        }
        "#,
        Limits {
            max_concurrent_host_calls: 2,
            ..Limits::default()
        },
        |_operation, _arguments| async move {
            time::sleep(Duration::from_millis(25)).await;
            Ok(Value::Null)
        },
    )
    .await?;
    let InvocationOutcome::Success(value, _) = outcome else {
        bail!("fanout probe failed unexpectedly: {outcome:?}");
    };
    assert_eq!(value["code"], "host_concurrency_limit");
    assert_eq!(value["operation"], "wit_read");
    Ok(())
}

#[tokio::test]
async fn interruption_stack_and_memory_failures_do_not_wedge_parent() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let short = Limits {
        wall_time_ms: 150,
        ..Limits::default()
    };
    assert_contained_failure("while (true) {}", short).await?;
    assert_contained_failure(
        "function recurse() { return recurse(); } return recurse();",
        Limits::default(),
    )
    .await?;
    assert_contained_failure(
        "const held = []; while (true) held.push(new ArrayBuffer(1024 * 1024));",
        Limits {
            memory_bytes: 4 * 1024 * 1024,
            ..Limits::default()
        },
    )
    .await?;

    assert_eq!(success("return 42;").await?, 42);
    Ok(())
}

#[tokio::test]
async fn crashed_timed_out_and_cancelled_workers_are_replaceable() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    match invoke_with_action(
        &worker(),
        "return null;",
        Limits::default(),
        TestAction::Crash,
    )
    .await?
    {
        InvocationOutcome::WorkerExited(_) => {}
        outcome => bail!("expected explicit worker crash, got {outcome:?}"),
    }
    assert_eq!(
        success("return 'after-crash';")
            .await
            .context("restart after crash")?,
        "after-crash"
    );

    let timeout = invoke(
        &worker(),
        "while (true) {}",
        Limits {
            wall_time_ms: 100,
            ..Limits::default()
        },
    )
    .await?;
    assert!(matches!(
        timeout,
        InvocationOutcome::Rejected(_) | InvocationOutcome::TimedOut
    ));
    assert_eq!(
        success("return 'after-timeout';")
            .await
            .context("restart after timeout")?,
        "after-timeout"
    );

    let started = wit_quickjs_spike::start(
        &worker(),
        "while (true) {}",
        Limits {
            wall_time_ms: 5_000,
            ..Limits::default()
        },
    )
    .await?;
    let spawned_pid = started.pid();
    match started.cancel().await? {
        InvocationOutcome::Cancelled { pid, .. } => assert_eq!(pid, spawned_pid),
        outcome => bail!("expected synchronized cancellation, got {outcome:?}"),
    }
    wait_for_pid_exit(spawned_pid).await?;
    assert_eq!(
        success("return 'after-cancel';")
            .await
            .context("restart after cancellation")?,
        "after-cancel"
    );
    Ok(())
}

#[tokio::test]
async fn malformed_and_oversized_ipc_is_rejected_without_harming_parent() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let mut child = Command::new(worker())
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("spawn worker")?;
    child
        .stdin
        .take()
        .context("worker stdin")?
        .write_all(b"{ definitely-not-json }\n")
        .await?;
    let status = time::timeout(Duration::from_secs(2), child.wait()).await??;
    assert!(!status.success());

    let oversized = "x".repeat(wit_quickjs_spike::MAX_SCRIPT_BYTES + 1);
    assert!(
        invoke(&worker(), &oversized, Limits::default())
            .await
            .is_err()
    );
    assert_eq!(success("return 'still-alive';").await?, "still-alive");
    Ok(())
}

#[tokio::test]
async fn host_call_budget_is_enforced() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let outcome = invoke(
        &worker(),
        "for (let i = 0; i < 17; i++) await hostCall(i); return 'unreachable';",
        Limits::default(),
    )
    .await?;
    assert!(matches!(outcome, InvocationOutcome::Rejected(_)));
    Ok(())
}

#[tokio::test]
async fn per_call_and_cumulative_host_result_bytes_have_stable_errors() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let limits = Limits {
        max_host_result_bytes: 1024,
        max_cumulative_host_result_bytes: 2048,
        ..Limits::default()
    };
    let outcome = invoke_with_host_handler(
        &worker(),
        r#"
        const codes = [];
        for (let index = 0; index < 3; index += 1) {
          try { await codemode.wit.read({ index }); }
          catch (error) { codes.push(error.code); }
        }
        return codes;
        "#,
        limits,
        |_operation, _arguments| async move { Ok(json!({ "text": "x".repeat(900) })) },
    )
    .await?;
    let InvocationOutcome::Success(value, stats) = outcome else {
        bail!("expected catchable byte limit: {outcome:?}");
    };
    assert_eq!(value, json!(["cumulative_host_bytes_limit"]));
    assert!(stats.host_result_bytes <= 2048);

    let per_call = invoke_with_host_handler(
        &worker(),
        "try { await codemode.wit.read({}); } catch (error) { return error.code; }",
        Limits {
            max_host_result_bytes: 64,
            max_cumulative_host_result_bytes: 64,
            ..Limits::default()
        },
        |_operation, _arguments| async move { Ok(json!({ "text": "x".repeat(100) })) },
    )
    .await?;
    assert!(matches!(
        per_call,
        InvocationOutcome::Success(Value::String(ref code), _)
            if code == "host_result_bytes_limit"
    ));
    Ok(())
}

#[tokio::test]
async fn captured_worker_logs_are_capped_and_secret_redacted() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let outcome = invoke_with_action(
        &worker(),
        "return 'safe';",
        Limits::default(),
        TestAction::EmitSecretDiagnostic,
    )
    .await?;
    let rendered = format!("{outcome:?}");
    let InvocationOutcome::Success(_, stats) = outcome else {
        bail!("diagnostic probe did not finish: {rendered}");
    };
    assert_eq!(stats.diagnostic_bytes, wit_quickjs_spike::MAX_LOG_BYTES);
    assert!(stats.diagnostics_truncated);
    assert!(!rendered.contains("should-never-cross-worker-boundary"));
    assert!(!rendered.contains("GITHUB_TOKEN"));
    Ok(())
}

#[tokio::test]
async fn near_limit_host_result_with_quotes_and_newlines_crosses_ipc_once() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let mut payload = String::new();
    let unit = "quoted: \"value\"\n";
    loop {
        let candidate = format!("{payload}{unit}");
        let value = json!({ "text": candidate });
        if serde_json::to_vec(&value)?.len() > 64 * 1024 {
            break;
        }
        payload.push_str(unit);
    }
    let host_value = json!({ "text": payload });
    let serialized_len = serde_json::to_vec(&host_value)?.len();
    assert!(serialized_len > 63 * 1024);
    assert!(serialized_len <= 64 * 1024);
    let expected_length = host_value["text"].as_str().unwrap().len();
    let expected_tail = host_value["text"]
        .as_str()
        .unwrap()
        .chars()
        .rev()
        .take(16)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    let outcome = invoke_with_host_handler(
        &worker(),
        "const result = await codemode.wit.read({}); return { length: result.text.length, tail: result.text.slice(-16) };",
        Limits::default(),
        move |operation, _arguments| {
            let host_value = host_value.clone();
            async move {
                assert_eq!(operation, "wit_read");
                Ok(host_value)
            }
        },
    )
    .await?;
    match outcome {
        InvocationOutcome::Success(value, _) => {
            assert_eq!(value["length"], expected_length);
            assert_eq!(value["tail"], expected_tail);
        }
        outcome => bail!("near-limit host result failed: {outcome:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn worker_diagnostics_stay_on_stderr_and_stdout_remains_framed() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let mut child = Command::new(worker())
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().context("worker stdin")?;
    let mut stdout = child.stdout.take().context("worker stdout")?;
    let mut stderr = child.stderr.take().context("worker stderr")?;
    write_frame(
        &mut stdin,
        &ExecuteRequest {
            version: PROTOCOL_VERSION,
            invocation_id: 9,
            script: "return { framed: true };".into(),
            limits: Limits::default(),
            expose_test_host_call: true,
            test_action: TestAction::EmitDiagnostic,
        },
    )
    .await?;
    drop(stdin);
    let frame = time::timeout(Duration::from_secs(2), read_frame(&mut stdout))
        .await??
        .context("worker closed before result")?;
    assert!(matches!(
        serde_json::from_slice::<WorkerMessage>(&frame)?,
        WorkerMessage::Result {
            invocation_id: 9,
            ok: true,
            ..
        }
    ));
    assert!(child.wait().await?.success());
    assert!(read_frame(&mut stdout).await?.is_none());
    let mut diagnostics = String::new();
    stderr.read_to_string(&mut diagnostics).await?;
    assert!(diagnostics.contains("worker diagnostic framing probe"));
    Ok(())
}

#[tokio::test]
async fn trusted_parent_rejects_forged_oversized_worker_success() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let outcome = invoke_with_action(
        &worker(),
        "return 'unreachable';",
        Limits::default(),
        TestAction::ForgeOversizedSuccess,
    )
    .await?;
    assert!(matches!(
        outcome,
        InvocationOutcome::LimitExceeded { code: "final_result_bytes_limit", ref message }
            if message == &format!(
                "worker result exceeds {}-byte limit",
                wit_quickjs_spike::MAX_RESULT_BYTES
            )
    ));
    Ok(())
}

#[tokio::test]
async fn malformed_result_envelopes_are_stable_protocol_errors() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    for (action, expected) in [
        (
            TestAction::ForgeMissingSuccessValue,
            "worker returned ok=true without a value",
        ),
        (
            TestAction::ForgeContradictorySuccess,
            "worker returned a contradictory result envelope",
        ),
        (
            TestAction::ForgeContradictoryFailure,
            "worker returned a contradictory result envelope",
        ),
    ] {
        let outcome =
            invoke_with_action(&worker(), "return null;", Limits::default(), action).await?;
        assert!(
            matches!(
                &outcome,
                InvocationOutcome::ProtocolError(message) if message == expected
            ),
            "{action:?}: {outcome:?}"
        );
    }
    assert_eq!(
        success("return 'after-malformed';").await?,
        "after-malformed"
    );
    Ok(())
}

#[tokio::test]
async fn parent_rejects_zero_duplicate_and_unknown_worker_messages() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    for action in [
        TestAction::EmitZeroCallId,
        TestAction::EmitDuplicateCallId,
        TestAction::EmitUnknownCallId,
        TestAction::EmitUnknownMessage,
    ] {
        let outcome =
            invoke_with_action(&worker(), "return null;", Limits::default(), action).await?;
        assert!(matches!(outcome, InvocationOutcome::ProtocolError(_)));
    }
    Ok(())
}

#[tokio::test]
async fn worker_rejects_zero_unknown_and_duplicate_parent_call_ids() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    for response_ids in [vec![0], vec![999], vec![1, 1]] {
        let script = if response_ids.len() == 2 {
            "return await Promise.all([hostCall(1), hostCall(2)]);"
        } else {
            "return await hostCall(1);"
        };
        let mut child = Command::new(worker())
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdin = child.stdin.take().context("worker stdin")?;
        let mut stdout = child.stdout.take().context("worker stdout")?;
        write_frame(
            &mut stdin,
            &ExecuteRequest {
                version: PROTOCOL_VERSION,
                invocation_id: 7,
                script: script.into(),
                limits: Limits::default(),
                expose_test_host_call: true,
                test_action: TestAction::Execute,
            },
        )
        .await?;

        let mut emitted_ids = Vec::new();
        while emitted_ids.len() < response_ids.len().max(1) {
            let frame = time::timeout(Duration::from_secs(2), read_frame(&mut stdout))
                .await??
                .context("worker closed before host call")?;
            if let WorkerMessage::HostCall { call_id, .. } = serde_json::from_slice(&frame)? {
                emitted_ids.push(call_id);
            }
        }
        for response_id in response_ids {
            write_frame(
                &mut stdin,
                &ParentMessage::HostResult {
                    invocation_id: 7,
                    call_id: response_id,
                    ok: true,
                    value: Some(Value::Null),
                    error: None,
                },
            )
            .await?;
        }
        let final_frame = time::timeout(Duration::from_secs(2), read_frame(&mut stdout))
            .await??
            .context("worker closed before rejection")?;
        assert!(matches!(
            serde_json::from_slice::<WorkerMessage>(&final_frame)?,
            WorkerMessage::Result { ok: false, .. }
        ));
        let _ = child.wait().await?;
    }
    Ok(())
}

#[cfg(unix)]
async fn pid_is_alive(pid: u32) -> Result<bool> {
    let status = time::timeout(
        Duration::from_secs(1),
        Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .context("PID liveness probe timed out")??;
    Ok(status.success())
}

async fn wait_for_pid_exit(pid: u32) -> Result<()> {
    for _ in 0..40 {
        if !pid_is_alive(pid).await? {
            return Ok(());
        }
        time::sleep(Duration::from_millis(25)).await;
    }
    bail!("cancelled worker PID {pid} is still alive")
}

#[cfg(windows)]
async fn pid_is_alive(pid: u32) -> Result<bool> {
    let output = time::timeout(
        Duration::from_secs(1),
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("PID liveness probe timed out")??;
    Ok(String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")))
}
