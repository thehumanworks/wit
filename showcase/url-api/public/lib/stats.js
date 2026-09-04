/**
 * Repository / directory statistics computed from the slim recursive tree
 * alone — zero blob fetches, so `stats` costs the same as `tree`.
 *
 * Agents use this to decide where to look before reading anything: how big
 * the repo is in tokens, which directories and languages dominate, which
 * files are too large to `cat` whole.
 */

import { estimateTokens, humanBytes, humanTokens } from "./textops.js";

/** Extension → language label (lowercase extension, no dot). */
const LANGUAGE_BY_EXT = {
  rs: "Rust",
  py: "Python",
  pyi: "Python",
  js: "JavaScript",
  mjs: "JavaScript",
  cjs: "JavaScript",
  jsx: "JavaScript",
  ts: "TypeScript",
  mts: "TypeScript",
  cts: "TypeScript",
  tsx: "TypeScript",
  go: "Go",
  java: "Java",
  kt: "Kotlin",
  kts: "Kotlin",
  scala: "Scala",
  swift: "Swift",
  c: "C",
  h: "C",
  cc: "C++",
  cpp: "C++",
  cxx: "C++",
  hpp: "C++",
  hh: "C++",
  cs: "C#",
  rb: "Ruby",
  php: "PHP",
  ex: "Elixir",
  exs: "Elixir",
  erl: "Erlang",
  hs: "Haskell",
  ml: "OCaml",
  mli: "OCaml",
  clj: "Clojure",
  cljs: "Clojure",
  lua: "Lua",
  r: "R",
  jl: "Julia",
  dart: "Dart",
  zig: "Zig",
  nim: "Nim",
  sh: "Shell",
  bash: "Shell",
  zsh: "Shell",
  fish: "Shell",
  ps1: "PowerShell",
  sql: "SQL",
  html: "HTML",
  htm: "HTML",
  css: "CSS",
  scss: "SCSS",
  less: "Less",
  vue: "Vue",
  svelte: "Svelte",
  md: "Markdown",
  mdx: "Markdown",
  rst: "reStructuredText",
  txt: "Text",
  json: "JSON",
  jsonc: "JSON",
  yaml: "YAML",
  yml: "YAML",
  toml: "TOML",
  xml: "XML",
  proto: "Protobuf",
  graphql: "GraphQL",
  gql: "GraphQL",
  tf: "Terraform",
  dockerfile: "Dockerfile",
  wasm: "WebAssembly",
  lock: "Lockfile",
};

/** Names without an extension that still map to a language. */
const LANGUAGE_BY_NAME = {
  dockerfile: "Dockerfile",
  makefile: "Makefile",
  gnumakefile: "Makefile",
  "cmakelists.txt": "CMake",
  "cargo.lock": "Lockfile",
  "package-lock.json": "Lockfile",
  "yarn.lock": "Lockfile",
  "pnpm-lock.yaml": "Lockfile",
  "poetry.lock": "Lockfile",
  "go.sum": "Lockfile",
  license: "Text",
  readme: "Markdown",
};

/** Extensions that are never worth reading as text. */
const BINARY_EXT = new Set([
  "png", "jpg", "jpeg", "gif", "webp", "ico", "bmp", "tiff", "svgz", "pdf",
  "zip", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "jar", "war",
  "wasm", "so", "dylib", "dll", "exe", "bin", "o", "a", "class", "pyc",
  "woff", "woff2", "ttf", "otf", "eot", "mp3", "mp4", "wav", "ogg", "mov",
  "avi", "webm", "sqlite", "db", "parquet",
]);

/**
 * @param {string} path
 * @returns {{ name: string, ext: string, language: string, binary: boolean }}
 */
export function classifyPath(path) {
  const name = String(path).split("/").pop() || String(path);
  const lower = name.toLowerCase();
  const dot = lower.lastIndexOf(".");
  const ext = dot > 0 ? lower.slice(dot + 1) : "";
  let language = LANGUAGE_BY_NAME[lower] ?? LANGUAGE_BY_EXT[ext] ?? null;
  if (!language) language = ext ? ext : "(no extension)";
  return { name, ext, language, binary: BINARY_EXT.has(ext) };
}

/**
 * Whether `path` sits under `base` (`base` empty = repo root).
 * @param {string} path
 * @param {string} base
 */
function under(path, base) {
  return base === "" || path === base || path.startsWith(`${base}/`);
}

/**
 * @typedef {{ path: string, type: string, size?: number }} SlimRow
 * @typedef {{
 *   path: string,
 *   files: number,
 *   bytes: number,
 *   tokens_est: number,
 *   directories: Array<{ name: string, files: number, bytes: number, tokens_est: number }>,
 *   languages: Array<{ language: string, files: number, bytes: number, tokens_est: number }>,
 *   largest_files: Array<{ path: string, bytes: number, tokens_est: number, binary: boolean }>,
 *   binary_files: number,
 *   max_depth: number,
 * }} RepoStats
 */

/**
 * Compute stats for the subtree rooted at `base` from slim tree rows.
 *
 * @param {SlimRow[]} rows
 * @param {string} base repo-relative directory ("" = root)
 * @param {{ largest?: number, isIgnored?: (path: string) => boolean }} [opts]
 * @returns {RepoStats}
 */
export function computeStats(rows, base, opts = {}) {
  const largestN = Math.max(0, opts.largest ?? 10);
  const isIgnored = opts.isIgnored ?? (() => false);
  /** @type {Map<string, { files: number, bytes: number }>} */
  const dirs = new Map();
  /** @type {Map<string, { files: number, bytes: number }>} */
  const langs = new Map();
  /** @type {Array<{ path: string, bytes: number, binary: boolean }>} */
  const files = [];
  let bytes = 0;
  let binaryFiles = 0;
  let maxDepth = 0;
  const baseDepth = base === "" ? 0 : base.split("/").length;

  for (const row of rows) {
    if (row.type !== "blob" || !under(row.path, base)) continue;
    if (isIgnored(row.path)) continue;
    const size = Number(row.size) || 0;
    const relative = base === "" ? row.path : row.path.slice(base.length + 1);
    const segments = relative.split("/");
    maxDepth = Math.max(maxDepth, segments.length - 1 + baseDepth);
    const { language, binary } = classifyPath(row.path);
    bytes += size;
    if (binary) binaryFiles += 1;
    files.push({ path: row.path, bytes: size, binary });

    const top = segments.length > 1 ? `${segments[0]}/` : ".";
    const dir = dirs.get(top) ?? { files: 0, bytes: 0 };
    dir.files += 1;
    dir.bytes += size;
    dirs.set(top, dir);

    const lang = langs.get(language) ?? { files: 0, bytes: 0 };
    lang.files += 1;
    lang.bytes += size;
    langs.set(language, lang);
  }

  const withTokens = ([name, v]) => ({ name, ...v, tokens_est: estimateTokens(v.bytes) });
  const byBytesDesc = (a, b) => b.bytes - a.bytes || a.name.localeCompare(b.name);

  return {
    path: base === "" ? "." : base,
    files: files.length,
    bytes,
    tokens_est: estimateTokens(bytes),
    directories: [...dirs].map(withTokens).sort(byBytesDesc),
    languages: [...langs]
      .map(withTokens)
      .sort(byBytesDesc)
      .map(({ name, ...rest }) => ({ language: name, ...rest })),
    largest_files: files
      .sort((a, b) => b.bytes - a.bytes || a.path.localeCompare(b.path))
      .slice(0, largestN)
      .map((f) => ({ ...f, tokens_est: estimateTokens(f.bytes) })),
    binary_files: binaryFiles,
    max_depth: maxDepth,
  };
}

/**
 * Plaintext rendering of stats (column-aligned, no colour).
 * @param {RepoStats} stats
 * @param {{ repo: string, ref: string, commit: string }} provenance
 */
export function formatStats(stats, provenance) {
  const lines = [];
  lines.push(`${provenance.repo} @ ${provenance.commit.slice(0, 7)} (${provenance.ref})`);
  lines.push(`path: ${stats.path}`);
  lines.push(
    `files: ${stats.files}  bytes: ${humanBytes(stats.bytes)}  ${humanTokens(stats.tokens_est)}` +
      `  binary: ${stats.binary_files}  max depth: ${stats.max_depth}`,
  );
  if (stats.files === 0) return lines.join("\n");

  const table = (title, rows, label) => {
    if (rows.length === 0) return;
    lines.push("");
    lines.push(title);
    const width = Math.max(...rows.map((r) => label(r).length));
    for (const r of rows) {
      lines.push(
        `  ${label(r).padEnd(width)}  ${String(r.files).padStart(6)} files  ` +
          `${humanBytes(r.bytes).padStart(9)}  ${humanTokens(r.tokens_est)}`,
      );
    }
  };
  table("by directory:", stats.directories, (r) => r.name);
  table("by language:", stats.languages, (r) => r.language);

  if (stats.largest_files.length > 0) {
    lines.push("");
    lines.push("largest files:");
    const width = Math.max(...stats.largest_files.map((f) => f.path.length));
    for (const f of stats.largest_files) {
      const tag = f.binary ? "  [bin]" : "";
      lines.push(
        `  ${f.path.padEnd(width)}  ${humanBytes(f.bytes).padStart(9)}  ${humanTokens(f.tokens_est)}${tag}`,
      );
    }
  }
  return lines.join("\n");
}
