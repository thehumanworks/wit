/**
 * Pure text helpers shared by the read-shaped verbs (cat ranges, head, tail,
 * rg). All line numbers are one-based and inclusive, matching the CLI and
 * the MCP `wit_read` contract.
 */

/** Rust `str::lines()`: split on `\n` / `\r\n`, drop one trailing empty line. */
export function rustLines(content) {
  const text = String(content ?? "");
  if (text === "") return [];
  const lines = text.split(/\r?\n/);
  if (lines.length && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/**
 * Rough token estimate used across the API (~4 bytes per token). The CLI's
 * `-l` uses lines × 5 for text it has read; this one needs only sizes so it
 * works from tree metadata without fetching blobs.
 * @param {number} bytes
 */
export function estimateTokens(bytes) {
  const n = Number(bytes);
  if (!Number.isFinite(n) || n <= 0) return 0;
  return Math.ceil(n / 4);
}

/**
 * Parse `?lines=A-B` (also `A-`, `-B`, `A`) into a one-based inclusive range.
 * Returns null for an empty value; throws a plain Error on malformed input.
 *
 * @param {string | null | undefined} value
 * @returns {{ start: number | null, end: number | null } | null}
 */
export function parseLineRange(value) {
  if (value == null) return null;
  const text = String(value).trim();
  if (text === "") return null;
  const m = text.match(/^(\d*)\s*[-:]\s*(\d*)$/) ?? text.match(/^(\d+)()$/);
  if (!m || (m[1] === "" && m[2] === "")) {
    throw new Error(`lines must look like START-END, got '${text}'`);
  }
  const start = m[1] === "" ? null : Number(m[1]);
  const end = m[2] === "" ? (m[1] !== "" && !/[-:]/.test(text) ? start : null) : Number(m[2]);
  if ((start != null && start < 1) || (end != null && end < 1)) {
    throw new Error("line numbers are one-based");
  }
  if (start != null && end != null && end < start) {
    throw new Error(`lines end (${end}) is before start (${start})`);
  }
  return { start, end };
}

/**
 * Select an inclusive one-based line range from file text.
 *
 * @param {string} content
 * @param {{ start?: number | null, end?: number | null } | null} [range]
 * @returns {{ lines: string[], start: number, end: number, total: number }}
 */
export function sliceLines(content, range) {
  const all = rustLines(content);
  const total = all.length;
  const start = Math.max(1, range?.start ?? 1);
  const end = Math.min(total, range?.end ?? total);
  if (total === 0 || start > total || end < start) {
    return { lines: [], start: Math.min(start, total + 1), end: Math.min(end, total), total };
  }
  return { lines: all.slice(start - 1, end), start, end, total };
}

/**
 * `{:>6}  {line}` numbering, identical to `head -N` / `cat -n` in the CLI.
 * @param {string[]} lines
 * @param {number} startLineNum
 * @param {boolean} number
 */
export function numberLines(lines, startLineNum, number) {
  if (!number) return lines.join("\n");
  return lines
    .map((line, i) => `${String(startLineNum + i).padStart(6, " ")}  ${line}`)
    .join("\n");
}

/** `head_from_text` in crates/wit/src/snapshot/memory_ops.rs */
export function headFromText(content, count, number = false) {
  return numberLines(rustLines(content).slice(0, Math.max(0, count)), 1, number);
}

/** `tail_from_text` in crates/wit/src/snapshot/memory_ops.rs */
export function tailFromText(content, count, fromLine = null, number = false) {
  const all = rustLines(content);
  const total = all.length;
  let selected;
  let startLineNum;
  if (fromLine != null) {
    const skip = Math.max(0, fromLine - 1);
    selected = all.slice(skip);
    startLineNum = fromLine;
  } else {
    const skip = Math.max(0, total - Math.max(0, count));
    selected = all.slice(skip);
    startLineNum = skip + 1;
  }
  return numberLines(selected, startLineNum, number);
}

/**
 * Compile a user regex for rg. Rust regex syntax and JavaScript's overlap for
 * everything agents type in practice; unsupported constructs fail closed.
 *
 * @param {string} pattern
 * @param {{ ignoreCase?: boolean, smartCase?: boolean, wordRegexp?: boolean }} [opts]
 */
export function compileSearchRegex(pattern, opts = {}) {
  let source = String(pattern ?? "");
  if (source === "") throw new Error("rg needs a non-empty pattern (?q=)");
  if (opts.wordRegexp) source = `\\b(?:${source})\\b`;
  let flags = "";
  const ignoreCase =
    opts.ignoreCase || (opts.smartCase && source === source.toLowerCase());
  if (ignoreCase) flags += "i";
  try {
    return new RegExp(source, flags);
  } catch (err) {
    throw new Error(`invalid rg pattern: ${err.message || err}`);
  }
}

/**
 * @typedef {{ path: string, line: number, text: string, is_context: boolean }} GrepLine
 */

/**
 * Search one file's text. Returns match and context lines in file order with
 * `--` separators between non-adjacent groups (same shape as the CLI's
 * GrepMatch stream: a separator is `{ line: 0, text: "--", is_context: true }`).
 *
 * @param {string} path
 * @param {string} content
 * @param {RegExp} regex
 * @param {{ invert?: boolean, before?: number, after?: number, maxMatches?: number }} [opts]
 * @returns {{ lines: GrepLine[], matchCount: number }}
 */
export function grepText(path, content, regex, opts = {}) {
  const lines = rustLines(content);
  const before = Math.max(0, opts.before ?? 0);
  const after = Math.max(0, opts.after ?? 0);
  const maxMatches = opts.maxMatches ?? Infinity;
  /** @type {number[]} */
  const hits = [];
  for (let i = 0; i < lines.length; i += 1) {
    regex.lastIndex = 0;
    const matched = regex.test(lines[i]);
    if (matched !== Boolean(opts.invert)) {
      hits.push(i);
      if (hits.length >= maxMatches) break;
    }
  }
  if (hits.length === 0) return { lines: [], matchCount: 0 };
  if (before === 0 && after === 0) {
    return {
      lines: hits.map((i) => ({ path, line: i + 1, text: lines[i], is_context: false })),
      matchCount: hits.length,
    };
  }
  const hitSet = new Set(hits);
  /** @type {GrepLine[]} */
  const out = [];
  let lastEmitted = -1;
  for (const hit of hits) {
    const from = Math.max(0, hit - before);
    const to = Math.min(lines.length - 1, hit + after);
    if (lastEmitted >= 0 && from > lastEmitted + 1) {
      out.push({ path, line: 0, text: "--", is_context: true });
    }
    for (let i = Math.max(from, lastEmitted + 1); i <= to; i += 1) {
      out.push({ path, line: i + 1, text: lines[i], is_context: !hitSet.has(i) });
    }
    lastEmitted = Math.max(lastEmitted, to);
  }
  return { lines: out, matchCount: hits.length };
}

/**
 * Minimal git-style glob → RegExp (`*`, `**`, `?`, `{a,b}`), anchored to the
 * whole repo-relative path. A glob without a slash matches the basename, like
 * ripgrep's `-g`.
 * @param {string} glob
 */
export function globToRegExp(glob) {
  const text = String(glob ?? "").trim();
  if (text === "") return null;
  const basenameOnly = !text.includes("/");
  let re = "";
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i];
    if (ch === "*") {
      if (text[i + 1] === "*") {
        i += 1;
        if (text[i + 1] === "/") {
          i += 1;
          re += "(?:.*/)?";
        } else {
          re += ".*";
        }
      } else {
        re += "[^/]*";
      }
    } else if (ch === "?") {
      re += "[^/]";
    } else if (ch === "{") {
      const close = text.indexOf("}", i);
      if (close === -1) {
        re += "\\{";
      } else {
        const alts = text
          .slice(i + 1, close)
          .split(",")
          .map((alt) => alt.replace(/[.+^$()|[\]\\]/g, "\\$&").replace(/\*/g, "[^/]*"));
        re += `(?:${alts.join("|")})`;
        i = close;
      }
    } else if (/[.+^$()|[\]\\]/.test(ch)) {
      re += `\\${ch}`;
    } else {
      re += ch;
    }
  }
  return new RegExp(basenameOnly ? `(?:^|/)${re}$` : `^${re}$`);
}

/**
 * Bytes → human label used in stats plaintext (`1.2 MB`, `840 B`).
 * @param {number} bytes
 */
export function humanBytes(bytes) {
  const n = Number(bytes) || 0;
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

/**
 * Compact token label (`~1.2k tok`, `~35k tok`, `~2.1M tok`).
 * @param {number} tokens
 */
export function humanTokens(tokens) {
  const n = Number(tokens) || 0;
  if (n < 1000) return `~${n} tok`;
  if (n < 1_000_000) return `~${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k tok`;
  return `~${(n / 1_000_000).toFixed(1)}M tok`;
}
