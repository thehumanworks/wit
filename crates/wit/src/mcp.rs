use crate::{
    ensure_rustls_provider,
    operation_registry::{DispatchErrorCode, OPERATIONS, OperationDispatchError},
    operations::{OperationCancellation, OperationContext, WitOperations},
};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    handler::server::{
        router::tool::{ToolRoute, ToolRouter},
        tool::ToolCallContext,
    },
    model::{
        CallToolResult, Implementation, ListResourcesResult, PaginatedRequestParams,
        ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    tool_handler,
    transport::stdio,
};
use serde_json::json;
use std::{
    future::Future,
    time::{Duration, Instant},
};

const SKILL_MD: &str = include_str!("skill/SKILL.md");
const MCP_OPERATION_TIMEOUT: Duration = Duration::from_secs(120);

fn bridged_operation_context(
    cancelled: impl Future<Output = ()> + Send + 'static,
) -> (OperationContext, tokio::task::JoinHandle<()>) {
    let cancellation = OperationCancellation::default();
    let operation_context = OperationContext::new(
        Some(Instant::now() + MCP_OPERATION_TIMEOUT),
        cancellation.clone(),
    );
    let bridge = tokio::spawn(async move {
        cancelled.await;
        cancellation.cancel();
    });
    (operation_context, bridge)
}

const WIT_WORKFLOW_GUIDE: &str = r#"# wit MCP workflow

Direct mode is the default and current recommendation. Use it for a simple open, list, search, read, or other one-operation task. Experimental Code Mode is an explicit opt-in for bounded composition: start `wit mcp --transport stdio --mode code` or `wit-mcp --mode code`; it exposes one normal MCP `code` tool. Omitting `--mode` selects the eight-tool direct surface.

## Direct mode

1. If `owner/repo` is unknown, call `wit_find_repositories` with narrow GitHub qualifiers.
2. Call `wit_refs` when branch or tag discovery matters, then call `wit_open`. Reuse the returned immutable `snapshot_id` throughout the task.
3. Call `wit_list` to orient with an explicit depth, or `wit_search_code` when symbols or text are known.
4. Call `wit_read` with explicit one-based line ranges for precise evidence.
5. Call `wit_context` when one deterministic operation should rank and merge evidence across files. It does not invoke a model or embeddings.
6. Call `wit_ast` for structure instead of text: `mode: "symbols"` (default) returns every definition in a file or directory with exact one-based line ranges, nesting, and signatures, so the next `wit_read` can be precise; `mode: "query"` with `language` runs a raw tree-sitter query (for example `(call_expression function: (identifier) @callee (#eq? @callee "render"))`) for questions regex cannot express. Supported: rust, python, javascript, typescript, tsx, go, java, c.
7. When `has_more` is true, pass `next_cursor` back with otherwise unchanged arguments. Changed arguments or snapshots invalidate a cursor.
8. Responses are structured by default and bounded to 64 KiB. Set `include_rendered_text` only for compatibility with text-oriented consumers.

## Experimental Code Mode

Pass one async JavaScript function body to `code`. Start with `codemode.wit.help()` for all signatures and examples or `codemode.wit.help("read")` for one method. Use `await` with `codemode.wit.findRepositories`, `codemode.wit.refs`, `codemode.wit.open`, `codemode.wit.list`, `codemode.wit.searchCode`, `codemode.wit.ast`, `codemode.wit.read`, and `codemode.wit.context`; TypeScript syntax, imports, filesystem, network, environment, process, subprocess, shell, modules, and arbitrary host calls are unavailable. Unknown method errors suggest the nearest method. Credentials and privileged operations remain in the Rust parent.

When owner/repo is fuzzy, call `findRepositories({ pattern: "ratatuizilla", max_items: 5 })`. Open once and reuse the snapshot within the parent server lifetime. Snapshots do not survive restart. Follow explicit cursors with otherwise unchanged arguments. Code Mode read defaults to compact text with top-level provenance; it also supports lines and structured formats. List supports a paths-only format, and searchCode supports path_prefix, glob/globs, and exclude filters. Return one focused JSON-serializable value with repository, commit, snapshot, path, blob, and line provenance. Page budgets expose remaining bytes and a near-limit warning; oversized final results point to compact result formats. Fixed source/time/call/page/snapshot/result budgets fail explicitly; cancellation and deadlines use stable errors. A failed worker is killed and reaped and the next invocation starts fresh. Generated source and worker diagnostic content are not persisted or logged; only capped diagnostic byte counts and truncation state are retained.

The checked-in external model evaluation is unrun. Code Mode remains experimental, no efficiency or latency improvement is claimed, and incomplete or failed benchmark evidence keeps direct mode as the fail-closed recommendation.
"#;

const WIT_TOOLS_GUIDE: &str = r#"# wit MCP tools

Direct mode (the default and recommendation for simple calls) exposes these eight tools:

- `wit_find_repositories`: discover owner/repo when unknown.
- `wit_refs`: discover default branch, branches, and tags.
- `wit_open`: pin a default branch, named branch, tag, or full commit SHA into an immutable server-lifetime snapshot.
- `wit_list`: bounded structure listing with explicit depth.
- `wit_search_code`: bounded multi-query regex search with atomic context groups and provenance.
- `wit_read`: explicit one-based inclusive line-range read with provenance.
- `wit_context`: deterministic ranked and merged multi-file evidence.
- `wit_ast`: tree-sitter structural search — `symbols` (definitions with exact line ranges, nesting, signatures) or `query` (raw tree-sitter query with captures) for rust, python, javascript, typescript, tsx, go, java, and c.

Collection responses use `items`, `returned_items`, `has_more`, `next_cursor`, and whole-structured-response `budget` metadata. Cursors are opaque and bound to the tool, snapshot, and normalized query. Default responses are at most 64 KiB; the fixed MCP framing outside structured content is not included and is constrained to less than 1 KiB. Human CLI commands are unchanged.

Experimental Code Mode instead exposes one normal MCP tool named `code`. Its input is an async JavaScript function body and its final result must be one JSON-serializable value no larger than 48 KiB. Call `codemode.wit.help()` to discover signatures and examples. The generated host methods are `codemode.wit.findRepositories`, `codemode.wit.refs`, `codemode.wit.open`, `codemode.wit.list`, `codemode.wit.searchCode`, `codemode.wit.ast`, `codemode.wit.read`, and `codemode.wit.context`. Read defaults to compact text and supports `format: "lines"`; list supports `format: "paths"`; searchCode supports `path_prefix`, `glob`/`globs`, and `exclude`. Their host results keep explicit cursors, budgets, snapshots, and provenance, including remaining bytes and near-limit warnings.

Default invocation limits are 32 KiB source, 10 seconds, 16 host calls with 4 concurrent, 8 page-producing calls, 2 snapshot opens, 64 KiB per host result, and 256 KiB cumulative host results. Resource, cancellation, deadline, worker, protocol, and invalid-final-result failures are explicit structured errors. Host-operation errors are catchable JavaScript errors with stable `code`, `operation`, and redacted `message`. There is no filesystem, network, environment, process, subprocess, shell, module loader, credential, or generic host-call capability.

Source is held only for an invocation and is not persisted or logged. Worker stderr content is discarded after capped byte/truncation accounting. Every invocation starts a fresh worker; a crashed, wedged, timed-out, or cancelled worker is killed and reaped. Snapshots belong to the Rust parent and expire when that server exits. Code Mode remains experimental because the checked-in model evaluation is unrun; no token, call, or latency benefit is claimed, and direct remains the fail-closed recommendation.
"#;

#[derive(Clone)]
pub struct WitMcpServer {
    tool_router: ToolRouter<Self>,
    operations: WitOperations,
}

impl WitMcpServer {
    pub fn new() -> Self {
        Self::with_operations(WitOperations::new())
    }

    pub fn with_operations(operations: WitOperations) -> Self {
        Self {
            tool_router: Self::operation_tool_router(),
            operations,
        }
    }

    fn operation_tool_router() -> ToolRouter<Self> {
        let mut router = ToolRouter::new();
        for descriptor in OPERATIONS {
            let input_schema = descriptor.input_schema();
            let output_schema = descriptor.output_schema();
            let tool = Tool::new_with_raw(
                descriptor.name,
                Some(descriptor.description.into()),
                input_schema
                    .as_object()
                    .expect("operation input schema must be an object")
                    .clone(),
            )
            .with_raw_output_schema(
                output_schema
                    .as_object()
                    .expect("operation output schema must be an object")
                    .clone()
                    .into(),
            );
            router.add_route(ToolRoute::new_dyn(
                tool,
                |context: ToolCallContext<'_, WitMcpServer>| {
                    Box::pin(async move {
                        let name = context.name.to_string();
                        let arguments =
                            serde_json::Value::Object(context.arguments.unwrap_or_default());
                        let request_ct = context.request_context.ct.clone();
                        let boundary_ct = request_ct.clone();
                        let (operation_context, cancellation_bridge) =
                            bridged_operation_context(request_ct.cancelled_owned());
                        let result = context
                            .service
                            .operations
                            .dispatch(&operation_context, &name, arguments)
                            .await;
                        if boundary_ct.is_cancelled() {
                            operation_context.cancellation().cancel();
                        }
                        cancellation_bridge.abort();
                        let result = match operation_context.check() {
                            Ok(()) => result,
                            Err(message) => Err(OperationDispatchError {
                                code: DispatchErrorCode::OperationFailed,
                                operation: name.clone(),
                                message,
                            }),
                        };
                        match result {
                            Ok(result) => Ok(CallToolResult::structured(result)),
                            Err(error) => Ok(CallToolResult::structured_error(
                                serde_json::to_value(error)
                                    .expect("dispatch errors must serialize"),
                            )),
                        }
                    })
                },
            ));
        }
        router
    }
}

impl Default for WitMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for WitMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("wit-mcp", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "wit MCP v2 is snapshot-first and structured-first. Call wit_open once, reuse snapshot_id for immutable reads, use wit_list for structure, wit_search_code for exact matches, wit_ast for definitions with exact line ranges or tree-sitter queries, wit_read for explicit line ranges, and wit_context for deterministic multi-file evidence. Collection tools are byte-bounded and return next_cursor when has_more is true. Fetch wit://skill/SKILL.md for the full workflow.",
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![
                Resource::new("wit://skill/SKILL.md", "wit-skill")
                    .with_description("Bundled snapshot-first wit agent skill")
                    .with_mime_type("text/markdown"),
                Resource::new("wit://guide/workflow", "wit-workflow-v2")
                    .with_description("Direct and experimental Code Mode selection workflow")
                    .with_mime_type("text/markdown"),
                Resource::new("wit://guide/tools", "wit-tools-v2")
                    .with_description("Direct and Code Mode tools, limits, and recovery contracts")
                    .with_mime_type("text/markdown"),
            ],
            next_cursor: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let text = match request.uri.as_str() {
            "wit://skill/SKILL.md" => SKILL_MD,
            "wit://guide/workflow" => WIT_WORKFLOW_GUIDE,
            "wit://guide/tools" => WIT_TOOLS_GUIDE,
            _ => {
                return Err(McpError::resource_not_found(
                    "resource_not_found",
                    Some(json!({ "uri": request.uri })),
                ));
            }
        };
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, request.uri).with_mime_type("text/markdown"),
        ]))
    }
}

pub async fn serve_stdio() -> anyhow::Result<()> {
    ensure_rustls_provider();
    let service = WitMcpServer::new()
        .serve(stdio())
        .await
        .inspect_err(|err| tracing::error!(?err, "wit MCP v2 server failed"))?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_tool_surface_is_semantic_and_small() {
        let server = WitMcpServer::new();
        let names = server
            .tool_router
            .list_all()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();
        let mut expected = OPERATIONS
            .iter()
            .map(|operation| operation.name.to_string())
            .collect::<Vec<_>>();
        expected.sort_unstable();
        assert_eq!(names, expected);
    }

    #[test]
    fn direct_mcp_contract_is_derived_from_operation_registry() {
        let server = WitMcpServer::new();
        for descriptor in OPERATIONS {
            let tool = server
                .tool_router
                .get(descriptor.name)
                .expect("registered operation must be an MCP tool");
            assert_eq!(tool.description.as_deref(), Some(descriptor.description));
            assert_eq!(
                serde_json::Value::Object((*tool.input_schema).clone()),
                descriptor.input_schema()
            );
            assert_eq!(
                tool.output_schema
                    .as_ref()
                    .map(|schema| serde_json::Value::Object((**schema).clone())),
                Some(descriptor.output_schema())
            );
        }
    }

    #[test]
    fn embedded_guides_publish_the_experimental_mode_contract() {
        let combined = format!("{WIT_WORKFLOW_GUIDE}\n{WIT_TOOLS_GUIDE}");
        for required in [
            "Direct mode is the default",
            "current recommendation",
            "Experimental Code Mode",
            "one normal MCP tool named `code`",
            "codemode.wit.open",
            "snapshot",
            "cursor",
            "provenance",
            "32 KiB source",
            "10 seconds",
            "no filesystem",
            "Credentials and privileged operations remain in the Rust parent",
            "cancellation",
            "killed and reaped",
            "not persisted or logged",
            "model evaluation is unrun",
            "fail-closed recommendation",
        ] {
            assert!(
                combined.contains(required),
                "embedded guides omit {required}"
            );
        }
        assert!(!combined.to_ascii_lowercase().contains("compat-v1"));
        assert!(!combined.to_ascii_lowercase().contains("mcp v1"));
    }

    #[tokio::test]
    async fn adapter_context_applies_deadline_and_bridges_cancellation() {
        let (cancel, cancelled) = tokio::sync::oneshot::channel::<()>();
        let started = Instant::now();
        let (context, bridge) = bridged_operation_context(async move {
            let _ = cancelled.await;
        });
        let deadline = context
            .deadline()
            .expect("adapter policy must set a deadline");
        assert!(deadline > started);
        assert!(deadline <= started + MCP_OPERATION_TIMEOUT + Duration::from_secs(1));

        cancel.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), bridge)
            .await
            .expect("cancellation bridge should be bounded")
            .unwrap();
        assert_eq!(context.check().unwrap_err(), "operation cancelled");
    }
}
