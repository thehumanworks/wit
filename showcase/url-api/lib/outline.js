/**
 * Regex-based symbol outline for one file: a line-numbered index of
 * definitions so an agent can pick an exact `?lines=A-B` range instead of
 * reading a large file whole. Deterministic, language-aware by extension,
 * and honest about being a heuristic (no AST; `end_line` is approximate).
 */

import { rustLines } from "./textops.js";

/**
 * @typedef {{ kind: string, re: RegExp, name: number }} Rule
 * name = capture group index holding the symbol name.
 */

/** @type {Rule[]} */
const RUST = [
  { kind: "fn", re: /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+|async\s+|unsafe\s+|extern\s+"[^"]*"\s+)*fn\s+([A-Za-z_][A-Za-z0-9_]*)/, name: 1 },
  { kind: "struct", re: /^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)/, name: 1 },
  { kind: "enum", re: /^\s*(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)/, name: 1 },
  { kind: "trait", re: /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?trait\s+([A-Za-z_][A-Za-z0-9_]*)/, name: 1 },
  { kind: "impl", re: /^\s*(?:unsafe\s+)?impl(?:<[^>]*>)?\s+(.+?)\s*(?:where\b.*)?\{?\s*$/, name: 1 },
  { kind: "mod", re: /^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)/, name: 1 },
  { kind: "type", re: /^\s*(?:pub(?:\([^)]*\))?\s+)?type\s+([A-Za-z_][A-Za-z0-9_]*)/, name: 1 },
  { kind: "const", re: /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+([A-Za-z_][A-Za-z0-9_]*)\s*:/, name: 1 },
  { kind: "macro", re: /^\s*macro_rules!\s+([A-Za-z_][A-Za-z0-9_]*)/, name: 1 },
];

/** @type {Rule[]} */
const PYTHON = [
  { kind: "class", re: /^\s*class\s+([A-Za-z_][A-Za-z0-9_]*)/, name: 1 },
  { kind: "def", re: /^\s*(?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)/, name: 1 },
];

/** @type {Rule[]} */
const JAVASCRIPT = [
  { kind: "class", re: /^\s*(?:export\s+(?:default\s+)?)?(?:abstract\s+)?class\s+([A-Za-z_$][\w$]*)/, name: 1 },
  { kind: "function", re: /^\s*(?:export\s+(?:default\s+)?)?(?:async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)/, name: 1 },
  { kind: "const", re: /^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::[^=]+)?=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*(?::[^=]+)?=>/, name: 1 },
  { kind: "interface", re: /^\s*(?:export\s+)?interface\s+([A-Za-z_$][\w$]*)/, name: 1 },
  { kind: "type", re: /^\s*(?:export\s+)?type\s+([A-Za-z_$][\w$]*)\s*(?:<[^=]*>)?\s*=/, name: 1 },
  { kind: "enum", re: /^\s*(?:export\s+)?(?:const\s+)?enum\s+([A-Za-z_$][\w$]*)/, name: 1 },
  { kind: "export", re: /^\s*export\s+(?:const|let|var)\s+([A-Za-z_$][\w$]*)/, name: 1 },
  { kind: "method", re: /^\s{2,}(?:(?:public|private|protected|static|readonly|async|override|get|set)\s+)*([A-Za-z_$][\w$]*)\s*(?:<[^>]*>)?\([^)]*\)\s*(?::\s*[^{]+)?\{\s*$/, name: 1 },
];

/** @type {Rule[]} */
const GO = [
  { kind: "func", re: /^func\s+(?:\([^)]*\)\s*)?([A-Za-z_][A-Za-z0-9_]*)/, name: 1 },
  { kind: "type", re: /^type\s+([A-Za-z_][A-Za-z0-9_]*)/, name: 1 },
  { kind: "var", re: /^(?:var|const)\s+([A-Za-z_][A-Za-z0-9_]*)/, name: 1 },
];

/** Java / Kotlin / C# / Scala style. */
/** @type {Rule[]} */
const JVM = [
  { kind: "class", re: /^\s*(?:(?:public|private|protected|internal|abstract|final|static|sealed|open|data|partial|export)\s+)*(?:class|interface|enum|record|object|trait|struct)\s+([A-Za-z_][\w]*)/, name: 1 },
  { kind: "method", re: /^\s+(?:(?:public|private|protected|internal|static|final|abstract|override|open|suspend|async|virtual|synchronized|inline|operator)\s+)*(?:fun\s+|def\s+|[\w<>\[\],.?]+\s+)([A-Za-z_][\w]*)\s*\([^;]*$/, name: 1 },
  { kind: "constructor", re: /^\s+(?:(?:public|private|protected|internal)\s+)*([A-Z][\w]*)\s*\([^;]*\)\s*(?:throws\s+[\w.,\s]+)?\{?\s*\}?\s*$/, name: 1 },
];

/** @type {Rule[]} */
const C_LIKE = [
  { kind: "define", re: /^\s*#\s*define\s+([A-Za-z_]\w*)/, name: 1 },
  { kind: "typedef", re: /^\s*typedef\s+.*?\b([A-Za-z_]\w*)\s*;\s*$/, name: 1 },
  { kind: "struct", re: /^\s*(?:typedef\s+)?(?:struct|class|union|enum)\s+([A-Za-z_]\w*)/, name: 1 },
  { kind: "function", re: /^(?!\s*(?:if|for|while|switch|return|else)\b)[A-Za-z_][\w\s*&:<>,~]*?[\s*&]([A-Za-z_][\w:~]*)\s*\([^;{}]*\)\s*(?:const)?\s*(?:noexcept)?\s*(?:override)?\s*\{?\s*$/, name: 1 },
];

/** @type {Rule[]} */
const RUBY = [
  { kind: "class", re: /^\s*(?:class|module)\s+([A-Z][\w:]*)/, name: 1 },
  { kind: "def", re: /^\s*def\s+(self\.)?([A-Za-z_][\w?!=]*)/, name: 2 },
];

/** @type {Rule[]} */
const PHP = [
  { kind: "class", re: /^\s*(?:abstract\s+|final\s+)?(?:class|interface|trait|enum)\s+([A-Za-z_]\w*)/, name: 1 },
  { kind: "function", re: /^\s*(?:(?:public|private|protected|static|abstract|final)\s+)*function\s+&?([A-Za-z_]\w*)/, name: 1 },
];

/** @type {Rule[]} */
const SHELL = [
  { kind: "function", re: /^\s*(?:function\s+)?([A-Za-z_][\w-]*)\s*\(\)\s*\{?/, name: 1 },
  { kind: "function", re: /^\s*function\s+([A-Za-z_][\w-]*)/, name: 1 },
];

/** @type {Rule[]} */
const ELIXIR = [
  { kind: "module", re: /^\s*defmodule\s+([A-Z][\w.]*)/, name: 1 },
  { kind: "def", re: /^\s*(?:def|defp|defmacro)\s+([a-z_][\w?!]*)/, name: 1 },
];

/** @type {Rule[]} */
const MARKDOWN = [
  { kind: "heading", re: /^(#{1,6})\s+(.+?)\s*#*\s*$/, name: 2 },
];

/** @type {Rule[]} */
const TOML_INI = [
  { kind: "section", re: /^\s*\[\[?([^\]]+)\]\]?\s*$/, name: 1 },
];

/** @type {Rule[]} */
const YAML = [
  { kind: "key", re: /^([A-Za-z_][\w.-]*)\s*:/, name: 1 },
];

/** @type {Rule[]} */
const SQL = [
  { kind: "table", re: /^\s*create\s+(?:or\s+replace\s+)?(?:table|view|function|procedure|index|type)\s+(?:if\s+not\s+exists\s+)?([\w."]+)/i, name: 1 },
];

/** @type {Record<string, { language: string, rules: Rule[] }>} */
const BY_EXT = {
  rs: { language: "Rust", rules: RUST },
  py: { language: "Python", rules: PYTHON },
  pyi: { language: "Python", rules: PYTHON },
  js: { language: "JavaScript", rules: JAVASCRIPT },
  mjs: { language: "JavaScript", rules: JAVASCRIPT },
  cjs: { language: "JavaScript", rules: JAVASCRIPT },
  jsx: { language: "JavaScript", rules: JAVASCRIPT },
  ts: { language: "TypeScript", rules: JAVASCRIPT },
  mts: { language: "TypeScript", rules: JAVASCRIPT },
  cts: { language: "TypeScript", rules: JAVASCRIPT },
  tsx: { language: "TypeScript", rules: JAVASCRIPT },
  go: { language: "Go", rules: GO },
  java: { language: "Java", rules: JVM },
  kt: { language: "Kotlin", rules: JVM },
  kts: { language: "Kotlin", rules: JVM },
  scala: { language: "Scala", rules: JVM },
  cs: { language: "C#", rules: JVM },
  swift: { language: "Swift", rules: JVM },
  c: { language: "C", rules: C_LIKE },
  h: { language: "C", rules: C_LIKE },
  cc: { language: "C++", rules: C_LIKE },
  cpp: { language: "C++", rules: C_LIKE },
  cxx: { language: "C++", rules: C_LIKE },
  hpp: { language: "C++", rules: C_LIKE },
  hh: { language: "C++", rules: C_LIKE },
  rb: { language: "Ruby", rules: RUBY },
  php: { language: "PHP", rules: PHP },
  sh: { language: "Shell", rules: SHELL },
  bash: { language: "Shell", rules: SHELL },
  zsh: { language: "Shell", rules: SHELL },
  ex: { language: "Elixir", rules: ELIXIR },
  exs: { language: "Elixir", rules: ELIXIR },
  md: { language: "Markdown", rules: MARKDOWN },
  mdx: { language: "Markdown", rules: MARKDOWN },
  toml: { language: "TOML", rules: TOML_INI },
  ini: { language: "INI", rules: TOML_INI },
  cfg: { language: "INI", rules: TOML_INI },
  yaml: { language: "YAML", rules: YAML },
  yml: { language: "YAML", rules: YAML },
  sql: { language: "SQL", rules: SQL },
};

/**
 * Languages this outline understands, for discovery docs.
 */
export function outlineLanguages() {
  return [...new Set(Object.values(BY_EXT).map((v) => v.language))].sort();
}

/**
 * @param {string} path
 * @returns {{ language: string, rules: Rule[] } | null}
 */
export function rulesForPath(path) {
  const name = String(path).split("/").pop() || "";
  const lower = name.toLowerCase();
  if (lower === "dockerfile" || lower === "makefile") return null;
  const dot = lower.lastIndexOf(".");
  const ext = dot >= 0 ? lower.slice(dot + 1) : "";
  return BY_EXT[ext] ?? null;
}

/**
 * @typedef {{ line: number, end_line: number, kind: string, name: string, indent: number, signature: string }} Symbol
 */

/**
 * @param {string} line
 */
function indentOf(line) {
  const m = line.match(/^[ \t]*/);
  return m ? m[0].replace(/\t/g, "    ").length : 0;
}

/**
 * Extract symbols from `content` using the rules for `path`.
 *
 * @param {string} path
 * @param {string} content
 * @param {{ maxSymbols?: number }} [opts]
 * @returns {{ language: string | null, supported: boolean, symbols: Symbol[], total_lines: number, truncated: boolean }}
 */
export function outlineFile(path, content, opts = {}) {
  const lines = rustLines(content);
  const spec = rulesForPath(path);
  if (!spec) {
    return { language: null, supported: false, symbols: [], total_lines: lines.length, truncated: false };
  }
  const maxSymbols = Math.max(1, opts.maxSymbols ?? 2000);
  const isMarkdown = spec.rules === MARKDOWN;
  /** @type {Symbol[]} */
  const symbols = [];
  let inFence = false;
  let truncated = false;
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    if (isMarkdown) {
      if (/^\s*(```|~~~)/.test(line)) inFence = !inFence;
      if (inFence) continue;
    }
    for (const rule of spec.rules) {
      const m = line.match(rule.re);
      if (!m) continue;
      const name = String(m[rule.name] ?? "").trim();
      if (!name) continue;
      const indent = isMarkdown ? m[1].length - 1 : indentOf(line);
      symbols.push({
        line: i + 1,
        end_line: i + 1,
        kind: rule.kind,
        name,
        indent,
        signature: line.trim().slice(0, 160),
      });
      break;
    }
    if (symbols.length >= maxSymbols) {
      truncated = i + 1 < lines.length;
      break;
    }
  }
  // Approximate end_line: the line before the next symbol at the same or a
  // shallower indent (headings: same or higher level), else end of file.
  for (let s = 0; s < symbols.length; s += 1) {
    let end = lines.length;
    for (let t = s + 1; t < symbols.length; t += 1) {
      if (symbols[t].indent <= symbols[s].indent) {
        end = symbols[t].line - 1;
        break;
      }
    }
    symbols[s].end_line = Math.max(symbols[s].line, end);
  }
  return {
    language: spec.language,
    supported: true,
    symbols,
    total_lines: lines.length,
    truncated,
  };
}

/**
 * Plaintext: `START-END  kind  name` with nesting shown by indent.
 * @param {ReturnType<typeof outlineFile>} outline
 * @param {string} path
 */
export function formatOutline(outline, path) {
  if (!outline.supported) {
    return `outline: no rules for ${path} (supported: ${outlineLanguages().join(", ")})`;
  }
  if (outline.symbols.length === 0) {
    return `outline: no symbols found in ${path} (${outline.total_lines} lines)`;
  }
  const lines = [`${path} (${outline.language}, ${outline.total_lines} lines)`];
  const width = String(outline.total_lines).length;
  for (const sym of outline.symbols) {
    const range = `${String(sym.line).padStart(width)}-${String(sym.end_line).padEnd(width)}`;
    const depth = " ".repeat(Math.min(sym.indent, 24));
    lines.push(`  ${range}  ${depth}${sym.kind} ${sym.name}`);
  }
  if (outline.truncated) lines.push("  ... (symbol limit reached)");
  return lines.join("\n");
}
