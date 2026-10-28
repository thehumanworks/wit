/**
 * Auth for the URL API host adapter.
 * Authorization header wins; ?token= is fallback.
 * Tokens must never appear in logs, traces, or error bodies.
 */

const TOKEN_QUERY_KEYS = new Set(["token", "access_token"]);

/**
 * Request-owned redaction context. No module/global credential registry.
 * The context is cleared on success or failure; late callbacks fail closed.
 * @template T
 * @param {Array<string | null | undefined>} secrets
 * @param {(scope: { scrub: (text: string) => string, console: typeof safeConsole }) => T | Promise<T>} fn
 * @returns {Promise<T>}
 */
export async function withActiveSecrets(secrets, fn) {
  const values = secrets.filter((s) => typeof s === "string" && s.length > 0);
  let closed = false;
  const scrub = (text) => closed ? "[REDACTED]" : scrubSecrets(text, values);
  const scope = { scrub, console: createSafeConsole(scrub) };
  try {
    return await fn(scope);
  } finally {
    values.fill("");
    values.length = 0;
    closed = true;
  }
}

/** Collect sensitive request values for exact, request-local redaction only. */
export function requestSecrets(request, serverToken = null) {
  const url = new URL(request.url);
  const values = [serverToken, extractToken({ headers: request.headers, url })];
  for (const key of TOKEN_QUERY_KEYS) values.push(...url.searchParams.getAll(key));
  for (const name of ["authorization", "proxy-authorization", "cookie"]) {
    const value = request.headers.get(name);
    if (!value) continue;
    values.push(value);
    if (name === "cookie") {
      for (const part of value.split(";")) {
        const separator = part.indexOf("=");
        if (separator >= 0) {
          const cookie = part.slice(separator + 1).trim();
          values.push(cookie);
          if (cookie.startsWith('"') && cookie.endsWith('"')) values.push(cookie.slice(1, -1));
        }
      }
    } else {
      // Include the opaque credential after any scheme (Basic is never forwarded).
      values.push(value.replace(/^\S+\s+/, ""));
    }
  }
  return values;
}

/**
 * @param {unknown} value
 */
function formatLogArg(value) {
  if (typeof value === "string") return value;
  if (value instanceof Error) {
    return value.stack || value.message || String(value);
  }
  if (value == null) return String(value);
  if (typeof value === "object") {
    try {
      return JSON.stringify(value);
    } catch {
      return Object.prototype.toString.call(value);
    }
  }
  return String(value);
}

/**
 * Strip secrets from a string (query tokens, bearer values, active PATs).
 * @param {string} text
 */
export function scrubSecrets(text, secrets = []) {
  if (text == null) return text;
  let out = String(text);
  // Exact active secrets first (header PAT, query PAT, etc.)
  for (const secret of secrets) {
    if (secret && out.includes(secret)) {
      out = out.split(secret).join("[REDACTED]");
    }
  }
  // Header-shaped diagnostics, including JSON objects, must not expose any scheme
  // or cookie values. Request contexts additionally redact exact opaque values.
  out = out.replace(/((?:["']?(?:authorization|proxy-authorization|cookie|set-cookie)["']?)\s*:\s*)(?:"(?:\\.|[^"\\\r\n])*"|'(?:\\.|[^'\\\r\n])*'|[^\r\n]+)/gi, "$1[REDACTED]");
  // ?token=... / &token=... (and access_token)
  out = out.replace(
    /([?&](?:token|access_token)=)([^&#\s]*)/gi,
    "$1[REDACTED]",
  );
  // Authorization: Bearer|token <pat>
  out = out.replace(
    /(Authorization\s*:\s*(?:Bearer|token)\s+)(\S+)/gi,
    "$1[REDACTED]",
  );
  // Bare Bearer|token <pat> in free-form log lines
  out = out.replace(
    /(\b(?:Bearer|token)\s+)(\S+)/gi,
    "$1[REDACTED]",
  );
  // Standalone github_pat_ / ghp_ tokens if they leak into messages
  out = out.replace(/\b(?:github_pat_|ghp_|gho_|ghu_|ghs_|ghr_)[A-Za-z0-9_]+/g, "[REDACTED]");
  return out;
}

/**
 * Scrub every console argument (strings, Errors, objects).
 * @param {unknown[]} args
 */
export function scrubLogArgs(args) {
  return args.map((arg) => scrubSecrets(formatLogArg(arg)));
}

/**
 * console.* wrappers that always scrubSecrets every argument.
 * Use these for every log line in the showcase host.
 */
function createSafeConsole(scrub) {
  return Object.fromEntries(["log", "info", "warn", "error", "debug"].map((level) => [
    level, (...args) => console[level](...args.map((arg) => scrub(formatLogArg(arg)))),
  ]));
}
export const safeConsole = createSafeConsole(scrubSecrets);

/**
 * Safe Error whose message is scrubbed.
 */
export class SafeError extends Error {
  /**
   * @param {string} message
   * @param {{ status?: number, code?: string }} [opts]
   */
  constructor(message, opts = {}) {
    super(scrubSecrets(message));
    this.name = "SafeError";
    this.status = opts.status ?? 500;
    this.code = opts.code;
  }
}

/**
 * Extract PAT from request.
 * Prefer Authorization: Bearer <pat> or Authorization: token <pat>.
 * Fall back to ?token= (leaks via logs/Referer — callers must document that).
 *
 * @param {{ headers?: Headers | Record<string,string>, url?: string|URL }} req
 * @returns {string | null}
 */
export function extractToken(req) {
  const headers = req.headers;
  let auth = null;
  if (headers && typeof headers.get === "function") {
    auth = headers.get("Authorization") || headers.get("authorization");
  } else if (headers && typeof headers === "object") {
    auth =
      headers.Authorization ||
      headers.authorization ||
      headers.AUTHORIZATION ||
      null;
  }
  if (auth) {
    const m = String(auth).match(/^\s*(?:Bearer|token)\s+(\S+)\s*$/i);
    if (m) return m[1];
    // Raw PAT in Authorization without scheme
    const raw = String(auth).trim();
    if (raw && !/\s/.test(raw)) return raw;
  }

  const url = req.url ? new URL(String(req.url), "http://local") : null;
  if (url) {
    for (const key of TOKEN_QUERY_KEYS) {
      const v = url.searchParams.get(key);
      if (v) return v;
    }
  }
  return null;
}

/**
 * Build GitHub Authorization header value from a PAT (never log this).
 * @param {string | null | undefined} token
 */
export function githubAuthHeader(token) {
  if (!token) return null;
  return `Bearer ${token}`;
}
