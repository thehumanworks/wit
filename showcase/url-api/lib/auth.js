/**
 * Auth for the URL API host adapter.
 * Authorization header wins; ?token= is fallback.
 * Tokens must never appear in logs, traces, or error bodies.
 */

const TOKEN_QUERY_KEYS = new Set(["token", "access_token"]);

/** @type {string[]} */
let activeSecrets = [];

/**
 * Run `fn` while treating `secrets` as redaction targets for scrubSecrets / safeConsole.
 * Nested calls restore the previous set. Awaits async `fn` so secrets stay active
 * through the full request (including catch-path console.error).
 * @template T
 * @param {Array<string | null | undefined>} secrets
 * @param {() => T | Promise<T>} fn
 * @returns {Promise<T>}
 */
export async function withActiveSecrets(secrets, fn) {
  const prev = activeSecrets;
  activeSecrets = secrets.filter((s) => typeof s === "string" && s.length > 0);
  try {
    return await fn();
  } finally {
    activeSecrets = prev;
  }
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
export function scrubSecrets(text) {
  if (text == null) return text;
  let out = String(text);
  // Exact active secrets first (header PAT, query PAT, etc.)
  for (const secret of activeSecrets) {
    if (secret && out.includes(secret)) {
      out = out.split(secret).join("[REDACTED]");
    }
  }
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
export const safeConsole = {
  /** @param {...unknown} args */
  log(...args) {
    console.log(...scrubLogArgs(args));
  },
  /** @param {...unknown} args */
  info(...args) {
    console.info(...scrubLogArgs(args));
  },
  /** @param {...unknown} args */
  warn(...args) {
    console.warn(...scrubLogArgs(args));
  },
  /** @param {...unknown} args */
  error(...args) {
    console.error(...scrubLogArgs(args));
  },
  /** @param {...unknown} args */
  debug(...args) {
    console.debug(...scrubLogArgs(args));
  },
};

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
