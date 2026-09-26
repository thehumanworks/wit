/**
 * Limits (ADR 0009). Defaults are sized so the cache stays inside the Workers
 * Paid included allowances and the account-wide R2 free tier; every value can
 * be overridden with a wrangler `[vars]` entry of the same name.
 */

const MiB = 1024 * 1024;

export const DEFAULTS = Object.freeze({
  /** Largest pack a fill will store (torvalds/linux at depth 1 fits). */
  MAX_PACK_BYTES: 512 * MiB,
  /** Hard cap on stored bytes; the oldest packs are evicted beyond it. */
  STORAGE_CAP_BYTES: 3_000_000_000,
  /** Fills started per UTC day, across all callers. */
  DAILY_FILL_LIMIT: 300,
  /** Upstream pack bytes streamed per UTC day (bounds Worker CPU). */
  DAILY_FILL_BYTES: 8_000_000_000,
  /** Fills running at the same time. */
  MAX_INFLIGHT_FILLS: 4,
  /** R2 multipart part size (R2 minimum is 5 MiB). */
  PART_BYTES: 16 * MiB,
  /** Wall-clock budget for one fill (queue consumers allow 15 minutes). */
  FILL_TIMEOUT_MS: 300_000,
  /** Matches the R2 lifecycle rule on the bucket. */
  RETENTION_DAYS: 30,
});

/**
 * @param {Record<string, unknown>} env
 * @returns {typeof DEFAULTS}
 */
export function limitsFromEnv(env) {
  /** @type {Record<string, number>} */
  const out = { ...DEFAULTS };
  for (const name of Object.keys(DEFAULTS)) {
    const raw = env?.[name];
    if (raw == null || raw === "") continue;
    const n = Number(raw);
    if (Number.isFinite(n) && n > 0) out[name] = n;
  }
  return /** @type {typeof DEFAULTS} */ (out);
}

/** Negative-cache lifetimes in seconds, by fill failure reason. */
export const NEGATIVE_TTL = Object.freeze({
  not_found_or_private: 3600,
  too_large: 7 * 86400,
  rate_limited: 600,
  not_tip: 600,
  upstream_error: 600,
  bad_pack: 600,
  timeout: 3600,
  blocked: 30 * 86400,
});

/** Reasons cached per repository (every commit); the rest are per commit. */
export const REPO_SCOPED = new Set(["not_found_or_private", "too_large", "blocked"]);

/** A pending fill older than this is presumed lost (consumer crash). */
export const PENDING_TTL_SECONDS = 20 * 60;
