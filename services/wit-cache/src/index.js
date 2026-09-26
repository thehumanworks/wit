/**
 * wit-cache: shared, read-only, no-login cache of depth-1 git packs keyed by
 * GitHub repo + commit SHA (ADR 0009).
 *
 *   GET|HEAD /v1/github/{owner}/{repo}/{commit}.pack?branch={branch}
 *     200  the pack (immutable bytes for that commit)
 *     404  miss; a GET with `branch` queues an anonymous fill
 *   GET /v1/stats                       storage and budget counters
 *   DELETE /v1/github/{owner}/{repo}    operator takedown (X-Wit-Admin-Key)
 *
 * Callers never write, and the caller's Authorization header is never read
 * or forwarded: the cache has no notion of caller identity.
 */

import { SafeError, safeConsole, scrubSecrets } from "./auth.js";
import { limitsFromEnv } from "./config.js";
import { FillCoordinator } from "./coordinator.js";
import { fillPack } from "./fill.js";
import { isSafeBranch, packKey, parsePackPath, parseRepoPath } from "./keys.js";
import { FillError } from "./upload-pack.js";

export { FillCoordinator };

const COORDINATOR_NAME = "global";
const CACHE_CONTROL_HIT = "public, max-age=86400, immutable";

/**
 * @typedef {{
 *   PACKS: R2Bucket,
 *   COORDINATOR: DurableObjectNamespace,
 *   FILL_QUEUE: Queue,
 *   READ_LIMITER?: { limit(o: { key: string }): Promise<{ success: boolean }> },
 *   FILL_LIMITER?: { limit(o: { key: string }): Promise<{ success: boolean }> },
 *   ADMIN_KEY?: string,
 * } & Record<string, unknown>} Env
 */

/**
 * @param {unknown} body
 * @param {number} status
 * @param {Record<string, string>} [headers]
 */
function json(body, status, headers = {}) {
  return new Response(scrubSecrets(JSON.stringify(body, null, 2)) + "\n", {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
      "x-content-type-options": "nosniff",
      ...headers,
    },
  });
}

/** Rate-limit key: the connecting IP, used only by the limiter and never logged. @param {Request} request */
function clientKey(request) {
  return request.headers.get("cf-connecting-ip") || "unknown";
}

/**
 * @param {Env["READ_LIMITER"]} limiter
 * @param {string} key
 */
async function allowed(limiter, key) {
  if (!limiter) return true;
  const { success } = await limiter.limit({ key });
  return success;
}

/** @param {Env} env */
function coordinator(env) {
  return env.COORDINATOR.get(env.COORDINATOR.idFromName(COORDINATOR_NAME));
}

/**
 * @param {Env} env
 * @param {string} path
 * @param {unknown} [body]
 */
async function callCoordinator(env, path, body) {
  const res = await coordinator(env).fetch(`https://coordinator${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) throw new SafeError(`fill coordinator answered HTTP ${res.status}`);
  return res.json();
}

/**
 * @param {R2Object} obj
 * @param {string} commit
 * @param {R2ObjectBody | null} body
 */
function packResponse(obj, commit, body) {
  const headers = new Headers({
    "content-type": "application/x-git-packfile",
    "content-length": String(obj.size),
    "cache-control": CACHE_CONTROL_HIT,
    etag: obj.httpEtag,
    "x-content-type-options": "nosniff",
    "x-wit-commit": commit,
  });
  const meta = obj.customMetadata ?? {};
  if (meta.filled_at) headers.set("x-wit-filled-at", meta.filled_at);
  if (meta.branch) headers.set("x-wit-branch-at-fill", meta.branch);
  return new Response(body ? body.body : null, { status: 200, headers });
}

/**
 * @param {Request} request
 * @param {Env} env
 * @param {{ owner: string, repo: string, commit: string }} target
 */
async function handlePack(request, env, target) {
  const ip = clientKey(request);
  if (!(await allowed(env.READ_LIMITER, ip))) {
    return json({ error: "read rate limit reached; retry in a minute" }, 429, { "retry-after": "60" });
  }
  const key = packKey(target.owner, target.repo, target.commit);
  if (request.method === "HEAD") {
    const head = await env.PACKS.head(key);
    return head ? packResponse(head, target.commit, null) : new Response(null, { status: 404 });
  }
  const obj = await env.PACKS.get(key);
  if (obj) return packResponse(obj, target.commit, obj);

  const branch = new URL(request.url).searchParams.get("branch");
  if (!branch || !isSafeBranch(branch)) {
    return json({ error: "not cached", fill: "skipped", reason: "branch query parameter required to fill" }, 404);
  }
  if (!(await allowed(env.FILL_LIMITER, ip))) {
    return json({ error: "not cached", fill: "skipped", reason: "fill_rate_limited" }, 404, {
      "retry-after": "60",
    });
  }
  const job = { owner: target.owner, repo: target.repo, commit: target.commit, branch };
  const decision = await callCoordinator(env, "/request-fill", job);
  if (decision.status === "queued") {
    try {
      await env.FILL_QUEUE.send(job);
    } catch (err) {
      await callCoordinator(env, "/release", job).catch(() => {});
      safeConsole.error("fill enqueue failed", err);
      return json({ error: "not cached", fill: "skipped", reason: "enqueue_failed" }, 404);
    }
  }
  /** @type {Record<string, string>} */
  const headers = {};
  if (decision.retryAfterSeconds) headers["retry-after"] = String(decision.retryAfterSeconds);
  return json(
    { error: "not cached", fill: decision.status, ...(decision.reason ? { reason: decision.reason } : {}) },
    404,
    headers,
  );
}

/**
 * Constant-time string comparison for the operator key.
 * @param {string} a
 * @param {string} b
 */
function sameSecret(a, b) {
  const enc = new TextEncoder();
  const x = enc.encode(a);
  const y = enc.encode(b);
  let diff = x.length ^ y.length;
  for (let i = 0; i < Math.max(x.length, y.length); i++) diff |= (x[i] ?? 0) ^ (y[i] ?? 0);
  return diff === 0;
}

/**
 * @param {Request} request
 * @param {Env} env
 * @param {{ owner: string, repo: string }} target
 */
async function handleTakedown(request, env, target) {
  const expected = typeof env.ADMIN_KEY === "string" ? env.ADMIN_KEY : "";
  const given = request.headers.get("x-wit-admin-key") ?? "";
  if (!expected || !sameSecret(given, expected)) return json({ error: "not found" }, 404);
  const result = await callCoordinator(env, "/takedown", target);
  return json({ ok: true, ...result }, 200);
}

/** @param {Env} env */
function info(env) {
  const limits = limitsFromEnv(env);
  return {
    service: "wit-cache",
    about: "Shared read-only cache of depth-1 git packs for public GitHub repositories (wit CLI).",
    pack: "GET /v1/github/{owner}/{repo}/{commit}.pack?branch={branch}",
    stats: "GET /v1/stats",
    fill: "A miss queues an anonymous fill of the branch tip; the next request hits.",
    retention_days: limits.RETENTION_DAYS,
    max_pack_bytes: limits.MAX_PACK_BYTES,
    docs: "https://github.com/thehumanworks/wit/blob/main/docs/adr/0009-shared-cloud-pack-cache.md",
  };
}

/**
 * @param {Request} request
 * @param {Env} env
 */
async function route(request, env) {
  const url = new URL(request.url);
  const method = request.method;
  if (url.pathname === "/" && (method === "GET" || method === "HEAD")) return json(info(env), 200);
  if (url.pathname === "/v1/stats" && method === "GET") {
    if (!(await allowed(env.READ_LIMITER, clientKey(request)))) {
      return json({ error: "read rate limit reached; retry in a minute" }, 429, { "retry-after": "60" });
    }
    return json(await callCoordinator(env, "/stats"), 200);
  }
  const pack = parsePackPath(url.pathname);
  if (pack) {
    if (method !== "GET" && method !== "HEAD") return json({ error: "method not allowed" }, 405, { allow: "GET, HEAD" });
    return handlePack(request, env, pack);
  }
  const repo = parseRepoPath(url.pathname);
  if (repo && method === "DELETE") return handleTakedown(request, env, repo);
  if (url.pathname.startsWith("/v1/github/")) {
    return json({ error: "expected /v1/github/{owner}/{repo}/{40-hex commit}.pack" }, 400);
  }
  return json({ error: "not found" }, 404);
}

/**
 * @param {Env} env
 * @param {import("./fill.js").FillJob} job
 * @param {{ fetchImpl?: typeof fetch }} [deps]
 */
export async function runFillJob(env, job, deps = {}) {
  const limits = limitsFromEnv(env);
  const started = Date.now();
  let outcome;
  try {
    const done = await fillPack(env, job, limits, deps);
    outcome = { ...job, ok: true, bytes: done.bytes, reused: done.reused };
    safeConsole.log(
      `fill ok ${job.owner}/${job.repo}@${job.commit} bytes=${done.bytes} parts=${done.parts} ms=${Date.now() - started}`,
    );
  } catch (err) {
    const fillErr = err instanceof FillError ? err : new FillError("upstream_error", String(err));
    outcome = {
      ...job,
      ok: false,
      reason: fillErr.reason,
      retryAfterSeconds: fillErr.retryAfterSeconds,
      bytesRead: fillErr.bytesRead,
    };
    safeConsole.warn(`fill failed ${job.owner}/${job.repo}@${job.commit} reason=${fillErr.reason}: ${fillErr.message}`);
  }
  await callCoordinator(env, "/complete", outcome);
  return outcome;
}

export default {
  /**
   * @param {Request} request
   * @param {Env} env
   */
  async fetch(request, env) {
    try {
      return await route(request, env);
    } catch (err) {
      safeConsole.error("request failed", err);
      const status = err instanceof SafeError ? err.status : 500;
      return json({ error: "cache unavailable; clients fall back to GitHub" }, status);
    }
  },

  /**
   * @param {MessageBatch<import("./fill.js").FillJob>} batch
   * @param {Env} env
   */
  async queue(batch, env) {
    for (const message of batch.messages) {
      const job = message.body;
      const valid =
        job &&
        parsePackPath(`/v1/github/${job.owner}/${job.repo}/${job.commit}.pack`) &&
        typeof job.branch === "string" &&
        isSafeBranch(job.branch);
      if (!valid) {
        message.ack();
        continue;
      }
      try {
        await runFillJob(env, job);
        message.ack();
      } catch (err) {
        safeConsole.error("fill bookkeeping failed", err);
        message.retry();
      }
    }
  },
};
