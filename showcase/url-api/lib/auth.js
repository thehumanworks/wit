/**
 * Auth for the URL API host adapter.
 * Authorization header wins; ?token= is fallback.
 * Tokens must never appear in logs, traces, or error bodies.
 */

const TOKEN_QUERY_KEYS = new Set(["token", "access_token"]);

/**
 * Strip secrets from a string (query tokens, bearer values).
 * @param {string} text
 */
export function scrubSecrets(text) {
  if (!text) return text;
  let out = String(text);
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
  // Standalone github_pat_ / ghp_ tokens if they leak into messages
  out = out.replace(/\b(?:github_pat_|ghp_|gho_|ghu_|ghs_|ghr_)[A-Za-z0-9_]+/g, "[REDACTED]");
  return out;
}

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
