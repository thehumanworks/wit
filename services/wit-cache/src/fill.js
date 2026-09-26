/**
 * One on-demand fill: confirm the commit is the current tip of the branch the
 * client named, fetch its depth-1 pack anonymously, verify framing, store it.
 *
 * Requiring a branch tip keeps the property that the filler never stores an
 * arbitrary requested SHA (GitHub serves any fork-network commit through the
 * parent's URL).
 */

import { packKey } from "./keys.js";
import { PackUpload, PackVerifier } from "./store.js";
import { FillError, fetchPack, lsRefs } from "./upload-pack.js";

/**
 * @typedef {{ owner: string, repo: string, commit: string, branch: string }} FillJob
 */

/**
 * @param {{ PACKS: R2Bucket }} env
 * @param {FillJob} job
 * @param {ReturnType<import("./config.js").limitsFromEnv>} limits
 * @param {{ fetchImpl?: typeof fetch, now?: () => number }} [deps]
 * @returns {Promise<{ bytes: number, parts: number, reused: boolean }>}
 */
export async function fillPack(env, job, limits, deps = {}) {
  const now = deps.now ?? Date.now;
  const key = packKey(job.owner, job.repo, job.commit);
  const existing = await env.PACKS.head(key);
  if (existing) return { bytes: existing.size, parts: 0, reused: true };

  const signal = AbortSignal.timeout(limits.FILL_TIMEOUT_MS);
  const opts = { fetchImpl: deps.fetchImpl, signal };
  const tip = await lsRefs(job.owner, job.repo, job.branch, opts);
  if (tip !== job.commit) {
    throw new FillError(
      "not_tip",
      tip ? "commit is not the current tip of the named branch" : "branch does not exist",
    );
  }

  const verifier = new PackVerifier(limits.MAX_PACK_BYTES);
  const upload = new PackUpload(env.PACKS, key, {
    partSize: limits.PART_BYTES,
    httpMetadata: { contentType: "application/x-git-packfile" },
    customMetadata: {
      commit: job.commit,
      branch: job.branch,
      repo: `${job.owner}/${job.repo}`,
      filled_at: new Date(now()).toISOString(),
    },
  });
  try {
    await fetchPack(
      job.owner,
      job.repo,
      job.commit,
      async (chunk) => {
        verifier.update(chunk);
        await upload.write(chunk);
      },
      opts,
    );
    verifier.finish();
    const done = await upload.finish();
    return { ...done, reused: false };
  } catch (err) {
    await upload.abort();
    if (err instanceof FillError) {
      if (!err.bytesRead) err.bytesRead = verifier.bytes;
      throw err;
    }
    throw new FillError("upstream_error", err instanceof Error ? err.message : String(err), {
      bytesRead: verifier.bytes,
    });
  }
}
