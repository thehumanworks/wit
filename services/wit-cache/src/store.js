/**
 * Streaming pack storage: verify the pack framing on the fly and write it to
 * R2 with a single PUT (small packs) or a multipart upload (large packs),
 * holding at most one part in memory.
 */

import { createHash } from "node:crypto";
import { FillError } from "./upload-pack.js";

const TRAILER = 20;

/**
 * Checks what the Worker can check cheaply: `PACK` magic, version 2/3, a
 * non-zero object count, the size cap, and the SHA-1 trailer over the whole
 * stream. Object-level integrity is the client's job (`index-pack --strict`).
 */
export class PackVerifier {
  /** @param {number} maxBytes */
  constructor(maxBytes) {
    this.maxBytes = maxBytes;
    this.bytes = 0;
    this.hash = createHash("sha1");
    /** @type {Uint8Array} */
    this.head = new Uint8Array(0);
    /** @type {Uint8Array} */
    this.tail = new Uint8Array(0);
  }

  /** @param {Uint8Array} chunk */
  update(chunk) {
    this.bytes += chunk.length;
    if (this.bytes > this.maxBytes) {
      throw new FillError("too_large", `pack exceeds the ${this.maxBytes}-byte cache cap`, {
        bytesRead: this.bytes,
      });
    }
    if (this.head.length < 12) {
      const need = 12 - this.head.length;
      const merged = new Uint8Array(this.head.length + Math.min(need, chunk.length));
      merged.set(this.head, 0);
      merged.set(chunk.subarray(0, need), this.head.length);
      this.head = merged;
    }
    // Hold back the last 20 bytes seen: they are the trailer if the stream ends here.
    if (chunk.length >= TRAILER) {
      if (this.tail.length) this.hash.update(this.tail);
      this.hash.update(chunk.subarray(0, chunk.length - TRAILER));
      this.tail = chunk.slice(chunk.length - TRAILER);
      return;
    }
    const joined = new Uint8Array(this.tail.length + chunk.length);
    joined.set(this.tail, 0);
    joined.set(chunk, this.tail.length);
    const cut = Math.max(0, joined.length - TRAILER);
    if (cut > 0) this.hash.update(joined.subarray(0, cut));
    this.tail = joined.slice(cut);
  }

  /** @returns {{ objects: number, bytes: number }} */
  finish() {
    const bad = (/** @type {string} */ msg) =>
      new FillError("bad_pack", msg, { bytesRead: this.bytes });
    if (this.head.length < 12 || this.tail.length < TRAILER) throw bad("pack is truncated");
    const magic = String.fromCharCode(...this.head.subarray(0, 4));
    const view = new DataView(this.head.buffer, this.head.byteOffset, 12);
    const version = view.getUint32(4);
    const objects = view.getUint32(8);
    if (magic !== "PACK" || (version !== 2 && version !== 3)) throw bad("stream is not a git pack");
    if (objects === 0) throw bad("pack has no objects");
    const digest = new Uint8Array(this.hash.digest());
    for (let i = 0; i < TRAILER; i++) {
      if (digest[i] !== this.tail[i]) throw bad("pack checksum mismatch");
    }
    return { objects, bytes: this.bytes };
  }
}

/**
 * R2 writer. All parts except the last have exactly `partSize` bytes, which
 * R2 multipart requires.
 */
export class PackUpload {
  /**
   * @param {R2Bucket} bucket
   * @param {string} key
   * @param {{ partSize: number, httpMetadata?: R2HTTPMetadata, customMetadata?: Record<string,string> }} opts
   */
  constructor(bucket, key, opts) {
    this.bucket = bucket;
    this.key = key;
    this.partSize = opts.partSize;
    this.options = { httpMetadata: opts.httpMetadata, customMetadata: opts.customMetadata };
    this.part = new Uint8Array(this.partSize);
    this.fill = 0;
    this.bytes = 0;
    /** @type {R2MultipartUpload | null} */
    this.upload = null;
    /** @type {R2UploadedPart[]} */
    this.parts = [];
  }

  /** @param {Uint8Array} chunk */
  async write(chunk) {
    this.bytes += chunk.length;
    let rest = chunk;
    while (rest.length > 0) {
      const n = Math.min(rest.length, this.partSize - this.fill);
      this.part.set(rest.subarray(0, n), this.fill);
      this.fill += n;
      rest = rest.subarray(n);
      if (this.fill === this.partSize) await this.flushPart();
    }
  }

  async flushPart() {
    if (!this.upload) this.upload = await this.bucket.createMultipartUpload(this.key, this.options);
    const uploaded = await this.upload.uploadPart(this.parts.length + 1, this.part.subarray(0, this.fill));
    this.parts.push(uploaded);
    this.fill = 0;
  }

  /** @returns {Promise<{ bytes: number, parts: number }>} */
  async finish() {
    if (!this.upload) {
      await this.bucket.put(this.key, this.part.subarray(0, this.fill), this.options);
      return { bytes: this.bytes, parts: 1 };
    }
    if (this.fill > 0) await this.flushPart();
    await this.upload.complete(this.parts);
    return { bytes: this.bytes, parts: this.parts.length };
  }

  async abort() {
    if (this.upload) await this.upload.abort().catch(() => {});
    this.upload = null;
  }
}
