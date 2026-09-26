/**
 * git pkt-line framing (gitprotocol-common): 4 hex digits of total length,
 * then payload. `0000` flush, `0001` delim, `0002` response-end.
 */

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export const FLUSH = "0000";
export const DELIM = "0001";
/** Largest pkt-line git allows (length prefix included). */
export const MAX_PKT_LEN = 65520;

/** @param {string} line */
export function pkt(line) {
  const bytes = encoder.encode(line);
  const len = bytes.length + 4;
  if (len > MAX_PKT_LEN) throw new Error("pkt-line too long");
  return len.toString(16).padStart(4, "0") + line;
}

/**
 * Buffered reader over a byte stream that yields pkt-lines without holding
 * more than one pkt-line plus one upstream chunk in memory.
 */
export class PktLineReader {
  /** @param {ReadableStream<Uint8Array>} stream */
  constructor(stream) {
    this.reader = stream.getReader();
    /** @type {Uint8Array} */
    this.buf = new Uint8Array(0);
    this.off = 0;
    this.done = false;
  }

  /** @param {number} n */
  async fill(n) {
    while (this.buf.length - this.off < n) {
      if (this.done) return false;
      const { value, done } = await this.reader.read();
      if (done) {
        this.done = true;
        continue;
      }
      if (!value || value.length === 0) continue;
      const rest = this.buf.length - this.off;
      if (rest === 0) {
        this.buf = value;
      } else {
        const merged = new Uint8Array(rest + value.length);
        merged.set(this.buf.subarray(this.off), 0);
        merged.set(value, rest);
        this.buf = merged;
      }
      this.off = 0;
    }
    return true;
  }

  /**
   * @returns {Promise<
   *   | { kind: "flush" | "delim" | "end" }
   *   | { kind: "data", payload: Uint8Array }
   *   | null
   * >} null at end of stream
   */
  async next() {
    if (!(await this.fill(4))) {
      if (this.buf.length - this.off > 0) throw new Error("truncated pkt-line header");
      return null;
    }
    const head = decoder.decode(this.buf.subarray(this.off, this.off + 4));
    if (!/^[0-9a-f]{4}$/.test(head)) throw new Error("malformed pkt-line header");
    const len = parseInt(head, 16);
    if (len === 0 || len === 1 || len === 2) {
      this.off += 4;
      return { kind: len === 0 ? "flush" : len === 1 ? "delim" : "end" };
    }
    if (len < 4 || len > MAX_PKT_LEN) throw new Error("invalid pkt-line length");
    if (!(await this.fill(len))) throw new Error("truncated pkt-line");
    const payload = this.buf.subarray(this.off + 4, this.off + len);
    this.off += len;
    return { kind: "data", payload };
  }

  cancel() {
    this.reader.cancel().catch(() => {});
  }
}

/** @param {Uint8Array} payload */
export function pktText(payload) {
  const text = decoder.decode(payload);
  return text.endsWith("\n") ? text.slice(0, -1) : text;
}
