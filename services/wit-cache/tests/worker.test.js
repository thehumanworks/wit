import assert from "node:assert/strict";
import { describe, it } from "node:test";
import worker, { runFillJob } from "../src/index.js";
import { packKey } from "../src/keys.js";
import { FakeBucket, SHA_A, SHA_B, SHA_C, fakeGitHub, makeEnv, makePack, readAll } from "./helpers.js";

const BASE = "https://wit-cache.test";

/** @param {any} env @param {string} path @param {RequestInit & { ip?: string }} [init] */
function call(env, path, init = {}) {
  const headers = new Headers(init.headers);
  headers.set("cf-connecting-ip", init.ip ?? "203.0.113.7");
  return worker.fetch(new Request(`${BASE}${path}`, { ...init, headers }), env);
}

/** @param {Response} res */
async function body(res) {
  const text = await res.text();
  assert.equal(text.includes("[REDACTED]"), false, `body must not need redaction: ${text}`);
  return JSON.parse(text);
}

describe("read path and lazy fill", () => {
  it("miss queues a fill; the consumer stores it; the next GET streams it", async () => {
    const pack = makePack(100_000);
    const gh = fakeGitHub({ "openai/codex": { refs: { main: SHA_A }, packs: { [SHA_A]: pack } } });
    const { env, bucket } = makeEnv();

    const miss = await call(env, `/v1/github/openai/codex/${SHA_A}.pack?branch=main`);
    assert.equal(miss.status, 404);
    assert.deepEqual(await body(miss), { error: "not cached", fill: "queued" });
    assert.equal(env.FILL_QUEUE.sent.length, 1);

    const again = await call(env, `/v1/github/openai/codex/${SHA_A}.pack?branch=main`);
    assert.equal((await body(again)).fill, "pending", "single-flight per repo@commit");
    assert.equal(env.FILL_QUEUE.sent.length, 1);

    const outcome = await runFillJob(env, env.FILL_QUEUE.sent[0], { fetchImpl: gh.fetchImpl });
    assert.equal(outcome.ok, true);
    assert.ok(bucket.objects.has(packKey("openai", "codex", SHA_A)));

    const hit = await call(env, `/v1/github/OpenAI/Codex/${SHA_A}.pack`);
    assert.equal(hit.status, 200);
    assert.equal(hit.headers.get("content-type"), "application/x-git-packfile");
    assert.equal(hit.headers.get("content-length"), String(pack.length));
    assert.equal(hit.headers.get("x-wit-commit"), SHA_A);
    assert.match(hit.headers.get("cache-control") ?? "", /immutable/);
    assert.deepEqual(await readAll(/** @type {any} */ (hit.body)), pack);

    const head = await call(env, `/v1/github/openai/codex/${SHA_A}.pack`, { method: "HEAD" });
    assert.equal(head.status, 200);
    assert.equal(head.headers.get("content-length"), String(pack.length));
  });

  it("ignores the caller Authorization header and never forwards it upstream", async () => {
    const pack = makePack(1000);
    const gh = fakeGitHub({ "o/r": { refs: { main: SHA_A }, packs: { [SHA_A]: pack } } });
    const { env } = makeEnv();
    const res = await call(env, `/v1/github/o/r/${SHA_A}.pack?branch=main`, {
      headers: { authorization: "Bearer ghp_callerSecretValue123" },
    });
    assert.equal(res.status, 404);
    const job = env.FILL_QUEUE.sent[0];
    assert.equal(JSON.stringify(job).includes("ghp_"), false, "fill job carries no caller credentials");
    await runFillJob(env, job, { fetchImpl: gh.fetchImpl });
    assert.ok(gh.requests.length >= 2);
    for (const req of gh.requests) assert.equal(req.headers.get("authorization"), null);
  });

  it("with an execution context the miss answers first and requests the fill afterwards", async () => {
    const { env } = makeEnv();
    /** @type {Promise<unknown>[]} */
    const pending = [];
    const ctx = { waitUntil: (/** @type {Promise<unknown>} */ p) => void pending.push(p) };
    const req = new Request(`${BASE}/v1/github/o/r/${SHA_A}.pack?branch=main`, {
      headers: { "cf-connecting-ip": "203.0.113.7" },
    });
    const res = await worker.fetch(req, env, ctx);
    assert.equal(res.status, 404);
    assert.deepEqual(await body(res), { error: "not cached", fill: "requested" });
    assert.equal(pending.length, 1);
    assert.deepEqual(await pending[0], { status: "queued" });
    assert.equal(env.FILL_QUEUE.sent.length, 1);
  });

  it("a miss without a branch does not fill", async () => {
    const { env } = makeEnv();
    const res = await call(env, `/v1/github/o/r/${SHA_A}.pack`);
    assert.equal(res.status, 404);
    assert.equal((await body(res)).fill, "skipped");
    assert.equal(env.FILL_QUEUE.sent.length, 0);
  });

  it("refuses to store a commit that is not the branch tip (fork-network smuggling)", async () => {
    const gh = fakeGitHub({ "o/r": { refs: { main: SHA_A }, packs: { [SHA_B]: makePack(10) } } });
    const { env, bucket } = makeEnv();
    await call(env, `/v1/github/o/r/${SHA_B}.pack?branch=main`);
    const outcome = await runFillJob(env, env.FILL_QUEUE.sent[0], { fetchImpl: gh.fetchImpl });
    assert.equal(outcome.reason, "not_tip");
    assert.equal(bucket.objects.size, 0);
    assert.equal(gh.requests.filter((r) => r.body.includes("command=fetch")).length, 0);
  });

  it("private or missing repos are negatively cached for every commit", async () => {
    const gh = fakeGitHub({});
    const { env } = makeEnv();
    await call(env, `/v1/github/o/secret/${SHA_A}.pack?branch=main`);
    const outcome = await runFillJob(env, env.FILL_QUEUE.sent[0], { fetchImpl: gh.fetchImpl });
    assert.equal(outcome.reason, "not_found_or_private");
    const next = await call(env, `/v1/github/o/secret/${SHA_B}.pack?branch=main`);
    const parsed = await body(next);
    assert.equal(parsed.fill, "skipped");
    assert.equal(parsed.reason, "not_found_or_private");
    assert.ok(Number(next.headers.get("retry-after")) > 0);
    assert.equal(env.FILL_QUEUE.sent.length, 1);
  });

  it("oversize packs are aborted and negatively cached per repo", async () => {
    const gh = fakeGitHub({ "o/big": { refs: { main: SHA_A }, packs: { [SHA_A]: makePack(20_000) } } });
    const { env, bucket } = makeEnv({ MAX_PACK_BYTES: "5000" });
    await call(env, `/v1/github/o/big/${SHA_A}.pack?branch=main`);
    const outcome = await runFillJob(env, env.FILL_QUEUE.sent[0], { fetchImpl: gh.fetchImpl });
    assert.equal(outcome.reason, "too_large");
    assert.equal(bucket.objects.size, 0);
    assert.equal((await body(await call(env, `/v1/github/o/big/${SHA_B}.pack?branch=main`))).reason, "too_large");
  });

  it("releases the reservation when the queue is unavailable", async () => {
    const { env } = makeEnv();
    env.FILL_QUEUE.fail = true;
    const res = await body(await call(env, `/v1/github/o/r/${SHA_A}.pack?branch=main`));
    assert.equal(res.reason, "enqueue_failed");
    env.FILL_QUEUE.fail = false;
    assert.equal((await body(await call(env, `/v1/github/o/r/${SHA_A}.pack?branch=main`))).fill, "queued");
  });

  it("queue consumer fills through global fetch and acks", async () => {
    const pack = makePack(3000);
    const gh = fakeGitHub({ "o/r": { refs: { main: SHA_A }, packs: { [SHA_A]: pack } } });
    const { env, bucket } = makeEnv();
    await call(env, `/v1/github/o/r/${SHA_A}.pack?branch=main`);
    const original = globalThis.fetch;
    globalThis.fetch = /** @type {any} */ (gh.fetchImpl);
    const acks = [];
    try {
      await worker.queue(
        /** @type {any} */ ({
          messages: [
            { body: env.FILL_QUEUE.sent[0], ack: () => acks.push("ok"), retry: () => acks.push("retry") },
            { body: { owner: "..", repo: "x", commit: "z", branch: "main" }, ack: () => acks.push("bad"), retry: () => {} },
          ],
        }),
        env,
      );
    } finally {
      globalThis.fetch = original;
    }
    assert.deepEqual(acks, ["ok", "bad"]);
    assert.equal(bucket.objects.size, 1);
  });
});

describe("limits", () => {
  it("per-IP read limit answers 429", async () => {
    const { env } = makeEnv({}, { readLimit: 2 });
    await call(env, `/v1/github/o/r/${SHA_A}.pack`);
    await call(env, `/v1/github/o/r/${SHA_A}.pack`);
    const res = await call(env, `/v1/github/o/r/${SHA_A}.pack`);
    assert.equal(res.status, 429);
    assert.equal(res.headers.get("retry-after"), "60");
    const other = await call(env, `/v1/github/o/r/${SHA_A}.pack`, { ip: "198.51.100.1" });
    assert.equal(other.status, 404);
  });

  it("per-IP fill limit skips the fill but still answers a plain miss", async () => {
    const { env } = makeEnv({}, { fillLimit: 1 });
    await call(env, `/v1/github/o/r/${SHA_A}.pack?branch=main`);
    const res = await body(await call(env, `/v1/github/o/r/${SHA_B}.pack?branch=main`));
    assert.equal(res.reason, "fill_rate_limited");
    assert.equal(env.FILL_QUEUE.sent.length, 1);
  });

  it("global daily fill budget and in-flight cap", async () => {
    const { env } = makeEnv({ DAILY_FILL_LIMIT: "2", MAX_INFLIGHT_FILLS: "5" });
    await call(env, `/v1/github/o/a/${SHA_A}.pack?branch=main`);
    await call(env, `/v1/github/o/b/${SHA_A}.pack?branch=main`);
    assert.equal((await body(await call(env, `/v1/github/o/c/${SHA_A}.pack?branch=main`))).reason, "daily_budget");

    const busy = makeEnv({ MAX_INFLIGHT_FILLS: "1" }).env;
    await call(busy, `/v1/github/o/a/${SHA_A}.pack?branch=main`);
    assert.equal((await body(await call(busy, `/v1/github/o/b/${SHA_A}.pack?branch=main`))).reason, "busy");
  });

  it("evicts the oldest packs beyond the storage cap", async () => {
    let clock = Date.UTC(2026, 8, 1);
    const pack = makePack(1000);
    const gh = fakeGitHub({
      "o/a": { refs: { main: SHA_A }, packs: { [SHA_A]: pack } },
      "o/b": { refs: { main: SHA_B }, packs: { [SHA_B]: pack } },
      "o/c": { refs: { main: SHA_C }, packs: { [SHA_C]: pack } },
    });
    const { env, bucket } = makeEnv({ STORAGE_CAP_BYTES: String(pack.length * 2) }, { now: () => clock });
    for (const [repo, sha] of [["a", SHA_A], ["b", SHA_B], ["c", SHA_C]]) {
      clock += 1000;
      await call(env, `/v1/github/o/${repo}/${sha}.pack?branch=main`);
      await runFillJob(env, env.FILL_QUEUE.sent.at(-1), { fetchImpl: gh.fetchImpl });
    }
    assert.deepEqual([...bucket.objects.keys()].sort(), [packKey("o", "b", SHA_B), packKey("o", "c", SHA_C)]);
    const stats = await body(await call(env, "/v1/stats"));
    assert.equal(stats.stored_packs, 2);
    assert.equal(stats.stored_bytes, pack.length * 2);
    assert.equal(stats.fills_today, 3);
  });
});

describe("routes", () => {
  it("rejects malformed paths and writes", async () => {
    const { env } = makeEnv();
    assert.equal((await call(env, "/v1/github/o/r/nothex.pack")).status, 400);
    assert.equal((await call(env, `/v1/github/o/r/${SHA_A}.pack`, { method: "PUT", body: "x" })).status, 405);
    assert.equal((await call(env, "/nope")).status, 404);
    const info = await body(await call(env, "/"));
    assert.equal(info.service, "wit-cache");
    assert.equal(info.retention_days, 30);
  });

  it("takedown needs the operator key, deletes packs, and blocks refills", async () => {
    const bucket = new FakeBucket();
    const { env } = makeEnv({ ADMIN_KEY: "operator-key-value" }, { bucket });
    await bucket.put(packKey("o", "r", SHA_A), makePack(10));
    assert.equal((await call(env, "/v1/github/o/r", { method: "DELETE" })).status, 404);
    assert.equal(
      (await call(env, "/v1/github/o/r", { method: "DELETE", headers: { "x-wit-admin-key": "wrong" } })).status,
      404,
    );
    const ok = await call(env, "/v1/github/o/r", {
      method: "DELETE",
      headers: { "x-wit-admin-key": "operator-key-value" },
    });
    assert.equal(ok.status, 200);
    assert.equal((await body(ok)).deleted, 1);
    assert.equal(bucket.objects.size, 0);
    assert.equal((await body(await call(env, `/v1/github/o/r/${SHA_B}.pack?branch=main`))).reason, "blocked");
  });

  it("takedown is disabled when no operator key is configured", async () => {
    const { env } = makeEnv();
    const res = await call(env, "/v1/github/o/r", { method: "DELETE", headers: { "x-wit-admin-key": "" } });
    assert.equal(res.status, 404);
  });
});
