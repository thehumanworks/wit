//! Native Code Mode MCP adapter.

use crate::{
    codemode_policy::{CodeModePolicy, InvocationBudget, ServerCapacity},
    ensure_rustls_provider,
    operation_context::{OperationCancellation, OperationContext},
    operation_registry::{
        DispatchErrorCode, OperationDispatchError, render_typescript_declarations,
    },
    operations::WitOperations,
};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{
        router::tool::{ToolRoute, ToolRouter},
        tool::ToolCallContext,
    },
    model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo, Tool},
    tool_handler,
    transport::stdio,
};
use serde_json::{Map, Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use wit_quickjs_spike::{InvocationOutcome, Limits, TestAction};

const CODE_TOOL_NAME: &str = "code";
const CODE_WALL_TIME: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct CodeModeMcpServer {
    tool_router: ToolRouter<Self>,
    operations: WitOperations,
    worker: PathBuf,
    policy: CodeModePolicy,
    capacity: Arc<ServerCapacity>,
}

impl CodeModeMcpServer {
    pub fn new(worker: impl Into<PathBuf>) -> Self {
        Self::with_operations(worker, WitOperations::new())
    }

    pub fn with_operations(worker: impl Into<PathBuf>, operations: WitOperations) -> Self {
        let policy = CodeModePolicy::default();
        policy
            .validate()
            .expect("built-in Code Mode policy must be valid");
        Self {
            tool_router: Self::code_tool_router(),
            operations,
            worker: worker.into(),
            capacity: Arc::new(ServerCapacity::new(&policy)),
            policy,
        }
    }

    fn code_tool_router() -> ToolRouter<Self> {
        let mut input_schema = Map::new();
        input_schema.insert("type".into(), json!("object"));
        input_schema.insert("additionalProperties".into(), json!(false));
        input_schema.insert("required".into(), json!(["code"]));
        input_schema.insert(
            "properties".into(),
            json!({
                "code": {
                    "type": "string",
                    "maxLength": wit_quickjs_spike::MAX_SCRIPT_BYTES,
                    "description": "Async JavaScript function body. Return one JSON-serializable value."
                }
            }),
        );
        let description = code_tool_description();
        let tool = Tool::new_with_raw(CODE_TOOL_NAME, Some(description.into()), input_schema);
        let mut router = ToolRouter::new();
        router.add_route(ToolRoute::new_dyn(
            tool,
            |context: ToolCallContext<'_, CodeModeMcpServer>| {
                Box::pin(async move {
                    let arguments = context.arguments.unwrap_or_default();
                    let Some(source) = arguments.get("code").and_then(Value::as_str) else {
                        return Ok(CallToolResult::structured_error(code_error(
                            "invalid_arguments",
                            "code must be a string",
                        )));
                    };
                    let cancellation = OperationCancellation::default();
                    let operation_context = OperationContext::new(None, cancellation.clone())
                        .bounded(CODE_WALL_TIME);
                    tokio::select! {
                        result = context.service.execute_with_context(source, operation_context) => Ok(result),
                        _ = context.request_context.ct.cancelled() => {
                            cancellation.cancel();
                            Ok(CallToolResult::structured_error(code_error(
                                "cancelled",
                                "code execution was cancelled",
                            )))
                        }
                    }
                })
            },
        ));
        router
    }

    #[cfg(test)]
    async fn execute(&self, source: &str) -> CallToolResult {
        self.execute_with_context(source, OperationContext::default().bounded(CODE_WALL_TIME))
            .await
    }

    async fn execute_with_context(
        &self,
        source: &str,
        operation_context: OperationContext,
    ) -> CallToolResult {
        let (source, worker_limits, test_action) =
            adversarial_invocation(source, self.policy.worker_limits());
        if let Err(error) = self.policy.check_source(source) {
            return CallToolResult::structured_error(code_error(error.code, error.message));
        }
        let _capacity = match self.capacity.try_start() {
            Ok(permit) => permit,
            Err(error) => {
                return CallToolResult::structured_error(code_error(error.code, error.message));
            }
        };
        let budget = Arc::new(InvocationBudget::new(&self.policy));
        let capacity = Arc::clone(&self.capacity);
        let operations = self.operations.clone();
        let outcome = wit_quickjs_spike::invoke_with_host_handler_and_action(
            &self.worker,
            source,
            worker_limits,
            test_action,
            move |operation, arguments| {
                let operations = operations.clone();
                let operation_context = operation_context.clone();
                let budget = Arc::clone(&budget);
                let capacity = Arc::clone(&capacity);
                async move {
                    budget.reserve_operation(&operation).map_err(|error| {
                        wit_quickjs_spike::HostError {
                            code: error.code.into(),
                            operation: operation.clone(),
                            message: error.message,
                        }
                    })?;
                    let _server_host_slot =
                        capacity.acquire_host_operation().await.map_err(|error| {
                            wit_quickjs_spike::HostError {
                                code: error.code.into(),
                                operation: operation.clone(),
                                message: error.message,
                            }
                        })?;
                    let value = operations
                        .dispatch(&operation_context, &operation, arguments)
                        .await
                        .map_err(host_dispatch_error)?;
                    Ok(sanitize_host_result(&operation, value))
                }
            },
        )
        .await;

        match outcome {
            Ok(InvocationOutcome::Success(value, _)) => CallToolResult::structured(value),
            Ok(InvocationOutcome::Rejected(message)) => {
                let code = if message.contains("host-call budget exhausted") {
                    "host_calls_limit"
                } else if message.contains("execution deadline exceeded") {
                    "deadline_exceeded"
                } else if message.contains("final JSON") && message.contains("exceeds") {
                    "final_result_bytes_limit"
                } else {
                    "code_rejected"
                };
                CallToolResult::structured_error(code_error(code, message))
            }
            Ok(InvocationOutcome::TimedOut) => CallToolResult::structured_error(code_error(
                "deadline_exceeded",
                "code execution deadline exceeded",
            )),
            Ok(InvocationOutcome::Cancelled { .. }) => CallToolResult::structured_error(
                code_error("cancelled", "code execution was cancelled"),
            ),
            Ok(InvocationOutcome::WorkerExited(_)) => CallToolResult::structured_error(code_error(
                "worker_exited",
                "code worker exited before returning a result",
            )),
            Ok(InvocationOutcome::ProtocolError(message)) => {
                CallToolResult::structured_error(code_error("worker_protocol_error", message))
            }
            Ok(InvocationOutcome::LimitExceeded { code, message }) => {
                CallToolResult::structured_error(code_error(code, message))
            }
            Err(error) => {
                tracing::error!(?error, "failed to start Code Mode worker");
                CallToolResult::structured_error(code_error(
                    "worker_start_failed",
                    "code worker could not be started",
                ))
            }
        }
    }
}

#[cfg(not(debug_assertions))]
fn adversarial_invocation(source: &str, limits: Limits) -> (&str, Limits, TestAction) {
    (source, limits, TestAction::Execute)
}

#[cfg(debug_assertions)]
fn adversarial_invocation(source: &str, mut limits: Limits) -> (&str, Limits, TestAction) {
    if let Some(source) = source.strip_prefix("/* wit-adversarial:crash */") {
        (source, limits, TestAction::Crash)
    } else if let Some(source) = source.strip_prefix("/* wit-adversarial:malformed */") {
        (source, limits, TestAction::EmitUnknownMessage)
    } else if let Some(source) = source.strip_prefix("/* wit-adversarial:per-call-bytes */") {
        limits.max_host_result_bytes = 1024;
        limits.max_cumulative_host_result_bytes = 4096;
        (source, limits, TestAction::Execute)
    } else if let Some(source) = source.strip_prefix("/* wit-adversarial:cumulative-bytes */") {
        limits.max_host_result_bytes = 2048;
        limits.max_cumulative_host_result_bytes = 4096;
        (source, limits, TestAction::Execute)
    } else {
        (source, limits, TestAction::Execute)
    }
}

fn code_error(code: &str, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn host_dispatch_error(error: OperationDispatchError) -> wit_quickjs_spike::HostError {
    let message = match error.code {
        DispatchErrorCode::OperationFailed => "wit operation failed".to_string(),
        _ => error.message,
    };
    wit_quickjs_spike::HostError {
        code: serde_json::to_value(error.code)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "operation_failed".into()),
        operation: error.operation,
        message,
    }
}

fn sanitize_host_result(operation: &str, mut value: Value) -> Value {
    if operation == "wit_open"
        && let Some(cache) = value.get_mut("cache").and_then(Value::as_object_mut)
    {
        // Cache refresh diagnostics originate in git/network errors and may contain absolute
        // paths or token-shaped data. Direct MCP retains the field; untrusted Code Mode does not.
        cache.insert("last_error".into(), Value::Null);
    }
    value
}

fn code_tool_description() -> String {
    format!(
        "Execute one bounded async JavaScript function body and return its JSON value. No filesystem, network, environment, module, shell, subprocess, or arbitrary MCP APIs are available. Host-operation errors are catchable Error objects with stable code, operation, and message fields. Explicit cursors remain visible.\n\nStart with codemode.wit.help() for method signatures and examples. If owner/repo is fuzzy, use:\nconst repos = await codemode.wit.findRepositories({{ pattern: 'ratatuizilla', max_items: 5 }});\n\nFor a known repository:\nconst opened = await codemode.wit.open({{ repo: 'owner/repo' }});\nreturn await codemode.wit.searchCode({{ snapshot_id: opened.snapshot_id, queries: ['symbol'], path_prefix: 'src' }});\n\nPrefer read()'s default text format, read({{ format: 'lines' }}), and list({{ format: 'paths' }}) to keep results below the 48 KiB final limit.\n\n{}",
        render_typescript_declarations()
    )
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for CodeModeMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "wit-mcp-codemode",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Call the single code tool with one bounded async JavaScript function body. Use codemode.wit.help() for signatures, compact read/list formats and search path filters to control result size, and return only focused JSON evidence.",
            )
    }
}

pub async fn serve_stdio_with_worker(worker: impl Into<PathBuf>) -> anyhow::Result<()> {
    ensure_rustls_provider();
    let service = CodeModeMcpServer::new(worker)
        .serve(stdio())
        .await
        .inspect_err(|error| tracing::error!(?error, "Code Mode server failed"))?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_registry::OPERATIONS;
    use rmcp::model::CallToolRequestParams;
    use std::{
        path::{Path, PathBuf},
        process::{Command, Stdio},
        time::{SystemTime, UNIX_EPOCH},
    };

    static ENVIRONMENT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[test]
    fn code_mode_advertises_exactly_one_tool_with_all_generated_operations() {
        let server = CodeModeMcpServer::new("unused-in-contract-test");
        let tools = server.tool_router.list_all();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, CODE_TOOL_NAME);
        let description = tools[0].description.as_deref().unwrap();
        for operation in OPERATIONS {
            assert!(description.contains(&format!("{}(", operation.code_method)));
        }
        assert!(description.contains("No filesystem, network, environment"));
    }

    #[test]
    fn privileged_operation_errors_are_redacted_before_worker_ipc() {
        let error = host_dispatch_error(OperationDispatchError {
            code: DispatchErrorCode::OperationFailed,
            operation: "wit_open".into(),
            message: "GITHUB_TOKEN=secret /private/cache/repo.git".into(),
        });
        assert_eq!(error.code, "operation_failed");
        assert_eq!(error.operation, "wit_open");
        assert_eq!(error.message, "wit operation failed");
        let rendered = format!("{error:?}");
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("/private/cache"));
    }

    #[tokio::test]
    async fn oversized_source_has_a_stable_error_without_starting_worker() {
        let server = CodeModeMcpServer::new("worker-must-not-start");
        let source = "x".repeat(wit_quickjs_spike::MAX_SCRIPT_BYTES + 1);
        let result = server.execute(&source).await;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "source_bytes_limit"
        );
    }

    #[tokio::test]
    async fn mcp_tools_list_exposes_only_code() -> anyhow::Result<()> {
        let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(async move {
            let server = CodeModeMcpServer::new("unused-in-contract-test")
                .serve(server_transport)
                .await?;
            server.waiting().await?;
            anyhow::Ok(())
        });
        let client = ().serve(client_transport).await?;
        let tools = client.list_all_tools().await?;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, CODE_TOOL_NAME);
        let description = tools[0].description.as_deref().unwrap();
        for operation in OPERATIONS {
            assert!(description.contains(&format!("{}(", operation.code_method)));
        }
        client.cancel().await?;
        server_task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn host_errors_are_catchable_and_final_json_is_strictly_bounded() -> anyhow::Result<()> {
        let server = CodeModeMcpServer::new(worker_path()?);
        let caught = successful_value(
            server
                .execute(
                    r#"
                    try {
                      await codemode.wit.read({});
                      return { caught: false };
                    } catch (error) {
                      return {
                        caught: true,
                        code: error.code,
                        operation: error.operation,
                        message: error.message
                      };
                    }
                    "#,
                )
                .await,
        )?;
        assert_eq!(caught["caught"], true);
        assert_eq!(caught["code"], "invalid_arguments");
        assert_eq!(caught["operation"], "wit_read");
        assert_eq!(
            caught["message"],
            "arguments do not match the operation schema"
        );

        for (source, expected_code) in [
            (
                "const value = {}; value.self = value; return value;",
                "code_rejected",
            ),
            (
                "return { invalid: Number.POSITIVE_INFINITY };",
                "code_rejected",
            ),
            ("return 'x'.repeat(60 * 1024);", "final_result_bytes_limit"),
        ] {
            let result = server.execute(source).await;
            assert_eq!(result.is_error, Some(true));
            assert_eq!(result.structured_content.unwrap()["code"], expected_code);
        }
        assert_eq!(
            successful_value(server.execute("return 'recovered';").await)?,
            "recovered"
        );
        Ok(())
    }

    #[tokio::test]
    async fn one_program_pages_and_reads_fixture_and_snapshot_survives_next_call()
    -> anyhow::Result<()> {
        let _environment_lock = ENVIRONMENT_LOCK.lock().await;
        let cache = tempfile::tempdir()?;
        let fixture = seed_cached_repo(cache.path())?;
        let _environment = TestEnvironment::install(cache.path(), &fixture.git_config);
        let (client, server_task) = start_code_client(worker_path()?).await?;
        let first = successful_value(
            call_code(
                &client,
                r#"
                    const opened = await codemode.wit.open({ repo: "owner/repo" });
                    const first = await codemode.wit.searchCode({
                      snapshot_id: opened.snapshot_id,
                      queries: ["alpha|beta|gamma"],
                      globs: ["*.md"],
                      max_results: 1,
                      max_bytes: 4096
                    });
                    const second = await codemode.wit.searchCode({
                      snapshot_id: opened.snapshot_id,
                      queries: ["alpha|beta|gamma"],
                      globs: ["*.md"],
                      max_results: 1,
                      max_bytes: 4096,
                      cursor: first.next_cursor
                    });
                    const read = await codemode.wit.read({
                      snapshot_id: opened.snapshot_id,
                      path: first.items[0].path,
                      start_line: first.items[0].match_line,
                      end_line: first.items[0].match_line,
                      max_bytes: 4096
                    });
                    return { opened, first, second, read };
                    "#,
            )
            .await?,
        )?;
        assert_eq!(first["first"]["has_more"], true);
        assert!(first["first"]["next_cursor"].is_string());
        assert_ne!(
            first["first"]["items"][0]["match_line"],
            first["second"]["items"][0]["match_line"]
        );
        assert_eq!(first["read"]["path"], "README.md");
        for group in ["first", "second"] {
            let item = &first[group]["items"][0];
            assert_eq!(item["repo"], first["opened"]["repo"]);
            assert_eq!(item["commit_sha"], first["opened"]["commit_sha"]);
            assert_eq!(item["snapshot_id"], first["opened"]["snapshot_id"]);
            assert!(item["blob_sha"].is_string());
        }
        assert_eq!(first["read"]["repo"], first["opened"]["repo"]);
        assert_eq!(first["read"]["commit_sha"], first["opened"]["commit_sha"]);
        assert_eq!(first["read"]["snapshot_id"], first["opened"]["snapshot_id"]);
        assert!(first["read"]["blob_sha"].is_string());

        let snapshot_id = serde_json::to_string(&first["opened"]["snapshot_id"])?;
        let later = successful_value(
            call_code(
                &client,
                &format!(
                    "return await codemode.wit.read({{ snapshot_id: {snapshot_id}, path: 'README.md', start_line: 2, end_line: 2, max_bytes: 4096 }});"
                ),
            )
            .await?,
        )?;
        assert_eq!(later["text"], "beta");
        assert_eq!(later["snapshot_id"], first["opened"]["snapshot_id"]);
        client.cancel().await?;
        server_task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn successful_open_redacts_cache_refresh_error_before_worker_ipc() -> anyhow::Result<()> {
        let _environment_lock = ENVIRONMENT_LOCK.lock().await;
        let cache = tempfile::tempdir()?;
        let fixture = seed_cached_repo(cache.path())?;
        let metadata_path = cache
            .path()
            .join("owner/repo/branches/b-main/metadata.json");
        let mut metadata: Value = serde_json::from_slice(&std::fs::read(&metadata_path)?)?;
        metadata["last_error"] =
            json!("fetch failed at /private/cache/repo.git with GITHUB_TOKEN=review-secret");
        std::fs::write(&metadata_path, serde_json::to_vec_pretty(&metadata)?)?;
        let _environment = TestEnvironment::install(cache.path(), &fixture.git_config);
        let server = CodeModeMcpServer::new(worker_path()?);

        let opened = successful_value(
            server
                .execute(r#"return await codemode.wit.open({ repo: "owner/repo" });"#)
                .await,
        )?;
        assert_eq!(opened["cache"]["state"], "stale_with_error");
        assert_eq!(opened["cache"]["last_error"], Value::Null);
        let rendered = serde_json::to_string(&opened)?;
        assert!(!rendered.contains("review-secret"));
        assert!(!rendered.contains("/private/cache"));
        assert!(!rendered.contains("GITHUB_TOKEN"));
        Ok(())
    }

    async fn start_code_client(
        worker: PathBuf,
    ) -> anyhow::Result<(
        rmcp::service::RunningService<rmcp::RoleClient, ()>,
        tokio::task::JoinHandle<anyhow::Result<()>>,
    )> {
        let (server_transport, client_transport) = tokio::io::duplex(128 * 1024);
        let server_task = tokio::spawn(async move {
            let server = CodeModeMcpServer::new(worker)
                .serve(server_transport)
                .await?;
            server.waiting().await?;
            anyhow::Ok(())
        });
        Ok((().serve(client_transport).await?, server_task))
    }

    async fn call_code(
        client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
        source: &str,
    ) -> anyhow::Result<CallToolResult> {
        Ok(client
            .call_tool(
                CallToolRequestParams::new(CODE_TOOL_NAME)
                    .with_arguments(json!({ "code": source }).as_object().unwrap().clone()),
            )
            .await?)
    }

    fn successful_value(result: CallToolResult) -> anyhow::Result<Value> {
        if result.is_error == Some(true) {
            anyhow::bail!("code tool failed: {:?}", result.structured_content);
        }
        result
            .structured_content
            .ok_or_else(|| anyhow::anyhow!("code tool omitted structured content"))
    }

    fn worker_path() -> anyhow::Result<PathBuf> {
        let executable = std::env::current_exe()?;
        let debug_dir = executable
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| anyhow::anyhow!("test executable has no target/debug parent"))?;
        let name = if cfg!(windows) {
            "wit-quickjs-spike-worker.exe"
        } else {
            "wit-quickjs-spike-worker"
        };
        let worker = debug_dir.join(name);
        if !worker.is_file() {
            anyhow::bail!(
                "build the experimental worker first: cargo build -p wit-quickjs-spike --bin wit-quickjs-spike-worker"
            );
        }
        Ok(worker)
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        git_config: PathBuf,
    }

    fn seed_cached_repo(cache_dir: &Path) -> anyhow::Result<Fixture> {
        let temp = tempfile::tempdir()?;
        let worktree = temp.path().join("worktree");
        let remote = temp.path().join("remote.git");
        let branch_dir = cache_dir
            .join("owner")
            .join("repo")
            .join("branches")
            .join("b-main");
        let repo_path = branch_dir.join("repo.git");
        run_git(&["init", worktree.to_str().unwrap()], None)?;
        run_git(&["checkout", "-b", "main"], Some(&worktree))?;
        std::fs::write(worktree.join("README.md"), "alpha\nbeta\ngamma\n")?;
        run_git(&["add", "."], Some(&worktree))?;
        run_git(
            &[
                "-c",
                "user.name=wit-test",
                "-c",
                "user.email=wit-test@example.com",
                "commit",
                "-m",
                "fixture",
            ],
            Some(&worktree),
        )?;
        let sha = git_stdout(&["rev-parse", "HEAD"], Some(&worktree))?;
        run_git(&["init", "--bare", remote.to_str().unwrap()], None)?;
        run_git(
            &["remote", "add", "origin", remote.to_str().unwrap()],
            Some(&worktree),
        )?;
        run_git(&["push", "origin", "main"], Some(&worktree))?;
        run_git(&["symbolic-ref", "HEAD", "refs/heads/main"], Some(&remote))?;
        std::fs::create_dir_all(&branch_dir)?;
        run_git(
            &[
                "clone",
                "--bare",
                worktree.to_str().unwrap(),
                repo_path.to_str().unwrap(),
            ],
            None,
        )?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        std::fs::write(
            branch_dir.join("metadata.json"),
            serde_json::to_vec_pretty(&json!({
                "cache_schema_version": 1,
                "owner_repo": "owner/repo",
                "branch": "main",
                "remote_url": "https://github.com/owner/repo",
                "current_sha": sha,
                "last_checked_at": now,
                "last_updated_at": now
            }))?,
        )?;
        let git_config = temp.path().join("gitconfig");
        std::fs::write(
            &git_config,
            format!(
                "[url \"file://{}\"]\n\tinsteadOf = https://github.com/owner/repo\n\tinsteadOf = https://github.com/owner/repo.git\n",
                remote.display()
            ),
        )?;
        Ok(Fixture {
            _temp: temp,
            git_config,
        })
    }

    fn run_git(args: &[&str], workdir: Option<&Path>) -> anyhow::Result<()> {
        let output = git_command(args, workdir).output()?;
        if !output.status.success() {
            anyhow::bail!(
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    fn git_stdout(args: &[&str], workdir: Option<&Path>) -> anyhow::Result<String> {
        let output = git_command(args, workdir).output()?;
        if !output.status.success() {
            anyhow::bail!(
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    }

    fn git_command(args: &[&str], workdir: Option<&Path>) -> Command {
        let mut command = Command::new("git");
        command.args(args).stdin(Stdio::null());
        if let Some(workdir) = workdir {
            command.current_dir(workdir);
        }
        command
    }

    struct TestEnvironment(Vec<(String, Option<std::ffi::OsString>)>);

    impl TestEnvironment {
        fn install(cache: &Path, git_config: &Path) -> Self {
            let values = [
                ("WIT_CACHE_DIR", cache.as_os_str()),
                ("GIT_CONFIG_GLOBAL", git_config.as_os_str()),
                ("GIT_CONFIG_NOSYSTEM", std::ffi::OsStr::new("1")),
            ];
            let previous = values
                .iter()
                .map(|(name, _)| ((*name).to_owned(), std::env::var_os(name)))
                .collect();
            for (name, value) in values {
                // SAFETY: this focused test is run in isolation; the guard restores every value.
                unsafe { std::env::set_var(name, value) };
            }
            Self(previous)
        }
    }

    impl Drop for TestEnvironment {
        fn drop(&mut self) {
            for (name, value) in self.0.drain(..) {
                // SAFETY: see install; restoration happens before the isolated test returns.
                unsafe {
                    if let Some(value) = value {
                        std::env::set_var(name, value);
                    } else {
                        std::env::remove_var(name);
                    }
                }
            }
        }
    }
}
