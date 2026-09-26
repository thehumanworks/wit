/**
 * No-network fakes: R2 bucket (with multipart rules), Durable Object SQLite
 * storage (node:sqlite), queue, rate limiter, and a GitHub upload-pack server.
 */

import { createHash, randomBytes } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { FillCoordinator } from "../src/coordinator.js";
import { DELIM, FLUSH, pkt } from "../src/pktline.js";

const enc = new TextEncoder();

/** @param {Uint8Array[]} parts */
export function concat(parts) {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let off = 0;
  for (const p of parts) {
    out.set(p, off);
    off += p.length;
  }
  return out;
}

/** @param {Uint8Array} bytes @param {number} [chunk] */
export function streamOf(bytes, chunk = 7) {
  let off = 0;
  return new ReadableStream({
    pull(controller) {
      if (off >= bytes.length) return controller.close();
      const n = Math.max(1, Math.min(chunk, bytes.length - off));
      controller.enqueue(bytes.slice(off, off + n));
      off += n;
    },
  });
}

/** @param {ReadableStream<Uint8Array>} stream */
export async function readAll(stream) {
  const parts = [];
  const reader = stream.getReader();
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    parts.push(value);
  }
  return concat(parts);
}

/**
 * Synthetic git pack: valid header and SHA-1 trailer around random payload.
 * The Worker checks framing only; object parsing is the client's job.
 * @param {number} payloadBytes
 */
export function makePack(payloadBytes = 1000) {
  const head = new Uint8Array(12);
  head.set(enc.encode("PACK"), 0);
  const view = new DataView(head.buffer);
  view.setUint32(4, 2);
  view.setUint32(8, 3);
  const body = concat([head, new Uint8Array(randomBytes(payloadBytes))]);
  const trailer = new Uint8Array(createHash("sha1").update(body).digest());
  return concat([body, trailer]);
}

export class FakeBucket {
  /** @param {{ minPartBytes?: number }} [opts] */
  constructor(opts = {}) {
    /** @type {Map<string, { bytes: Uint8Array, httpMetadata?: any, customMetadata?: Record<string,string> }>} */
    this.objects = new Map();
    this.minPartBytes = opts.minPartBytes ?? 1;
    this.ops = { put: 0, createMultipart: 0, uploadPart: 0, complete: 0, abort: 0, get: 0, head: 0, delete: 0 };
  }

  /** @param {string} key */
  meta(key) {
    const o = this.objects.get(key);
    if (!o) return null;
    return { key, size: o.bytes.length, httpEtag: `"${key.length}-${o.bytes.length}"`, customMetadata: o.customMetadata ?? {} };
  }

  /** @param {string} key */
  async head(key) {
    this.ops.head++;
    return this.meta(key);
  }

  /** @param {string} key */
  async get(key) {
    this.ops.get++;
    const m = this.meta(key);
    if (!m) return null;
    return { ...m, body: streamOf(/** @type {any} */ (this.objects.get(key)).bytes, 4096) };
  }

  /** @param {string} key @param {Uint8Array} value @param {any} [opts] */
  async put(key, value, opts = {}) {
    this.ops.put++;
    this.objects.set(key, { bytes: new Uint8Array(value), ...opts });
  }

  /** @param {string | string[]} keys */
  async delete(keys) {
    this.ops.delete++;
    for (const k of Array.isArray(keys) ? keys : [keys]) this.objects.delete(k);
  }

  /** @param {{ prefix?: string }} opts */
  async list(opts = {}) {
    const objects = [...this.objects.keys()].filter((k) => k.startsWith(opts.prefix ?? "")).map((key) => ({ key }));
    return { objects, truncated: false };
  }

  /** @param {string} key @param {any} opts */
  async createMultipartUpload(key, opts = {}) {
    this.ops.createMultipart++;
    const bucket = this;
    /** @type {Map<number, Uint8Array>} */
    const parts = new Map();
    return {
      async uploadPart(/** @type {number} */ n, /** @type {Uint8Array} */ data) {
        bucket.ops.uploadPart++;
        parts.set(n, new Uint8Array(data));
        return { partNumber: n, etag: `p${n}` };
      },
      async complete(/** @type {{partNumber:number}[]} */ uploaded) {
        bucket.ops.complete++;
        const ordered = uploaded.map((p) => /** @type {Uint8Array} */ (parts.get(p.partNumber)));
        const sizes = ordered.map((p) => p.length);
        for (let i = 0; i < sizes.length - 1; i++) {
          if (sizes[i] !== sizes[0]) throw new Error("R2: non-final parts must be the same size");
          if (sizes[i] < bucket.minPartBytes) throw new Error("R2: part below minimum size");
        }
        bucket.objects.set(key, { bytes: concat(ordered), ...opts });
      },
      async abort() {
        bucket.ops.abort++;
      },
    };
  }
}

export function fakeSql() {
  const db = new DatabaseSync(":memory:");
  return {
    /** @param {string} q @param {...any} b */
    exec(q, ...b) {
      const stmt = db.prepare(q);
      const rows = /^\s*SELECT/i.test(q) ? stmt.all(...b) : (stmt.run(...b), []);
      return { toArray: () => rows.map((r) => ({ ...r })) };
    },
  };
}

/** @param {FillCoordinator} instance */
export function fakeNamespace(instance) {
  return {
    idFromName: (/** @type {string} */ name) => name,
    get: () => ({
      fetch: (/** @type {string} */ url, /** @type {RequestInit} */ init) => instance.fetch(new Request(url, init)),
    }),
  };
}

export class FakeQueue {
  constructor() {
    /** @type {any[]} */
    this.sent = [];
    this.fail = false;
  }
  /** @param {any} body */
  async send(body) {
    if (this.fail) throw new Error("queue unavailable");
    this.sent.push(body);
  }
}

/** @param {number} limit */
export function fakeLimiter(limit) {
  /** @type {Map<string, number>} */
  const seen = new Map();
  return {
    seen,
    async limit(/** @type {{key:string}} */ { key }) {
      const n = (seen.get(key) ?? 0) + 1;
      seen.set(key, n);
      return { success: n <= limit };
    },
  };
}

/**
 * @param {Record<string, string>} [vars]
 * @param {{ bucket?: FakeBucket, readLimit?: number, fillLimit?: number, now?: () => number }} [opts]
 */
export function makeEnv(vars = {}, opts = {}) {
  const bucket = opts.bucket ?? new FakeBucket();
  /** @type {any} */
  const env = { PACKS: bucket, FILL_QUEUE: new FakeQueue(), ...vars };
  const coord = new FillCoordinator({ storage: { sql: fakeSql() } }, env);
  if (opts.now) coord.now = opts.now;
  env.COORDINATOR = fakeNamespace(coord);
  if (opts.readLimit) env.READ_LIMITER = fakeLimiter(opts.readLimit);
  if (opts.fillLimit) env.FILL_LIMITER = fakeLimiter(opts.fillLimit);
  return { env, bucket, coord };
}

/**
 * Encode an upload-pack v2 fetch response carrying `pack` on side-band 1.
 * @param {Uint8Array} pack
 * @param {string} commit
 */
export function fetchResponseBytes(pack, commit) {
  const parts = [
    enc.encode(pkt("shallow-info\n") + pkt(`shallow ${commit}\n`) + DELIM + pkt("packfile\n")),
    enc.encode(pkt("\u0002Enumerating objects\n")),
  ];
  for (let off = 0; off < pack.length; off += 65515) {
    const slice = pack.subarray(off, off + 65515);
    const len = (slice.length + 5).toString(16).padStart(4, "0");
    parts.push(enc.encode(len), new Uint8Array([1]), slice);
  }
  parts.push(enc.encode(FLUSH));
  return concat(parts);
}

/**
 * Fake github.com upload-pack. `repos[owner/repo]` maps branch -> commit and
 * commit -> pack; missing repos answer 401 like GitHub does anonymously.
 * @param {Record<string, { refs: Record<string,string>, packs: Record<string, Uint8Array> }>} repos
 */
export function fakeGitHub(repos) {
  /** @type {{ url: string, headers: Headers, body: string }[]} */
  const requests = [];
  /** @param {string} url @param {RequestInit} init */
  const fetchImpl = async (url, init = {}) => {
    const headers = new Headers(init.headers);
    const body = String(init.body ?? "");
    requests.push({ url, headers, body });
    const m = url.match(/^https:\/\/github\.com\/([^/]+\/[^/]+)\.git\/git-upload-pack$/);
    const repo = m && repos[m[1]];
    if (!repo) return new Response("Repository not found.\n", { status: 401 });
    if (body.includes("command=ls-refs")) {
      const prefix = body.match(/ref-prefix (\S+)\n/)?.[1] ?? "";
      let out = "";
      for (const [branch, sha] of Object.entries(repo.refs)) {
        const name = `refs/heads/${branch}`;
        if (name.startsWith(prefix)) out += pkt(`${sha} ${name}\n`);
      }
      return new Response(streamOf(enc.encode(out + FLUSH), 13), { status: 200 });
    }
    const want = body.match(/want ([0-9a-f]{40})\n/)?.[1] ?? "";
    const pack = repo.packs[want];
    if (!pack) return new Response(streamOf(enc.encode(pkt(`ERR upload-pack: not our ref ${want}\n`))), { status: 200 });
    return new Response(streamOf(fetchResponseBytes(pack, want), 997), { status: 200 });
  };
  return { fetchImpl, requests };
}

export const SHA_A = "a".repeat(40);
export const SHA_B = "b".repeat(40);
export const SHA_C = "c".repeat(40);
