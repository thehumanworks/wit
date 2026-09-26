import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import { PENDING_TTL_SECONDS, fillReserveBytes, limitsFromEnv } from "../src/config.js";
import worker, { runFillJob } from "../src/index.js";
import { packKey } from "../src/keys.js";
import { MAX_PKT_LEN } from "../src/pktline.js";
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
    const stats = await body(await call(env, "/v1/stats"));
    assert.deepEqual([stats.fills_today, stats.fill_bytes_today, stats.fills_in_flight], [0, 0, 0]);
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

  it("a transient coordinator error in complete is retried without re-running the fill", async () => {
    const gh = fakeGitHub({ "o/r": { refs: { main: SHA_A }, packs: { [SHA_A]: makePack(40_000) } } });
    const { env, bucket, coord } = makeEnv();
    await call(env, `/v1/github/o/r/${SHA_A}.pack?branch=main`);
    const complete = coord.complete.bind(coord);
    let failures = 1;
    coord.complete = async (/** @type {any} */ outcome) => {
      if (failures-- > 0) throw new Error("durable object reset");
      return complete(outcome);
    };
    /** @type {number[]} */
    const waits = [];
    const sleep = async (/** @type {number} */ ms) => waits.push(ms);
    const outcome = await runFillJob(env, env.FILL_QUEUE.sent[0], { fetchImpl: gh.fetchImpl, sleep });
    assert.equal(outcome.ok, true);
    assert.deepEqual(waits, [500]);
    assert.equal(gh.requests.length, 2, "one ls-refs and one fetch");
    assert.equal(bucket.ops.put, 1);
    const stats = coord.stats();
    assert.equal(stats.stored_packs, 1);
    assert.equal(stats.fills_in_flight, 0);
  });

  it("a fill whose bookkeeping keeps failing writes nothing to R2 on its one redelivery", async () => {
    const gh = fakeGitHub({ "o/r": { refs: { main: SHA_A }, packs: { [SHA_A]: makePack(40_000) } } });
    const { env, bucket, coord } = makeEnv({ PART_BYTES: String(16 * 1024) });
    await call(env, `/v1/github/o/r/${SHA_A}.pack?branch=main`);
    const job = env.FILL_QUEUE.sent[0];
    const complete = coord.complete.bind(coord);
    let calls = 0;
    coord.complete = async () => {
      calls++;
      throw new Error("durable object unavailable");
    };
    const deps = { fetchImpl: gh.fetchImpl, sleep: async () => {} };
    await assert.rejects(runFillJob(env, job, deps), /HTTP 500|unavailable/);
    assert.equal(calls, 3, "complete is tried three times before the queue redelivers");
    assert.ok(bucket.ops.uploadPart > 0);

    const writes = { ...bucket.ops };
    const requests = gh.requests.length;
    coord.complete = complete;
    const outcome = await runFillJob(env, job, { ...deps, attempts: 2 });
    assert.equal(outcome.reused, true);
    assert.equal(gh.requests.length, requests, "the redelivery does not refetch from GitHub");
    for (const op of /** @type {const} */ (["put", "createMultipart", "uploadPart", "complete"])) {
      assert.equal(bucket.ops[op], writes[op], `no Class A ${op} on the redelivery`);
    }
    assert.equal(coord.stats().stored_packs, 1);
    assert.equal(coord.stats().fill_bytes_today, outcome.bytes, "charged the pack the first delivery read");
  });

  it("the fill queue redelivers a message at most once", () => {
    const toml = readFileSync(new URL("../wrangler.toml", import.meta.url), "utf8");
    const consumer = toml.split("[[queues.consumers]]")[1].split(/\n\[/)[0];
    const retries = Number(consumer.match(/^max_retries = (\d+)$/m)?.[1]);
    assert.ok(
      retries <= 1,
      `max_retries = ${retries}: ADR 0009's Class A worst case (Budget.classA_worst_case_le_free_tier) allows one retry`,
    );
    assert.doesNotMatch(consumer, /dead_letter_queue/);
  });
});

describe("byte reservations", () => {
  const RESERVE = 1000 + MAX_PKT_LEN - 5;
  const vars = { MAX_PACK_BYTES: "1000", DAILY_FILL_BYTES: String(RESERVE + 2000) };
  const noWait = { sleep: async () => {} };

  it("a queued fill reserves the most it can read and complete settles to what it read", async () => {
    const pack = makePack(300);
    const gh = fakeGitHub({ "o/a": { refs: { main: SHA_A }, packs: { [SHA_A]: pack } } });
    const { env, coord } = makeEnv(vars);
    await call(env, `/v1/github/o/a/${SHA_A}.pack?branch=main`);
    assert.equal(coord.stats().fill_bytes_today, RESERVE);
    assert.equal(fillReserveBytes(limitsFromEnv(env)), RESERVE);
    await runFillJob(env, env.FILL_QUEUE.sent[0], { fetchImpl: gh.fetchImpl, ...noWait });
    assert.equal(coord.stats().fill_bytes_today, pack.length);
  });

  it("a fill is queued only when its whole reservation fits in the day's bytes", async () => {
    const pack = makePack(300);
    const gh = fakeGitHub({ "o/a": { refs: { main: SHA_A }, packs: { [SHA_A]: pack } } });
    const { env } = makeEnv(vars);
    await call(env, `/v1/github/o/a/${SHA_A}.pack?branch=main`);
    const second = await body(await call(env, `/v1/github/o/b/${SHA_B}.pack?branch=main`));
    assert.equal(second.reason, "daily_budget", "two reservations exceed the day's bytes");
    await runFillJob(env, env.FILL_QUEUE.sent[0], { fetchImpl: gh.fetchImpl, ...noWait });
    assert.equal((await body(await call(env, `/v1/github/o/b/${SHA_B}.pack?branch=main`))).fill, "queued");
  });

  it("when every complete fails, expired reservations stay charged until the next UTC day", async () => {
    let clock = Date.UTC(2026, 8, 1, 1);
    const shas = [SHA_A, SHA_B, SHA_C, "d".repeat(40)];
    const repos = Object.fromEntries(shas.map((sha, i) => [`o/r${i}`, { refs: { main: sha }, packs: { [sha]: makePack(300) } }]));
    const gh = fakeGitHub(repos);
    const { env, coord } = makeEnv({ ...vars, DAILY_FILL_BYTES: String(3 * RESERVE) }, { now: () => clock });
    coord.complete = async () => {
      throw new Error("durable object unavailable");
    };
    let fetched = 0;
    for (const [i, sha] of shas.entries()) {
      clock += (PENDING_TTL_SECONDS + 1) * 1000;
      const res = await body(await call(env, `/v1/github/o/r${i}/${sha}.pack?branch=main`));
      if (res.fill !== "queued") {
        assert.equal(res.reason, "daily_budget");
        continue;
      }
      await assert.rejects(runFillJob(env, env.FILL_QUEUE.sent.at(-1), { fetchImpl: gh.fetchImpl, ...noWait }));
      fetched++;
    }
    assert.equal(fetched, 3, "at most DAILY_FILL_BYTES / reserve fills a day");
    clock += (PENDING_TTL_SECONDS + 1) * 1000;
    const stats = coord.stats();
    assert.equal(stats.fills_in_flight, 0, "expired rows free their in-flight slots");
    assert.equal(stats.fill_bytes_today, 3 * RESERVE);
    clock = Date.UTC(2026, 8, 2, 0, 1);
    assert.equal((await body(await call(env, `/v1/github/o/r3/${shas[3]}.pack?branch=main`))).fill, "queued");
  });

  it("a failed fill settles to the bytes it read", async () => {
    const gh = fakeGitHub({ "o/big": { refs: { main: SHA_A }, packs: { [SHA_A]: makePack(20_000) } } });
    const { env, coord } = makeEnv(vars);
    await call(env, `/v1/github/o/big/${SHA_A}.pack?branch=main`);
    const outcome = await runFillJob(env, env.FILL_QUEUE.sent[0], { fetchImpl: gh.fetchImpl, ...noWait });
    assert.equal(outcome.reason, "too_large");
    assert.ok(outcome.bytesRead > 1000 && outcome.bytesRead <= RESERVE);
    assert.equal(coord.stats().fill_bytes_today, outcome.bytesRead);

    const missing = makeEnv(vars);
    await call(missing.env, `/v1/github/o/gone/${SHA_A}.pack?branch=main`);
    const gone = await runFillJob(missing.env, missing.env.FILL_QUEUE.sent[0], { fetchImpl: gh.fetchImpl, ...noWait });
    assert.equal(gone.reason, "not_found_or_private");
    assert.equal(missing.coord.stats().fill_bytes_today, 0, "a fill that never fetched refunds everything");
  });

  it("a redelivery never refetches; without a stored pack it keeps the whole reservation", async () => {
    const gh = fakeGitHub({ "o/a": { refs: { main: SHA_A }, packs: { [SHA_A]: makePack(300) } } });
    const { env, coord } = makeEnv(vars);
    await call(env, `/v1/github/o/a/${SHA_A}.pack?branch=main`);
    const outcome = await runFillJob(env, env.FILL_QUEUE.sent[0], { fetchImpl: gh.fetchImpl, attempts: 2, ...noWait });
    assert.equal(outcome.ok, false);
    assert.equal(gh.requests.length, 0);
    const stats = coord.stats();
    assert.equal(stats.fills_in_flight, 0);
    assert.equal(stats.fill_bytes_today, RESERVE);
  });

  it("a reservation settles once, and only for the fill that holds it", async () => {
    let clock = Date.UTC(2026, 8, 1, 1);
    const pack = makePack(300);
    const gh = fakeGitHub({ "o/a": { refs: { main: SHA_A }, packs: { [SHA_A]: pack } } });
    const { env, coord } = makeEnv({ ...vars, DAILY_FILL_BYTES: String(3 * RESERVE) }, { now: () => clock });
    await call(env, `/v1/github/o/a/${SHA_A}.pack?branch=main`);
    const stale = env.FILL_QUEUE.sent[0];
    clock += (PENDING_TTL_SECONDS + 1) * 1000;
    await call(env, `/v1/github/o/a/${SHA_A}.pack?branch=main`);
    const fresh = env.FILL_QUEUE.sent[1];
    assert.notEqual(stale.reservation, fresh.reservation);

    await runFillJob(env, stale, { fetchImpl: gh.fetchImpl, ...noWait });
    let stats = coord.stats();
    assert.equal(stats.fills_in_flight, 1, "the late complete leaves the newer fill pending");
    assert.equal(stats.fill_bytes_today, 2 * RESERVE, "and refunds neither reservation");

    await coord.complete({ ...fresh, ok: true, bytes: pack.length, bytesRead: pack.length });
    await coord.complete({ ...fresh, ok: true, bytes: pack.length, bytesRead: 0 });
    stats = coord.stats();
    assert.equal(stats.fills_in_flight, 0);
    assert.equal(stats.fill_bytes_today, RESERVE + pack.length, "the second complete refunds nothing");
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

  it("a fill request racing a stored pack keeps its bytes in the ledger", async () => {
    const pack = makePack(1000);
    const gh = fakeGitHub({ "o/r": { refs: { main: SHA_A }, packs: { [SHA_A]: pack } } });
    const { env, coord } = makeEnv();
    await call(env, `/v1/github/o/r/${SHA_A}.pack?branch=main`);
    await runFillJob(env, env.FILL_QUEUE.sent[0], { fetchImpl: gh.fetchImpl });
    // A miss observed just before that fill finished asks again, and the queue send fails.
    coord.requestFill({ owner: "o", repo: "r", commit: SHA_A });
    coord.release({ owner: "o", repo: "r", commit: SHA_A });
    const stats = await body(await call(env, "/v1/stats"));
    assert.equal(stats.stored_bytes, pack.length, "every stored pack must stay counted against the cap");
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

  it("a fill that finishes after a takedown neither restores the pack nor shortens the block", async () => {
    let clock = Date.UTC(2026, 8, 1);
    const pack = makePack(100);
    const gh = fakeGitHub({ "o/r": { refs: { main: SHA_A }, packs: { [SHA_A]: pack } } });
    const { env, bucket, coord } = makeEnv({ ADMIN_KEY: "operator-key-value" }, { now: () => clock });
    await call(env, `/v1/github/o/r/${SHA_A}.pack?branch=main`);
    const admin = { method: "DELETE", headers: { "x-wit-admin-key": "operator-key-value" } };
    assert.equal((await call(env, "/v1/github/o/r", admin)).status, 200);
    await runFillJob(env, env.FILL_QUEUE.sent[0], { fetchImpl: gh.fetchImpl });
    assert.equal(bucket.objects.size, 0, "a taken-down repo must not reappear from an in-flight fill");

    await coord.complete({ owner: "o", repo: "r", commit: SHA_B, ok: false, reason: "not_found_or_private" });
    clock += 2 * 3600_000;
    assert.equal((await body(await call(env, `/v1/github/o/r/${SHA_B}.pack?branch=main`))).reason, "blocked");
  });

  it("a fill that completes while a takedown is listing R2 is still removed", async () => {
    const pack = makePack(100);
    const gh = fakeGitHub({ "o/r": { refs: { main: SHA_A }, packs: { [SHA_A]: pack } } });
    const { env, bucket } = makeEnv({ ADMIN_KEY: "operator-key-value" });
    await call(env, `/v1/github/o/r/${SHA_A}.pack?branch=main`);
    const list = bucket.list.bind(bucket);
    let raced = false;
    bucket.list = async (opts) => {
      const listed = await list(opts);
      if (!raced) {
        raced = true;
        await runFillJob(env, env.FILL_QUEUE.sent[0], { fetchImpl: gh.fetchImpl });
      }
      return listed;
    };
    const admin = { method: "DELETE", headers: { "x-wit-admin-key": "operator-key-value" } };
    assert.equal((await call(env, "/v1/github/o/r", admin)).status, 200);
    assert.ok(raced);
    assert.equal(bucket.objects.size, 0, "a pack stored during the takedown must not survive it");
  });

  it("takedown is disabled when no operator key is configured", async () => {
    const { env } = makeEnv();
    const res = await call(env, "/v1/github/o/r", { method: "DELETE", headers: { "x-wit-admin-key": "" } });
    assert.equal(res.status, 404);
  });
});
