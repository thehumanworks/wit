import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import { isSafeBranch, packKey, parsePackPath, parseRepoPath } from "../src/keys.js";
import { PktLineReader, pkt } from "../src/pktline.js";
import { PackUpload, PackVerifier } from "../src/store.js";
import { FillError, fetchBody, fetchPack, lsRefs, lsRefsBody } from "../src/upload-pack.js";
import { FakeBucket, SHA_A, SHA_B, fakeGitHub, makePack, readAll, streamOf } from "./helpers.js";

const enc = new TextEncoder();

describe("keys", () => {
  it("lowercases owner/repo into a canonical key", () => {
    assert.equal(packKey("OpenAI", "Codex", SHA_A), `v1/github/openai/codex/${SHA_A}.pack`);
  });

  it("parses only well-formed pack paths", () => {
    assert.deepEqual(parsePackPath(`/v1/github/openai/codex/${SHA_A}.pack`), {
      owner: "openai",
      repo: "codex",
      commit: SHA_A,
    });
    for (const bad of [
      `/v1/github/../codex/${SHA_A}.pack`,
      `/v1/github/openai/codex.git/${SHA_A}.pack`,
      `/v1/github/openai/codex/${SHA_A.toUpperCase()}.pack`,
      `/v1/github/openai/codex/abc.pack`,
      `/v1/github/open%2Fai/codex/${SHA_A}.pack`,
      `/v1/github/openai/codex/x/${SHA_A}.pack`,
    ]) {
      assert.equal(parsePackPath(bad), null, bad);
    }
    assert.deepEqual(parseRepoPath("/v1/github/openai/codex"), { owner: "openai", repo: "codex" });
  });

  it("accepts real branch names and rejects unsafe ones", () => {
    for (const ok of ["main", "release/1.2", "feat/x_y-z", "v1.0+build"]) assert.ok(isSafeBranch(ok), ok);
    for (const bad of ["", "-rf", "a..b", "a/.hidden", "x.lock", "a b", "a\nb", "/main", "main/"]) {
      assert.equal(isSafeBranch(bad), false, JSON.stringify(bad));
    }
  });
});

describe("pkt-line", () => {
  it("encodes the length prefix including itself", () => {
    assert.equal(pkt("command=ls-refs\n"), "0014command=ls-refs\n");
  });

  it("reads pkt-lines split across arbitrary chunk boundaries", async () => {
    const bytes = enc.encode(pkt("hello\n") + "0001" + pkt("world\n") + "0000");
    const reader = new PktLineReader(streamOf(bytes, 3));
    const seen = [];
    for (;;) {
      const item = await reader.next();
      if (!item) break;
      seen.push(item.kind === "data" ? new TextDecoder().decode(item.payload) : item.kind);
    }
    assert.deepEqual(seen, ["hello\n", "delim", "world\n", "flush"]);
  });

  it("rejects truncated input", async () => {
    const reader = new PktLineReader(streamOf(enc.encode("000ahel"), 2));
    await assert.rejects(() => reader.next(), /truncated/);
  });
});

describe("upload-pack client", () => {
  it("builds exact protocol v2 requests with no tags and no thin packs", () => {
    assert.equal(
      lsRefsBody("main"),
      "0014command=ls-refs\n" + "0014agent=wit-cache\n" + "0001" + "001fref-prefix refs/heads/main\n" + "0000",
    );
    const body = fetchBody(SHA_A);
    assert.match(body, new RegExp(`want ${SHA_A}\\n`));
    assert.match(body, /deepen 1\n/);
    assert.doesNotMatch(body, /include-tag|thin-pack/);
  });

  it("ls-refs matches the branch exactly, not by prefix", async () => {
    const gh = fakeGitHub({ "o/r": { refs: { "main-with-prs": SHA_B, main: SHA_A }, packs: {} } });
    assert.equal(await lsRefs("o", "r", "main", { fetchImpl: gh.fetchImpl }), SHA_A);
    assert.equal(await lsRefs("o", "r", "nope", { fetchImpl: gh.fetchImpl }), null);
  });

  it("maps an anonymous 401 to not_found_or_private and never sends credentials", async () => {
    const gh = fakeGitHub({});
    await assert.rejects(
      () => lsRefs("o", "private", "main", { fetchImpl: gh.fetchImpl }),
      (err) => err instanceof FillError && err.reason === "not_found_or_private",
    );
    for (const req of gh.requests) {
      assert.equal(req.headers.get("authorization"), null);
      assert.equal(req.headers.get("git-protocol"), "version=2");
    }
  });

  it("maps 429 to rate_limited with Retry-After", async () => {
    const fetchImpl = async () => new Response("slow down", { status: 429, headers: { "retry-after": "120" } });
    await assert.rejects(
      () => lsRefs("o", "r", "main", { fetchImpl }),
      (err) => err instanceof FillError && err.reason === "rate_limited" && err.retryAfterSeconds === 120,
    );
  });

  it("demultiplexes side-band 1 pack bytes and skips progress", async () => {
    const pack = makePack(200_000);
    const gh = fakeGitHub({ "o/r": { refs: { main: SHA_A }, packs: { [SHA_A]: pack } } });
    const chunks = [];
    const { shallow } = await fetchPack("o", "r", SHA_A, (c) => void chunks.push(c.slice()), {
      fetchImpl: gh.fetchImpl,
    });
    assert.deepEqual(shallow, [SHA_A]);
    const got = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
    let off = 0;
    for (const c of chunks) {
      got.set(c, off);
      off += c.length;
    }
    assert.deepEqual(got, pack);
  });

  it("maps 'not our ref' to not_tip", async () => {
    const gh = fakeGitHub({ "o/r": { refs: { main: SHA_A }, packs: {} } });
    await assert.rejects(
      () => fetchPack("o", "r", SHA_B, () => {}, { fetchImpl: gh.fetchImpl }),
      (err) => err instanceof FillError && err.reason === "not_tip",
    );
  });
});

describe("pack verifier", () => {
  /** @param {Uint8Array} bytes @param {number} max */
  function verify(bytes, max = 1 << 30) {
    const v = new PackVerifier(max);
    for (let i = 0; i < bytes.length; i += 333) v.update(bytes.subarray(i, i + 333));
    return v.finish();
  }

  it("accepts a well-formed pack", () => {
    assert.equal(verify(makePack(5000)).objects, 3);
  });

  it("rejects a corrupted trailer, bad magic, and truncation", () => {
    const pack = makePack(5000);
    const flipped = pack.slice();
    flipped[100] ^= 0xff;
    assert.throws(() => verify(flipped), /checksum/);
    const magic = pack.slice();
    magic[0] = 0x51;
    assert.throws(() => verify(magic), /not a git pack/);
    assert.throws(() => verify(pack.subarray(0, 10)), /truncated/);
  });

  it("aborts as too_large as soon as the cap is crossed", () => {
    assert.throws(
      () => verify(makePack(5000), 1000),
      (err) => err instanceof FillError && err.reason === "too_large",
    );
  });
});

describe("R2 pack upload", () => {
  it("uses one PUT when the pack fits in a part", async () => {
    const bucket = new FakeBucket();
    const up = new PackUpload(/** @type {any} */ (bucket), "k", { partSize: 1 << 20 });
    const pack = makePack(1000);
    await up.write(pack);
    assert.deepEqual(await up.finish(), { bytes: pack.length, parts: 1 });
    assert.equal(bucket.ops.put, 1);
    assert.equal(bucket.ops.createMultipart, 0);
    assert.deepEqual(await readAll((await bucket.get("k")).body), pack);
  });

  it("streams multipart with equal-size parts for large packs", async () => {
    const bucket = new FakeBucket({ minPartBytes: 4096 });
    const up = new PackUpload(/** @type {any} */ (bucket), "k", { partSize: 4096 });
    const pack = makePack(50_000);
    for (let i = 0; i < pack.length; i += 1500) await up.write(pack.subarray(i, i + 1500));
    const done = await up.finish();
    assert.equal(done.parts, Math.ceil(pack.length / 4096));
    assert.deepEqual(await readAll((await bucket.get("k")).body), pack);
  });
});

describe("shared helpers", () => {
  it("auth.js is an exact copy of the url-api scrubbing helpers", () => {
    const here = readFileSync(new URL("../src/auth.js", import.meta.url), "utf8");
    const there = readFileSync(new URL("../../../showcase/url-api/lib/auth.js", import.meta.url), "utf8");
    assert.equal(here, there, "copy showcase/url-api/lib/auth.js to services/wit-cache/src/auth.js");
  });
});
