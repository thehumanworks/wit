//! AST-backed code search built on tree-sitter.
//!
//! Two operations, both deterministic and offline once the source is in hand:
//!
//! - [`symbols`]: language-aware definition index (functions, types, methods,
//!   constants, …) with exact start/end lines from the parse tree, nesting
//!   (`parent`, `depth`), and a one-line signature. This is the structural
//!   counterpart of `rg`: it answers "what is defined here and where does each
//!   definition end" without regex heuristics.
//! - [`run_query`]: raw tree-sitter S-expression queries with captures, for
//!   questions grep cannot express ("every call to `foo` inside an `unsafe`
//!   block", "async functions without a return type").
//!
//! Grammars are compiled into the native binaries only; the wasm snapshot
//! crate and the URL API keep their regex outline (see ADR 0008).

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fmt;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

/// Largest source the AST operations parse; larger blobs are skipped.
pub const MAX_AST_SOURCE_BYTES: usize = 4 * 1024 * 1024;
/// Signature / capture text is truncated to this many bytes.
pub const MAX_SNIPPET_BYTES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AstLanguage {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    Java,
    C,
}

impl AstLanguage {
    pub const ALL: &'static [AstLanguage] = &[
        AstLanguage::Rust,
        AstLanguage::Python,
        AstLanguage::JavaScript,
        AstLanguage::TypeScript,
        AstLanguage::Tsx,
        AstLanguage::Go,
        AstLanguage::Java,
        AstLanguage::C,
    ];

    /// Stable lowercase name used in CLI flags, JSON, and docs.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::Go => "go",
            Self::Java => "java",
            Self::C => "c",
        }
    }

    /// Extensions this language claims (lowercase, no dot).
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Rust => &["rs"],
            Self::Python => &["py", "pyi"],
            Self::JavaScript => &["js", "mjs", "cjs", "jsx"],
            Self::TypeScript => &["ts", "mts", "cts"],
            Self::Tsx => &["tsx"],
            Self::Go => &["go"],
            Self::Java => &["java"],
            Self::C => &["c", "h"],
        }
    }

    /// Detect by file extension.
    pub fn from_path(path: &str) -> Option<Self> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
        Self::ALL
            .iter()
            .copied()
            .find(|language| language.extensions().contains(&ext.as_str()))
    }

    /// Parse a user-supplied language name or extension (`rust`, `rs`, `ts`, …).
    pub fn from_name(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        Self::ALL.iter().copied().find(|language| {
            language.name() == value || language.extensions().contains(&value.as_str())
        })
    }

    fn grammar(self) -> Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
        }
    }

    /// The definition query: every pattern captures the definition node as
    /// `@def` and its name node as `@name`.
    fn symbol_query(self) -> &'static str {
        match self {
            Self::Rust => RUST_SYMBOLS,
            Self::Python => PYTHON_SYMBOLS,
            Self::JavaScript => JAVASCRIPT_SYMBOLS,
            Self::TypeScript | Self::Tsx => TYPESCRIPT_SYMBOLS,
            Self::Go => GO_SYMBOLS,
            Self::Java => JAVA_SYMBOLS,
            Self::C => C_SYMBOLS,
        }
    }

    /// Human label for a definition node kind (`function_item` → `fn`).
    fn kind_label(self, node_kind: &str) -> &'static str {
        match (self, node_kind) {
            (Self::Rust, "function_item" | "function_signature_item") => "fn",
            (Self::Rust, "struct_item") => "struct",
            (Self::Rust, "enum_item") => "enum",
            (Self::Rust, "union_item") => "union",
            (Self::Rust, "trait_item") => "trait",
            (Self::Rust, "impl_item") => "impl",
            (Self::Rust, "mod_item") => "mod",
            (Self::Rust, "type_item") => "type",
            (Self::Rust, "const_item") => "const",
            (Self::Rust, "static_item") => "static",
            (Self::Rust, "macro_definition") => "macro",
            (Self::Python, "function_definition") => "def",
            (Self::Python, "class_definition") => "class",
            (
                Self::JavaScript | Self::TypeScript | Self::Tsx,
                "function_declaration" | "generator_function_declaration" | "function_signature",
            ) => "function",
            (
                Self::JavaScript | Self::TypeScript | Self::Tsx,
                "class_declaration" | "abstract_class_declaration",
            ) => "class",
            (
                Self::JavaScript | Self::TypeScript | Self::Tsx,
                "method_definition" | "method_signature" | "abstract_method_signature",
            ) => "method",
            (Self::JavaScript | Self::TypeScript | Self::Tsx, "variable_declarator") => "const",
            (Self::TypeScript | Self::Tsx, "interface_declaration") => "interface",
            (Self::TypeScript | Self::Tsx, "type_alias_declaration") => "type",
            (Self::TypeScript | Self::Tsx, "enum_declaration") => "enum",
            (Self::TypeScript | Self::Tsx, "internal_module") => "namespace",
            (Self::Go, "function_declaration") => "func",
            (Self::Go, "method_declaration") => "method",
            (Self::Go, "type_spec") => "type",
            (Self::Go, "const_spec") => "const",
            (Self::Go, "var_spec") => "var",
            (Self::Java, "class_declaration") => "class",
            (Self::Java, "interface_declaration") => "interface",
            (Self::Java, "enum_declaration") => "enum",
            (Self::Java, "record_declaration") => "record",
            (Self::Java, "annotation_type_declaration") => "annotation",
            (Self::Java, "method_declaration") => "method",
            (Self::Java, "constructor_declaration") => "constructor",
            (Self::Java, "field_declaration") => "field",
            (Self::C, "function_definition") => "function",
            (Self::C, "struct_specifier") => "struct",
            (Self::C, "union_specifier") => "union",
            (Self::C, "enum_specifier") => "enum",
            (Self::C, "type_definition") => "typedef",
            (Self::C, "preproc_def" | "preproc_function_def") => "define",
            _ => "definition",
        }
    }
}

impl fmt::Display for AstLanguage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

const RUST_SYMBOLS: &str = r#"
(function_item name: (identifier) @name) @def
(function_signature_item name: (identifier) @name) @def
(struct_item name: (type_identifier) @name) @def
(enum_item name: (type_identifier) @name) @def
(union_item name: (type_identifier) @name) @def
(trait_item name: (type_identifier) @name) @def
(impl_item type: (_) @name) @def
(mod_item name: (identifier) @name) @def
(type_item name: (type_identifier) @name) @def
(const_item name: (identifier) @name) @def
(static_item name: (identifier) @name) @def
(macro_definition name: (identifier) @name) @def
"#;

const PYTHON_SYMBOLS: &str = r#"
(function_definition name: (identifier) @name) @def
(class_definition name: (identifier) @name) @def
"#;

const JAVASCRIPT_SYMBOLS: &str = r#"
(function_declaration name: (identifier) @name) @def
(generator_function_declaration name: (identifier) @name) @def
(class_declaration name: (identifier) @name) @def
(method_definition name: (property_identifier) @name) @def
(variable_declarator name: (identifier) @name value: (arrow_function)) @def
(variable_declarator name: (identifier) @name value: (function_expression)) @def
"#;

const TYPESCRIPT_SYMBOLS: &str = r#"
(function_declaration name: (identifier) @name) @def
(generator_function_declaration name: (identifier) @name) @def
(function_signature name: (identifier) @name) @def
(class_declaration name: (type_identifier) @name) @def
(abstract_class_declaration name: (type_identifier) @name) @def
(method_definition name: (property_identifier) @name) @def
(method_signature name: (property_identifier) @name) @def
(abstract_method_signature name: (property_identifier) @name) @def
(variable_declarator name: (identifier) @name value: (arrow_function)) @def
(variable_declarator name: (identifier) @name value: (function_expression)) @def
(interface_declaration name: (type_identifier) @name) @def
(type_alias_declaration name: (type_identifier) @name) @def
(enum_declaration name: (identifier) @name) @def
(internal_module name: (identifier) @name) @def
"#;

const GO_SYMBOLS: &str = r#"
(function_declaration name: (identifier) @name) @def
(method_declaration name: (field_identifier) @name) @def
(type_spec name: (type_identifier) @name) @def
(const_spec name: (identifier) @name) @def
(var_spec name: (identifier) @name) @def
"#;

const JAVA_SYMBOLS: &str = r#"
(class_declaration name: (identifier) @name) @def
(interface_declaration name: (identifier) @name) @def
(enum_declaration name: (identifier) @name) @def
(record_declaration name: (identifier) @name) @def
(annotation_type_declaration name: (identifier) @name) @def
(method_declaration name: (identifier) @name) @def
(constructor_declaration name: (identifier) @name) @def
(field_declaration declarator: (variable_declarator name: (identifier) @name)) @def
"#;

const C_SYMBOLS: &str = r#"
(function_definition declarator: (function_declarator declarator: (identifier) @name)) @def
(function_definition declarator: (pointer_declarator declarator: (function_declarator declarator: (identifier) @name))) @def
(struct_specifier name: (type_identifier) @name body: (_)) @def
(union_specifier name: (type_identifier) @name body: (_)) @def
(enum_specifier name: (type_identifier) @name body: (_)) @def
(type_definition declarator: (type_identifier) @name) @def
(preproc_def name: (identifier) @name) @def
(preproc_function_def name: (identifier) @name) @def
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstError {
    UnsupportedLanguage(String),
    SourceTooLarge(usize),
    Parse(String),
    Query(String),
}

impl fmt::Display for AstError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedLanguage(path) => write!(
                formatter,
                "no tree-sitter grammar for '{path}' (supported: {})",
                supported_languages_summary()
            ),
            Self::SourceTooLarge(bytes) => write!(
                formatter,
                "source is {bytes} bytes; the AST operations parse at most {MAX_AST_SOURCE_BYTES}"
            ),
            Self::Parse(message) => write!(formatter, "parse failed: {message}"),
            Self::Query(message) => write!(formatter, "invalid tree-sitter query: {message}"),
        }
    }
}

impl std::error::Error for AstError {}

/// `rust (rs), python (py, pyi), …` for help text and errors.
pub fn supported_languages_summary() -> String {
    AstLanguage::ALL
        .iter()
        .map(|language| format!("{} ({})", language.name(), language.extensions().join(", ")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One definition found by [`symbols`]. Lines are one-based and inclusive;
/// columns are zero-based byte offsets within the line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AstSymbol {
    pub kind: String,
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub start_col: usize,
    pub end_col: usize,
    /// Name of the nearest enclosing definition, if any.
    pub parent: Option<String>,
    /// Nesting depth among definitions (0 = top level).
    pub depth: usize,
    /// First line of the definition, trimmed and truncated.
    pub signature: String,
}

/// Optional filters for [`symbols`].
#[derive(Debug, Clone, Default)]
pub struct SymbolFilter {
    /// Keep only these kind labels (`fn`, `struct`, …). Empty = all.
    pub kinds: Vec<String>,
    /// Keep only names matching this regex.
    pub name: Option<Regex>,
}

impl SymbolFilter {
    fn keep(&self, symbol: &AstSymbol) -> bool {
        (self.kinds.is_empty() || self.kinds.iter().any(|kind| kind == &symbol.kind))
            && self
                .name
                .as_ref()
                .is_none_or(|regex| regex.is_match(&symbol.name))
    }
}

fn parse(language: AstLanguage, source: &str) -> Result<Tree, AstError> {
    if source.len() > MAX_AST_SOURCE_BYTES {
        return Err(AstError::SourceTooLarge(source.len()));
    }
    let mut parser = Parser::new();
    parser
        .set_language(&language.grammar())
        .map_err(|err| AstError::Parse(err.to_string()))?;
    parser
        .parse(source, None)
        .ok_or_else(|| AstError::Parse("tree-sitter returned no tree".to_string()))
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

/// One-based inclusive line range of a node. A node whose end position sits at
/// column 0 of a later line (a trailing newline, e.g. C `#define`) does not
/// occupy that line.
fn line_range(node: Node<'_>) -> (usize, usize) {
    let start = node.start_position();
    let end = node.end_position();
    let end_row = if end.column == 0 && end.row > start.row {
        end.row - 1
    } else {
        end.row
    };
    (start.row + 1, end_row + 1)
}

fn truncate_snippet(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or("").trim();
    if first_line.len() <= MAX_SNIPPET_BYTES {
        return first_line.to_string();
    }
    let mut end = MAX_SNIPPET_BYTES;
    while !first_line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &first_line[..end])
}

/// Rust `impl` headers name both the trait and the type (`Display for Point`).
fn rust_impl_name(node: Node<'_>, source: &str) -> Option<String> {
    let ty = node.child_by_field_name("type")?;
    let type_text = node_text(ty, source).trim().to_string();
    match node.child_by_field_name("trait") {
        Some(trait_node) => Some(format!(
            "{} for {}",
            node_text(trait_node, source).trim(),
            type_text
        )),
        None => Some(type_text),
    }
}

/// Extract definitions from `source` in `language`, nested and sorted by position.
pub fn symbols(
    language: AstLanguage,
    source: &str,
    filter: &SymbolFilter,
) -> Result<Vec<AstSymbol>, AstError> {
    let tree = parse(language, source)?;
    let grammar = language.grammar();
    let query = Query::new(&grammar, language.symbol_query())
        .map_err(|err| AstError::Query(format!("built-in symbol query for {language}: {err}")))?;
    let def_index = query
        .capture_index_for_name("def")
        .ok_or_else(|| AstError::Query("symbol query lacks @def".to_string()))?;
    let name_index = query
        .capture_index_for_name("name")
        .ok_or_else(|| AstError::Query("symbol query lacks @name".to_string()))?;

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    // (start_byte, end_byte, symbol) for nesting computation.
    let mut found: Vec<(usize, usize, AstSymbol)> = Vec::new();
    while let Some(matched) = matches.next() {
        let mut def_node = None;
        let mut name_node = None;
        for capture in matched.captures() {
            if capture.index == def_index {
                def_node = Some(capture.node);
            } else if capture.index == name_index {
                name_node = Some(capture.node);
            }
        }
        let (Some(def), Some(name)) = (def_node, name_node) else {
            continue;
        };
        let name_text = if language == AstLanguage::Rust && def.kind() == "impl_item" {
            rust_impl_name(def, source).unwrap_or_else(|| node_text(name, source).to_string())
        } else {
            node_text(name, source).trim().to_string()
        };
        if name_text.is_empty() {
            continue;
        }
        let (start_line, end_line) = line_range(def);
        let symbol = AstSymbol {
            kind: language.kind_label(def.kind()).to_string(),
            name: name_text,
            start_line,
            end_line,
            start_col: def.start_position().column,
            end_col: def.end_position().column,
            parent: None,
            depth: 0,
            signature: truncate_snippet(node_text(def, source)),
        };
        found.push((def.start_byte(), def.end_byte(), symbol));
    }

    // Deterministic order and de-duplication (a node can satisfy two patterns).
    found.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    found.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1 && a.2.name == b.2.name);

    // Nesting: nearest earlier definition whose byte range contains this one.
    let mut stack: Vec<(usize, usize, String)> = Vec::new();
    for (start, end, symbol) in &mut found {
        while stack
            .last()
            .is_some_and(|(_, parent_end, _)| *parent_end < *end || *parent_end <= *start)
        {
            stack.pop();
        }
        symbol.depth = stack.len();
        symbol.parent = stack.last().map(|(_, _, name)| name.clone());
        stack.push((*start, *end, symbol.name.clone()));
    }

    Ok(found
        .into_iter()
        .map(|(_, _, symbol)| symbol)
        .filter(|symbol| filter.keep(symbol))
        .collect())
}

/// One capture from [`run_query`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AstCapture {
    /// Capture name without the `@` (`name`, `call`, …).
    pub capture: String,
    /// Index of the query pattern that matched (0-based, in query order).
    pub pattern_index: usize,
    /// Match ordinal within the file, so captures of one match can be regrouped.
    pub match_index: usize,
    pub node_kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub start_col: usize,
    pub end_col: usize,
    /// Captured text, first line only, truncated.
    pub text: String,
}

/// Run a raw tree-sitter query. Every capture of every match is returned in
/// document order of the match, then capture order.
pub fn run_query(
    language: AstLanguage,
    source: &str,
    query_text: &str,
) -> Result<Vec<AstCapture>, AstError> {
    let tree = parse(language, source)?;
    let grammar = language.grammar();
    let query = Query::new(&grammar, query_text).map_err(|err| AstError::Query(err.to_string()))?;
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    let mut out = Vec::new();
    let mut match_index = 0usize;
    while let Some(matched) = matches.next() {
        for capture in matched.captures() {
            let node = capture.node;
            let (start_line, end_line) = line_range(node);
            out.push(AstCapture {
                capture: capture_names
                    .get(capture.index as usize)
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| capture.index.to_string()),
                pattern_index: matched.pattern_index,
                match_index,
                node_kind: node.kind().to_string(),
                start_line,
                end_line,
                start_col: node.start_position().column,
                end_col: node.end_position().column,
                text: truncate_snippet(node_text(node, source)),
            });
        }
        match_index += 1;
    }
    Ok(out)
}

/// Validate a query against a language without running it (fail fast before
/// walking a repository).
pub fn validate_query(language: AstLanguage, query_text: &str) -> Result<(), AstError> {
    Query::new(&language.grammar(), query_text)
        .map(|_| ())
        .map_err(|err| AstError::Query(err.to_string()))
}

/// Plaintext rendering used by the CLI: `START-END  [indent]kind name`.
pub fn format_symbols(
    path: &str,
    language: AstLanguage,
    symbols: &[AstSymbol],
    total_lines: usize,
) -> String {
    let mut lines = vec![format!("{path} ({language}, {total_lines} lines)")];
    if symbols.is_empty() {
        lines.push("  (no definitions)".to_string());
        return lines.join("\n");
    }
    let width = total_lines.to_string().len();
    for symbol in symbols {
        lines.push(format!(
            "  {:>width$}-{:<width$}  {}{} {}",
            symbol.start_line,
            symbol.end_line,
            "  ".repeat(symbol.depth),
            symbol.kind,
            symbol.name,
            width = width
        ));
    }
    lines.join("\n")
}

/// Plaintext rendering for query captures: `path:line:col: @capture (kind) text`.
pub fn format_captures(path: &str, captures: &[AstCapture]) -> String {
    captures
        .iter()
        .map(|capture| {
            format!(
                "{path}:{}:{}: @{} ({}) {}",
                capture.start_line,
                capture.start_col + 1,
                capture.capture,
                capture.node_kind,
                capture.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(symbols: &[AstSymbol]) -> Vec<String> {
        symbols
            .iter()
            .map(|s| format!("{} {}@{}-{}", s.kind, s.name, s.start_line, s.end_line))
            .collect()
    }

    #[test]
    fn every_builtin_symbol_query_compiles() {
        for language in AstLanguage::ALL {
            Query::new(&language.grammar(), language.symbol_query())
                .unwrap_or_else(|err| panic!("{language} symbol query: {err}"));
        }
    }

    #[test]
    fn language_detection_by_path_and_name() {
        assert_eq!(
            AstLanguage::from_path("src/lib.rs"),
            Some(AstLanguage::Rust)
        );
        assert_eq!(AstLanguage::from_path("a/b.TSX"), Some(AstLanguage::Tsx));
        assert_eq!(AstLanguage::from_path("Makefile"), None);
        assert_eq!(AstLanguage::from_path("x.h"), Some(AstLanguage::C));
        assert_eq!(AstLanguage::from_name("ts"), Some(AstLanguage::TypeScript));
        assert_eq!(AstLanguage::from_name("Python"), Some(AstLanguage::Python));
        assert_eq!(AstLanguage::from_name("cobol"), None);
        assert!(supported_languages_summary().contains("rust (rs)"));
    }

    #[test]
    fn rust_symbols_are_nested_with_exact_ranges() {
        let source = r#"
pub struct Widget {
    name: String,
}

impl fmt::Display for Widget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Widget {
    pub fn new(name: &str) -> Self {
        Self { name: name.into() }
    }
}

pub trait Render {
    fn render(&self) -> String;
}

mod tests {
    #[test]
    fn it_works() {}
}

pub const LIMIT: usize = 1;
macro_rules! say { () => {} }
"#;
        let found = symbols(AstLanguage::Rust, source, &SymbolFilter::default()).unwrap();
        assert_eq!(
            names(&found),
            vec![
                "struct Widget@2-4",
                "impl fmt::Display for Widget@6-10",
                "fn fmt@7-9",
                "impl Widget@12-16",
                "fn new@13-15",
                "trait Render@18-20",
                "fn render@19-19",
                "mod tests@22-25",
                "fn it_works@24-24",
                "const LIMIT@27-27",
                "macro say@28-28",
            ]
        );
        let fmt = &found[2];
        assert_eq!(fmt.parent.as_deref(), Some("fmt::Display for Widget"));
        assert_eq!(fmt.depth, 1);
        assert_eq!(
            fmt.signature,
            "fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {"
        );
        assert_eq!(found[0].depth, 0);
        assert_eq!(found[0].parent, None);
    }

    #[test]
    fn symbol_filters_by_kind_and_name() {
        let source = "fn alpha() {}\nfn beta() {}\nstruct Gamma;\n";
        let only_fns = symbols(
            AstLanguage::Rust,
            source,
            &SymbolFilter {
                kinds: vec!["fn".into()],
                name: None,
            },
        )
        .unwrap();
        assert_eq!(names(&only_fns), vec!["fn alpha@1-1", "fn beta@2-2"]);
        let by_name = symbols(
            AstLanguage::Rust,
            source,
            &SymbolFilter {
                kinds: vec![],
                name: Some(Regex::new("^(beta|Gamma)$").unwrap()),
            },
        )
        .unwrap();
        assert_eq!(names(&by_name), vec!["fn beta@2-2", "struct Gamma@3-3"]);
    }

    #[test]
    fn python_symbols_include_decorated_and_async_defs() {
        let source = "import os\n\n@dataclass\nclass Runner:\n    def __init__(self):\n        pass\n\n    async def run(self):\n        return 1\n\n\ndef main():\n    print(1)\n";
        let found = symbols(AstLanguage::Python, source, &SymbolFilter::default()).unwrap();
        assert_eq!(
            names(&found),
            vec![
                "class Runner@4-9",
                "def __init__@5-6",
                "def run@8-9",
                "def main@12-13"
            ]
        );
        assert_eq!(found[1].parent.as_deref(), Some("Runner"));
    }

    #[test]
    fn typescript_and_javascript_symbols() {
        let ts = r#"
export interface Shape { area(): number }
export type Pair<T> = [T, T];
export const add = (a: number, b: number) => a + b;
const mul = function (a: number) { return a; };
export default class Circle implements Shape {
  constructor(private r: number) {}
  area(): number { return 3; }
  static unit() { return new Circle(1); }
}
export function helper() {}
enum Color { Red }
namespace Util { export const x = 1; }
export abstract class Base { abstract go(): void; }
"#;
        let found = symbols(AstLanguage::TypeScript, ts, &SymbolFilter::default()).unwrap();
        assert_eq!(
            names(&found),
            vec![
                "interface Shape@2-2",
                "method area@2-2",
                "type Pair@3-3",
                "const add@4-4",
                "const mul@5-5",
                "class Circle@6-10",
                "method constructor@7-7",
                "method area@8-8",
                "method unit@9-9",
                "function helper@11-11",
                "enum Color@12-12",
                "namespace Util@13-13",
                "class Base@14-14",
                "method go@14-14",
            ]
        );
        let js =
            "function f() {}\nclass K { m() {} }\nconst g = () => 1;\nconst h = function () {};\n";
        let found = symbols(AstLanguage::JavaScript, js, &SymbolFilter::default()).unwrap();
        assert_eq!(
            names(&found),
            vec![
                "function f@1-1",
                "class K@2-2",
                "method m@2-2",
                "const g@3-3",
                "const h@4-4"
            ]
        );
        let tsx = "export const App = () => <div />;\nfunction Inner() { return null; }\n";
        let found = symbols(AstLanguage::Tsx, tsx, &SymbolFilter::default()).unwrap();
        assert_eq!(names(&found), vec!["const App@1-1", "function Inner@2-2"]);
    }

    #[test]
    fn go_java_and_c_symbols() {
        let go = "package a\n\ntype S struct{}\n\nfunc (s *S) M() {}\n\nfunc F() {}\nconst V = 1\nvar W = 2\n";
        let found = symbols(AstLanguage::Go, go, &SymbolFilter::default()).unwrap();
        assert_eq!(
            names(&found),
            vec![
                "type S@3-3",
                "method M@5-5",
                "func F@7-7",
                "const V@8-8",
                "var W@9-9"
            ]
        );

        let java = "public class A {\n    private int x;\n    public static void main(String[] args) {\n    }\n    A(int x) {}\n}\ninterface I { void go(); }\n";
        let found = symbols(AstLanguage::Java, java, &SymbolFilter::default()).unwrap();
        assert_eq!(
            names(&found),
            vec![
                "class A@1-6",
                "field x@2-2",
                "method main@3-4",
                "constructor A@5-5",
                "interface I@7-7",
                "method go@7-7",
            ]
        );

        let c = "#define MAX 10\n#define SQ(x) ((x)*(x))\ntypedef struct node node_t;\nstruct node { int v; };\nstatic int add(int a, int b) {\n    return a + b;\n}\nint *alloc(void) { return 0; }\nint main(void) {\n  if (x) {\n  }\n}\n";
        let found = symbols(AstLanguage::C, c, &SymbolFilter::default()).unwrap();
        assert_eq!(
            names(&found),
            vec![
                "define MAX@1-1",
                "define SQ@2-2",
                "typedef node_t@3-3",
                "struct node@4-4",
                "function add@5-7",
                "function alloc@8-8",
                "function main@9-12",
            ]
        );
    }

    #[test]
    fn raw_queries_return_captures_with_positions() {
        let source = "fn a() { helper(1); }\nfn b() { other(); helper(2); }\n";
        let query =
            "(call_expression function: (identifier) @callee (#eq? @callee \"helper\")) @call";
        let captures = run_query(AstLanguage::Rust, source, query).unwrap();
        assert_eq!(captures.len(), 4);
        let calls: Vec<_> = captures.iter().filter(|c| c.capture == "call").collect();
        assert_eq!(
            calls
                .iter()
                .map(|c| (c.start_line, c.start_col, c.text.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, 9, "helper(1)"), (2, 18, "helper(2)")]
        );
        assert_eq!(calls[0].match_index, 0);
        assert_eq!(calls[1].match_index, 1);
        assert_eq!(calls[0].node_kind, "call_expression");
        assert_eq!(
            captures.iter().filter(|c| c.capture == "callee").count(),
            2,
            "predicates filter out the `other()` call"
        );
    }

    #[test]
    fn invalid_queries_and_oversized_sources_fail_clearly() {
        let err = run_query(AstLanguage::Rust, "fn a() {}", "(nonexistent_node) @x").unwrap_err();
        assert!(matches!(err, AstError::Query(_)), "{err}");
        assert!(err.to_string().contains("invalid tree-sitter query"));
        assert!(validate_query(AstLanguage::Python, "(function_definition) @f").is_ok());
        assert!(validate_query(AstLanguage::Python, "(function_definition @f").is_err());
        let huge = "x".repeat(MAX_AST_SOURCE_BYTES + 1);
        assert!(matches!(
            symbols(AstLanguage::Rust, &huge, &SymbolFilter::default()).unwrap_err(),
            AstError::SourceTooLarge(_)
        ));
    }

    #[test]
    fn formatting_is_aligned_and_indented() {
        let found = symbols(
            AstLanguage::Rust,
            "impl A {\n    fn b() {}\n}\n",
            &SymbolFilter::default(),
        )
        .unwrap();
        assert_eq!(
            format_symbols("src/a.rs", AstLanguage::Rust, &found, 3),
            "src/a.rs (rust, 3 lines)\n  1-3  impl A\n  2-2    fn b"
        );
        assert_eq!(
            format_symbols("e.rs", AstLanguage::Rust, &[], 0),
            "e.rs (rust, 0 lines)\n  (no definitions)"
        );
        let captures = run_query(AstLanguage::Rust, "fn a() {}", "(function_item) @f").unwrap();
        assert_eq!(
            format_captures("a.rs", &captures),
            "a.rs:1:1: @f (function_item) fn a() {}"
        );
    }

    #[test]
    fn signatures_are_truncated_on_char_boundaries() {
        let long = format!("fn {}() {{}}", "é".repeat(MAX_SNIPPET_BYTES));
        let found = symbols(AstLanguage::Rust, &long, &SymbolFilter::default()).unwrap();
        assert!(found[0].signature.ends_with('…'));
        assert!(found[0].signature.len() <= MAX_SNIPPET_BYTES + '…'.len_utf8());
    }
}
