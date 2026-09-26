/**
 * FillCoordinator: one global Durable Object (SQLite storage) that owns every
 * fill decision, so budgets and single-flight are strongly consistent:
 *
 * - pending fills (single-flight per repo@commit, max in flight)
 * - daily fill count and upstream byte budget
 * - negative cache (private/missing, too large, rate limited, not a tip)
 * - ledger of stored packs, used to evict the oldest beyond the storage cap
 *
 * It only sees misses and fill completions, never cache hits. It never sees
 * client IP addresses.
 */

import { NEGATIVE_TTL, PENDING_TTL_SECONDS, REPO_SCOPED, limitsFromEnv } from "./config.js";
import { packKey, repoId, repoPrefix } from "./keys.js";

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS packs (key TEXT PRIMARY KEY, repo TEXT NOT NULL, bytes INTEGER NOT NULL, filled_at INTEGER NOT NULL)`,
  `CREATE INDEX IF NOT EXISTS packs_by_age ON packs (filled_at)`,
  `CREATE TABLE IF NOT EXISTS negatives (scope TEXT PRIMARY KEY, reason TEXT NOT NULL, until INTEGER NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS pending (key TEXT PRIMARY KEY, since INTEGER NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS daily (day TEXT PRIMARY KEY, fills INTEGER NOT NULL, bytes INTEGER NOT NULL)`,
];

/** @param {number} ms */
function utcDay(ms) {
  return new Date(ms).toISOString().slice(0, 10);
}

export class FillCoordinator {
  /**
   * @param {{ storage: { sql: { exec: (q: string, ...b: unknown[]) => { toArray(): any[] } } } }} ctx
   * @param {{ PACKS: R2Bucket } & Record<string, unknown>} env
   */
  constructor(ctx, env) {
    this.sql = ctx.storage.sql;
    this.env = env;
    this.limits = limitsFromEnv(env);
    this.now = () => Date.now();
    for (const stmt of SCHEMA) this.sql.exec(stmt);
  }

  /** @param {string} q @param {...unknown} b */
  rows(q, ...b) {
    return this.sql.exec(q, ...b).toArray();
  }

  /** @param {Request} request */
  async fetch(request) {
    const path = new URL(request.url).pathname;
    const body = request.method === "POST" ? await request.json() : {};
    let result;
    switch (path) {
      case "/request-fill":
        result = this.requestFill(body);
        break;
      case "/release":
        result = this.release(body);
        break;
      case "/complete":
        result = await this.complete(body);
        break;
      case "/takedown":
        result = await this.takedown(body);
        break;
      case "/stats":
        result = this.stats();
        break;
      default:
        return new Response("not found", { status: 404 });
    }
    return Response.json(result);
  }

  /** @param {number} nowMs */
  prune(nowMs) {
    const now = Math.floor(nowMs / 1000);
    this.rows(`DELETE FROM negatives WHERE until <= ?`, now);
    this.rows(`DELETE FROM pending WHERE since <= ?`, now - PENDING_TTL_SECONDS);
    this.rows(`DELETE FROM daily WHERE day < ?`, utcDay(nowMs - 7 * 86400_000));
    // The bucket lifecycle rule deletes objects after RETENTION_DAYS; drop
    // their ledger rows a day later so stored-byte accounting stays honest.
    this.rows(`DELETE FROM packs WHERE filled_at <= ?`, now - (this.limits.RETENTION_DAYS + 1) * 86400);
  }

  /** @param {string} day */
  today(day) {
    const [row] = this.rows(`SELECT fills, bytes FROM daily WHERE day = ?`, day);
    return row ?? { fills: 0, bytes: 0 };
  }

  /** @param {string} day @param {number} fills @param {number} bytes */
  bumpDaily(day, fills, bytes) {
    this.rows(
      `INSERT INTO daily (day, fills, bytes) VALUES (?, ?, ?)
       ON CONFLICT(day) DO UPDATE SET fills = MAX(0, fills + excluded.fills), bytes = bytes + excluded.bytes`,
      day,
      fills,
      bytes,
    );
  }

  /** @param {string} repo @param {string} key */
  negativeFor(repo, key) {
    const now = Math.floor(this.now() / 1000);
    const [row] = this.rows(
      `SELECT reason, until FROM negatives WHERE scope IN (?, ?, 'global') AND until > ? ORDER BY until DESC LIMIT 1`,
      `repo:${repo}`,
      `key:${key}`,
      now,
    );
    return row ? { reason: row.reason, retryAfterSeconds: row.until - now } : null;
  }

  /**
   * Decide whether a miss may start a fill. Reserves the budget when it does.
   * @param {{ owner: string, repo: string, commit: string }} job
   */
  requestFill(job) {
    const nowMs = this.now();
    this.prune(nowMs);
    const repo = repoId(job.owner, job.repo);
    const key = packKey(job.owner, job.repo, job.commit);
    const negative = this.negativeFor(repo, key);
    if (negative) return { status: "skipped", ...negative };
    if (this.rows(`SELECT 1 FROM pending WHERE key = ?`, key).length) return { status: "pending" };
    const [{ n: inflight }] = this.rows(`SELECT COUNT(*) AS n FROM pending`);
    if (inflight >= this.limits.MAX_INFLIGHT_FILLS) {
      return { status: "skipped", reason: "busy", retryAfterSeconds: 60 };
    }
    const day = utcDay(nowMs);
    const used = this.today(day);
    if (used.fills >= this.limits.DAILY_FILL_LIMIT || used.bytes >= this.limits.DAILY_FILL_BYTES) {
      return { status: "skipped", reason: "daily_budget" };
    }
    // The Worker only asks after an R2 miss, so a ledger row here is stale.
    this.rows(`DELETE FROM packs WHERE key = ?`, key);
    this.rows(`INSERT INTO pending (key, since) VALUES (?, ?)`, key, Math.floor(nowMs / 1000));
    this.bumpDaily(day, 1, 0);
    return { status: "queued" };
  }

  /** Undo a reservation whose queue send failed. @param {{ owner: string, repo: string, commit: string }} job */
  release(job) {
    const key = packKey(job.owner, job.repo, job.commit);
    this.rows(`DELETE FROM pending WHERE key = ?`, key);
    this.bumpDaily(utcDay(this.now()), -1, 0);
    return { ok: true };
  }

  /**
   * Record a fill outcome; on success evict the oldest packs beyond the cap.
   * @param {{ owner: string, repo: string, commit: string, ok: boolean, bytes?: number,
   *   reused?: boolean, reason?: string, retryAfterSeconds?: number | null, bytesRead?: number }} outcome
   */
  async complete(outcome) {
    const nowMs = this.now();
    const now = Math.floor(nowMs / 1000);
    const repo = repoId(outcome.owner, outcome.repo);
    const key = packKey(outcome.owner, outcome.repo, outcome.commit);
    this.rows(`DELETE FROM pending WHERE key = ?`, key);
    const day = utcDay(nowMs);
    if (!outcome.ok) {
      const reason = outcome.reason && outcome.reason in NEGATIVE_TTL ? outcome.reason : "upstream_error";
      const ttl =
        reason === "rate_limited" && outcome.retryAfterSeconds
          ? Math.min(outcome.retryAfterSeconds, 3600)
          : NEGATIVE_TTL[/** @type {keyof typeof NEGATIVE_TTL} */ (reason)];
      const scope = reason === "rate_limited" ? "global" : REPO_SCOPED.has(reason) ? `repo:${repo}` : `key:${key}`;
      this.rows(
        `INSERT INTO negatives (scope, reason, until) VALUES (?, ?, ?)
         ON CONFLICT(scope) DO UPDATE SET reason = excluded.reason, until = excluded.until`,
        scope,
        reason,
        now + ttl,
      );
      this.bumpDaily(day, 0, outcome.bytesRead ?? 0);
      return { ok: true, evicted: [] };
    }
    const bytes = outcome.bytes ?? 0;
    this.rows(
      `INSERT INTO packs (key, repo, bytes, filled_at) VALUES (?, ?, ?, ?)
       ON CONFLICT(key) DO UPDATE SET bytes = excluded.bytes`,
      key,
      repo,
      bytes,
      now,
    );
    if (!outcome.reused) this.bumpDaily(day, 0, bytes);
    return { ok: true, evicted: await this.evict(key) };
  }

  /** @param {string} keep key that must survive (the pack just stored) */
  async evict(keep) {
    /** @type {string[]} */
    const evicted = [];
    for (;;) {
      const [{ total }] = this.rows(`SELECT COALESCE(SUM(bytes), 0) AS total FROM packs`);
      if (total <= this.limits.STORAGE_CAP_BYTES) break;
      const [oldest] = this.rows(
        `SELECT key FROM packs WHERE key != ? ORDER BY filled_at ASC, key ASC LIMIT 1`,
        keep,
      );
      if (!oldest) break;
      await this.env.PACKS.delete(oldest.key);
      this.rows(`DELETE FROM packs WHERE key = ?`, oldest.key);
      evicted.push(oldest.key);
    }
    return evicted;
  }

  /** Operator takedown: delete every pack of a repo and block refills. @param {{ owner: string, repo: string }} target */
  async takedown(target) {
    const repo = repoId(target.owner, target.repo);
    const prefix = repoPrefix(target.owner, target.repo);
    /** @type {Set<string>} */
    const keys = new Set(this.rows(`SELECT key FROM packs WHERE repo = ?`, repo).map((r) => r.key));
    let cursor;
    do {
      const listed = await this.env.PACKS.list({ prefix, cursor });
      for (const obj of listed.objects) keys.add(obj.key);
      cursor = listed.truncated ? listed.cursor : undefined;
    } while (cursor);
    if (keys.size) await this.env.PACKS.delete([...keys]);
    this.rows(`DELETE FROM packs WHERE repo = ?`, repo);
    const until = Math.floor(this.now() / 1000) + NEGATIVE_TTL.blocked;
    this.rows(
      `INSERT INTO negatives (scope, reason, until) VALUES (?, 'blocked', ?)
       ON CONFLICT(scope) DO UPDATE SET reason = 'blocked', until = excluded.until`,
      `repo:${repo}`,
      until,
    );
    return { deleted: keys.size };
  }

  stats() {
    const nowMs = this.now();
    this.prune(nowMs);
    const [stored] = this.rows(`SELECT COUNT(*) AS packs, COALESCE(SUM(bytes), 0) AS bytes FROM packs`);
    const [{ n: inflight }] = this.rows(`SELECT COUNT(*) AS n FROM pending`);
    const used = this.today(utcDay(nowMs));
    return {
      stored_packs: stored.packs,
      stored_bytes: stored.bytes,
      fills_in_flight: inflight,
      fills_today: used.fills,
      fill_bytes_today: used.bytes,
      limits: this.limits,
    };
  }
}
