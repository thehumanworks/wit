/**
 * Anonymous git smart-HTTP protocol v2 client for github.com.
 *
 * The fill never sends credentials, so private repositories cannot enter the
 * cache: GitHub answers an anonymous upload-pack for them with 401.
 */

import { DELIM, FLUSH, PktLineReader, pkt, pktText } from "./pktline.js";

export const USER_AGENT = "git/2.43.0 wit-cache (+https://github.com/thehumanworks/wit)";
const AGENT = "agent=wit-cache\n";

/**
 * Fill failure with a stable machine-readable reason. Messages avoid the
 * words that scrubSecrets treats as credential prefixes.
 */
export class FillError extends Error {
  /**
   * @param {"not_found_or_private"|"not_tip"|"too_large"|"rate_limited"|"upstream_error"|"bad_pack"|"timeout"} reason
   * @param {string} message
   * @param {{ retryAfterSeconds?: number | null, bytesRead?: number }} [opts]
   */
  constructor(reason, message, opts = {}) {
    super(message);
    this.name = "FillError";
    this.reason = reason;
    this.retryAfterSeconds = opts.retryAfterSeconds ?? null;
    this.bytesRead = opts.bytesRead ?? 0;
  }
}

/**
 * @param {string} owner
 * @param {string} repo
 */
export function uploadPackUrl(owner, repo) {
  return `https://github.com/${owner}/${repo}.git/git-upload-pack`;
}

/** @param {string} branch */
export function lsRefsBody(branch) {
  return (
    pkt("command=ls-refs\n") + pkt(AGENT) + DELIM + pkt(`ref-prefix refs/heads/${branch}\n`) + FLUSH
  );
}

/**
 * Depth-1 fetch of exactly one commit: no tags (no `include-tag`), no thin
 * pack (the client has no base objects), no progress chatter.
 * @param {string} commit
 */
export function fetchBody(commit) {
  return (
    pkt("command=fetch\n") +
    pkt(AGENT) +
    DELIM +
    pkt("no-progress\n") +
    pkt("ofs-delta\n") +
    pkt(`want ${commit}\n`) +
    pkt("deepen 1\n") +
    pkt("done\n") +
    FLUSH
  );
}

/**
 * @param {typeof fetch} fetchImpl
 * @param {string} url
 * @param {string} body
 * @param {AbortSignal | undefined} signal
 */
async function post(fetchImpl, url, body, signal) {
  let res;
  try {
    res = await fetchImpl(url, {
      method: "POST",
      headers: {
        "Git-Protocol": "version=2",
        "Content-Type": "application/x-git-upload-pack-request",
        Accept: "application/x-git-upload-pack-result",
        "User-Agent": USER_AGENT,
      },
      body,
      redirect: "manual",
      signal,
    });
  } catch (err) {
    if (signal?.aborted) throw new FillError("timeout", "GitHub fetch exceeded the fill time limit");
    throw new FillError("upstream_error", `GitHub request failed: ${errorText(err)}`);
  }
  if (res.status === 200) return res;
  await res.body?.cancel().catch(() => {});
  if (res.status === 401 || res.status === 404 || (res.status >= 300 && res.status < 400)) {
    throw new FillError(
      "not_found_or_private",
      "GitHub rejected the anonymous fetch (missing, renamed, or private repository); only public repositories are cached",
    );
  }
  if (res.status === 429 || (res.status === 403 && res.headers.get("retry-after"))) {
    const retry = Number(res.headers.get("retry-after"));
    throw new FillError("rate_limited", "GitHub rate limited the cache host", {
      retryAfterSeconds: Number.isFinite(retry) && retry > 0 ? retry : null,
    });
  }
  if (res.status === 403) {
    throw new FillError("not_found_or_private", "GitHub refused anonymous access to the repository");
  }
  throw new FillError("upstream_error", `GitHub upload-pack answered HTTP ${res.status}`);
}

/** @param {unknown} err */
function errorText(err) {
  return err instanceof Error ? err.message : String(err);
}

/**
 * Resolve `refs/heads/{branch}` with an anonymous ls-refs.
 * @param {string} owner
 * @param {string} repo
 * @param {string} branch
 * @param {{ fetchImpl?: typeof fetch, signal?: AbortSignal }} [opts]
 * @returns {Promise<string | null>} tip commit, or null when the branch does not exist
 */
export async function lsRefs(owner, repo, branch, opts = {}) {
  const res = await post(opts.fetchImpl ?? fetch, uploadPackUrl(owner, repo), lsRefsBody(branch), opts.signal);
  const reader = new PktLineReader(/** @type {ReadableStream<Uint8Array>} */ (res.body));
  const wanted = `refs/heads/${branch}`;
  let tip = null;
  try {
    for (;;) {
      const item = await reader.next();
      if (!item || item.kind !== "data") break;
      const line = pktText(item.payload);
      if (line.startsWith("ERR ")) throw new FillError("upstream_error", `GitHub ls-refs error: ${line.slice(4)}`);
      const [oid, name] = line.split(" ");
      if (name === wanted && /^[0-9a-f]{40}$/.test(oid)) tip = oid;
    }
  } catch (err) {
    if (err instanceof FillError) throw err;
    throw new FillError(opts.signal?.aborted ? "timeout" : "upstream_error", `ls-refs response unreadable: ${errorText(err)}`);
  } finally {
    reader.cancel();
  }
  return tip;
}

/**
 * Fetch the depth-1 pack for `commit` and hand pack bytes to `onPack` as they
 * arrive (side-band channel 1). Nothing is buffered beyond one pkt-line.
 * @param {string} owner
 * @param {string} repo
 * @param {string} commit
 * @param {(chunk: Uint8Array) => Promise<void> | void} onPack
 * @param {{ fetchImpl?: typeof fetch, signal?: AbortSignal }} [opts]
 * @returns {Promise<{ shallow: string[] }>}
 */
export async function fetchPack(owner, repo, commit, onPack, opts = {}) {
  const res = await post(opts.fetchImpl ?? fetch, uploadPackUrl(owner, repo), fetchBody(commit), opts.signal);
  const reader = new PktLineReader(/** @type {ReadableStream<Uint8Array>} */ (res.body));
  /** @type {string[]} */
  const shallow = [];
  let inPack = false;
  let sawPack = false;
  try {
    for (;;) {
      const item = await reader.next();
      if (!item) break;
      if (item.kind !== "data") {
        if (inPack && item.kind === "flush") break;
        continue;
      }
      if (!inPack) {
        const line = pktText(item.payload);
        if (line.startsWith("ERR ")) {
          const reason = /not our ref|not a valid|unadvertised/i.test(line) ? "not_tip" : "upstream_error";
          throw new FillError(reason, `GitHub fetch error: ${line.slice(4)}`);
        }
        if (line.startsWith("shallow ")) shallow.push(line.slice(8));
        if (line === "packfile") {
          inPack = true;
          sawPack = true;
        }
        continue;
      }
      const band = item.payload[0];
      if (band === 1) {
        await onPack(item.payload.subarray(1));
      } else if (band === 3) {
        throw new FillError("upstream_error", `GitHub fetch error: ${pktText(item.payload.subarray(1))}`);
      }
    }
  } catch (err) {
    if (err instanceof FillError) throw err;
    throw new FillError(opts.signal?.aborted ? "timeout" : "upstream_error", `pack stream failed: ${errorText(err)}`);
  } finally {
    reader.cancel();
  }
  if (!sawPack) throw new FillError("upstream_error", "GitHub fetch response had no packfile section");
  return { shallow };
}
