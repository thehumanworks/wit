use crate::operations::{
    AstArgs, AstItem, ContextArgs, ContextItem, FindRepositoriesArgs, ListArgs, ListResponse,
    OpenArgs, OpenResponse, OperationContext, Page, ReadArgs, ReadResponse, RefItem, RefsArgs,
    RepositoryItem, SearchCodeArgs, SearchItem, WitOperations,
};
use schemars::{JsonSchema, schema_for};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{collections::BTreeMap, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationClass {
    Discovery,
    Snapshot,
    Read,
    Search,
}

#[derive(Clone, Copy)]
pub struct OperationDescriptor {
    pub name: &'static str,
    pub code_method: &'static str,
    pub description: &'static str,
    pub classification: OperationClass,
    pub dispatch_target: DispatchTarget,
    pub input_type: &'static str,
    pub output_type: &'static str,
    input_schema: fn() -> Value,
    output_schema: fn() -> Value,
}

impl OperationDescriptor {
    pub fn input_schema(&self) -> Value {
        (self.input_schema)()
    }

    pub fn output_schema(&self) -> Value {
        (self.output_schema)()
    }
}

impl fmt::Debug for OperationDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperationDescriptor")
            .field("name", &self.name)
            .field("code_method", &self.code_method)
            .field("description", &self.description)
            .field("classification", &self.classification)
            .field("dispatch_target", &self.dispatch_target)
            .field("input_type", &self.input_type)
            .field("output_type", &self.output_type)
            .finish_non_exhaustive()
    }
}

fn json_schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schema_for!(T)).expect("Rust operation schemas must serialize")
}

macro_rules! declare_operations {
    ($(
        $target:ident {
            name: $name:literal,
            code_method: $code_method:literal,
            description: $description:literal,
            classification: $classification:ident,
            handler: $handler:ident,
            input: $input:ty,
            output: $output:ty $(,)?
        }
    ),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum DispatchTarget {
            $($target),+
        }

        /// The sole declaration of wit operations shared by direct MCP and Code Mode.
        pub static OPERATIONS: &[OperationDescriptor] = &[
            $(OperationDescriptor {
                name: $name,
                code_method: $code_method,
                description: $description,
                classification: OperationClass::$classification,
                dispatch_target: DispatchTarget::$target,
                input_type: stringify!($input),
                output_type: stringify!($output),
                input_schema: json_schema::<$input>,
                output_schema: json_schema::<$output>,
            }),+
        ];

        impl WitOperations {
            pub async fn dispatch(
                &self,
                context: &OperationContext,
                name: &str,
                arguments: Value,
            ) -> Result<Value, OperationDispatchError> {
                let descriptor =
                    operation(name).ok_or_else(|| OperationDispatchError::unknown(name))?;
                match descriptor.dispatch_target {
                    $(DispatchTarget::$target => encode(
                        name,
                        self.$handler(context, decode::<$input>(name, arguments)?)
                            .await
                            .map_err(|error| OperationDispatchError::failed(name, error))?,
                    )),+
                }
            }
        }
    };
}

declare_operations! {
    FindRepositories {
        name: "wit_find_repositories",
        code_method: "findRepositories",
        description: "Discover GitHub repositories when owner/repo is unknown; for fuzzy names use pattern plus a small max_items (for example { pattern: 'ratatuizilla', max_items: 5 }), then call open",
        classification: Discovery,
        handler: find_repositories,
        input: FindRepositoriesArgs,
        output: Page<RepositoryItem>,
    },
    Refs {
        name: "wit_refs",
        code_method: "refs",
        description: "Discover default branch, branches, and tags, or resolve one ref before opening an immutable snapshot",
        classification: Discovery,
        handler: refs,
        input: RefsArgs,
        output: Page<RefItem>,
    },
    Open {
        name: "wit_open",
        code_method: "open",
        description: "Open one immutable repository snapshot before listing, searching, or reading; reuse its snapshot_id to prevent mixed revisions",
        classification: Snapshot,
        handler: open,
        input: OpenArgs,
        output: OpenResponse,
    },
    List {
        name: "wit_list",
        code_method: "list",
        description: "List bounded repository structure from a snapshot with explicit depth; use format: 'paths' for a compact paths-only result",
        classification: Read,
        handler: list,
        input: ListArgs,
        output: ListResponse,
    },
    SearchCode {
        name: "wit_search_code",
        code_method: "searchCode",
        description: "Search one immutable snapshot with regex queries; narrow results with path_prefix, glob/globs, and exclude filters",
        classification: Search,
        handler: search_code,
        input: SearchCodeArgs,
        output: Page<SearchItem>,
    },
    Read {
        name: "wit_read",
        code_method: "read",
        description: "Read an explicit one-based inclusive line range; Code Mode defaults to compact text and supports lines or structured formats",
        classification: Read,
        handler: read,
        input: ReadArgs,
        output: ReadResponse,
    },
    Ast {
        name: "wit_ast",
        code_method: "ast",
        description: "Structural search on a snapshot via tree-sitter: mode 'symbols' (default) indexes definitions with exact line ranges, nesting, and signatures for rust/python/javascript/typescript/tsx/go/java/c; mode 'query' runs a raw tree-sitter query with a required language",
        classification: Search,
        handler: ast,
        input: AstArgs,
        output: Page<AstItem>,
    },
    Context {
        name: "wit_context",
        code_method: "context",
        description: "Gather deterministic ranked multi-file evidence from a snapshot; use when one answer needs several bounded supporting snippets",
        classification: Search,
        handler: context,
        input: ContextArgs,
        output: Page<ContextItem>,
    },
}

pub fn operation(name: &str) -> Option<&'static OperationDescriptor> {
    OPERATIONS.iter().find(|operation| operation.name == name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchErrorCode {
    UnknownOperation,
    InvalidArguments,
    OperationFailed,
    SerializationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperationDispatchError {
    pub code: DispatchErrorCode,
    pub operation: String,
    pub message: String,
}

impl OperationDispatchError {
    fn unknown(operation: &str) -> Self {
        Self {
            code: DispatchErrorCode::UnknownOperation,
            operation: operation.to_string(),
            message: "unknown wit operation".to_string(),
        }
    }

    fn invalid(operation: &str) -> Self {
        Self {
            code: DispatchErrorCode::InvalidArguments,
            operation: operation.to_string(),
            message: "arguments do not match the operation schema".to_string(),
        }
    }

    fn failed(operation: &str, message: String) -> Self {
        Self {
            code: DispatchErrorCode::OperationFailed,
            operation: operation.to_string(),
            message,
        }
    }

    fn serialization(operation: &str) -> Self {
        Self {
            code: DispatchErrorCode::SerializationFailed,
            operation: operation.to_string(),
            message: "operation result could not be serialized".to_string(),
        }
    }
}

impl fmt::Display for OperationDispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.operation, self.message)
    }
}

impl std::error::Error for OperationDispatchError {}

fn decode<T: DeserializeOwned>(name: &str, arguments: Value) -> Result<T, OperationDispatchError> {
    serde_json::from_value(arguments).map_err(|_| OperationDispatchError::invalid(name))
}

fn encode<T: Serialize>(name: &str, result: T) -> Result<Value, OperationDispatchError> {
    serde_json::to_value(result).map_err(|_| OperationDispatchError::serialization(name))
}

pub fn render_typescript_declarations() -> String {
    let mut definitions = BTreeMap::<String, Value>::new();
    let mut roots = Vec::new();
    for operation in OPERATIONS {
        let input_name = format!("{}Input", pascal_case(operation.code_method));
        let output_name = format!("{}Result", pascal_case(operation.code_method));
        let input_schema = operation.input_schema();
        let output_schema = operation.output_schema();
        collect_definitions(&input_schema, &mut definitions);
        collect_definitions(&output_schema, &mut definitions);
        roots.push((
            operation,
            input_name,
            input_schema,
            output_name,
            output_schema,
        ));
    }

    let mut rendered =
        String::from("// Generated from Rust operation contracts. Do not edit by hand.\n\n");
    for (name, schema) in definitions {
        rendered.push_str(&format!("export type {name} = {};\n\n", ts_type(&schema)));
    }
    for (_, input_name, input_schema, output_name, output_schema) in &roots {
        rendered.push_str(&format!(
            "export type {input_name} = {};\n\n",
            ts_type(input_schema)
        ));
        rendered.push_str(&format!(
            "export type {output_name} = {};\n\n",
            ts_type(output_schema)
        ));
    }
    rendered.push_str(
        "export type WitCodeModeMethod = \"findRepositories\" | \"refs\" | \"open\" | \"list\" | \"searchCode\" | \"read\" | \"context\";\n\n",
    );
    rendered.push_str(
        "export type WitCodeModeHelpEntry = { name: WitCodeModeMethod; signature: string; description: string; example: string };\n\n",
    );
    rendered.push_str(
        "export type WitCodeModeHelp = { namespace: \"codemode.wit\"; methods: Array<WitCodeModeHelpEntry>; limits: { final_result_bytes: number; host_result_bytes: number }; guidance: string };\n\n",
    );
    rendered.push_str("export interface WitCodeModeApi {\n");
    rendered.push_str(
        "  /** List all methods and signatures, or describe one method without a host call. */\n  help(): WitCodeModeHelp;\n  help(method: WitCodeModeMethod): WitCodeModeHelpEntry;\n",
    );
    for (operation, input_name, _, output_name, _) in &roots {
        rendered.push_str(&format!("  /** {} */\n", operation.description));
        match operation.code_method {
            "list" => rendered.push_str(
                "  list(arguments: ListInput & { format: \"paths\" }): Promise<CompactListPage>;\n  list(arguments: ListInput & { format?: \"structured\" }): Promise<StructuredListPage>;\n",
            ),
            "read" => rendered.push_str(
                "  read(arguments: ReadInput & { format: \"structured\" }): Promise<StructuredReadPage>;\n  read(arguments: ReadInput & { format: \"lines\" }): Promise<CompactReadLinesPage>;\n  read(arguments: ReadInput & { format?: \"text\" }): Promise<CompactReadTextPage>;\n",
            ),
            _ => rendered.push_str(&format!(
                "  {}(arguments: {}): Promise<{}>;\n",
                operation.code_method, input_name, output_name
            )),
        }
    }
    rendered.push_str("}\n\n");
    rendered.push_str(
        "export {};\n\ndeclare global {\n  const codemode: {\n    readonly wit: WitCodeModeApi;\n  };\n}\n",
    );
    rendered
}

fn collect_definitions(schema: &Value, definitions: &mut BTreeMap<String, Value>) {
    if let Some(items) = schema.get("$defs").and_then(Value::as_object) {
        for (name, value) in items {
            if let Some(previous) = definitions.insert(name.clone(), value.clone()) {
                assert_eq!(
                    previous, *value,
                    "conflicting JSON Schema definition {name}"
                );
            }
        }
    }
}

fn pascal_case(value: &str) -> String {
    let mut result = String::new();
    let mut uppercase = true;
    for character in value.chars() {
        if character == '_' || character == '-' {
            uppercase = true;
        } else if uppercase {
            result.extend(character.to_uppercase());
            uppercase = false;
        } else {
            result.push(character);
        }
    }
    result
}

fn ts_type(schema: &Value) -> String {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        return reference
            .rsplit('/')
            .next()
            .unwrap_or("unknown")
            .to_string();
    }
    if let Some(value) = schema.get("const") {
        return json_literal(value);
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return values
            .iter()
            .map(json_literal)
            .collect::<Vec<_>>()
            .join(" | ");
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(values) = schema.get(key).and_then(Value::as_array) {
            return values.iter().map(ts_type).collect::<Vec<_>>().join(" | ");
        }
    }
    if let Some(values) = schema.get("allOf").and_then(Value::as_array) {
        return values.iter().map(ts_type).collect::<Vec<_>>().join(" & ");
    }
    if let Some(types) = schema.get("type").and_then(Value::as_array) {
        return types
            .iter()
            .map(|value| match value.as_str() {
                Some("null") => "null".to_string(),
                Some("boolean") => "boolean".to_string(),
                Some("integer" | "number") => "number".to_string(),
                Some("string") => "string".to_string(),
                Some("array") => format!(
                    "Array<{}>",
                    schema
                        .get("items")
                        .map(ts_type)
                        .unwrap_or_else(|| "unknown".to_string())
                ),
                Some("object") => object_type(schema),
                _ => "unknown".to_string(),
            })
            .collect::<Vec<_>>()
            .join(" | ");
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("null") => "null".to_string(),
        Some("boolean") => "boolean".to_string(),
        Some("integer" | "number") => "number".to_string(),
        Some("string") => "string".to_string(),
        Some("array") => format!(
            "Array<{}>",
            schema
                .get("items")
                .map(ts_type)
                .unwrap_or_else(|| "unknown".to_string())
        ),
        Some("object") => object_type(schema),
        _ if schema.get("properties").is_some() => object_type(schema),
        _ => "unknown".to_string(),
    }
}

fn object_type(schema: &Value) -> String {
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return schema
            .get("additionalProperties")
            .filter(|value| !value.is_boolean())
            .map(|value| format!("Record<string, {}>", ts_type(value)))
            .unwrap_or_else(|| "Record<string, unknown>".to_string());
    };
    let mut fields = properties.iter().collect::<Vec<_>>();
    fields.sort_by_key(|(name, _)| *name);
    let body = fields
        .into_iter()
        .map(|(name, value)| {
            let optional = if required.contains(name.as_str()) {
                ""
            } else {
                "?"
            };
            format!("{name}{optional}: {}", ts_type(value))
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!("{{ {body} }}")
}

fn json_literal(value: &Value) -> String {
    serde_json::to_string(value).expect("JSON Schema literals must serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn registry_operation_names_and_code_methods_are_unique() {
        let names = OPERATIONS
            .iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        let unique = names.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), OPERATIONS.len());
        let code_methods = OPERATIONS
            .iter()
            .map(|entry| entry.code_method)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(code_methods.len(), OPERATIONS.len());
    }

    #[tokio::test]
    async fn dispatch_errors_are_structured_and_stable() {
        let operations = WitOperations::new();
        assert_eq!(
            serde_json::to_value(
                operations
                    .dispatch(&OperationContext::default(), "wit_missing", json!({}))
                    .await
                    .unwrap_err()
            )
            .unwrap(),
            json!({
                "code": "unknown_operation",
                "operation": "wit_missing",
                "message": "unknown wit operation"
            })
        );
        assert_eq!(
            serde_json::to_value(
                operations
                    .dispatch(
                        &OperationContext::default(),
                        "wit_open",
                        json!({"repo": 42})
                    )
                    .await
                    .unwrap_err()
            )
            .unwrap(),
            json!({
                "code": "invalid_arguments",
                "operation": "wit_open",
                "message": "arguments do not match the operation schema"
            })
        );
    }

    #[test]
    fn checked_in_typescript_declarations_match_registry() {
        assert_eq!(
            include_str!("../codemode.wit.d.ts"),
            render_typescript_declarations(),
            "run scripts/generate_codemode_declarations.sh"
        );
    }
}
