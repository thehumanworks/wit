/**
 * Shared request handler for browser page + Cloudflare Pages Function.
 *
 * Snapshot verbs (tree/ls/cat/head/tail/rg/stats/outline) run over the
 * MemoryBackend wasm (open/list/read) with the host cache in front of
 * `get_json`. Metadata verbs (refs/commits) and repository `search` call the
 * GitHub REST API from the host directly. Every response carries provenance
 * headers (`x-wit-repo`, `x-wit-ref`, `x-wit-commit`, `x-wit-cache`) so an
 * agent knows exactly which commit it read.
 */

import {
  extractToken,
  SafeError,
  safeConsole,
  scrubSecrets,
  withActiveSecrets,
} from "./auth.js";
import { apiIndexText, llmsText, openApiDocument } from "./discovery.js";
import {
  formatCat,
  formatCommits,
  formatLs,
  formatRefs,
  formatRgCounts,
  formatRgFiles,
  formatRgMatches,
  formatSearch,
  formatTree,
} from "./format.js";
import {
  MAX_BLOB_BYTES,
  blobShaForPath,
  createHostCache,
  githubFailure,
  githubGet,
  prefetchBlob,
  prefetchOpen,
  treeRowsFor,
} from "./github.js";
import { formatOutline, outlineFile } from "./outline.js";
import { errorBody, isApiPath, parseRoute } from "./routes.js";
import { computeStats, formatStats } from "./stats.js";
import {
  compileSearchRegex,
  globToRegExp,
  grepText,
  headFromText,
  rustLines,
  sliceLines,
  tailFromText,
} from "./textops.js";
import {
  collectTreeFiles,
  loadWasm,
  wasmList,
  wasmOpen,
  wasmRead,
} from "./wasm-host.js";
import { resolveRefName, ttlFromSearchParams } from "./repo-cache.js";

export const API_VERSION = "2";

/**
 * @typedef {{
 *   loadWasmBytes: () => Promise<BufferSource | Response | WebAssembly.Module>,
 *   cache?: import('./repo-cache.js').RepoSnapshotCache,
 *   persistentCache?: import('./persistent-cache.js').KvRepoCache,
 *   waitUntil?: (promise: Promise<unknown>) => void,
 *   serverToken?: string | null,
 *   rawBlobs?: boolean,
 *   blobConcurrency?: number,
 * }} HandlerDeps
 *
 * `serverToken` is the host's own GitHub token, used only when the caller
 * sends none. It lifts the anonymous 60 req/h limit that shared Worker egress
 * IPs exhaust permanently. It must be a token with public read access only.
 */

/** Isolate-lifetime cache for the worker (browser passes its own). */
let defaultCache = null;

/**
 * @param {HandlerDeps} [deps]
 */
function getCache(deps) {
  if (deps?.cache) return deps.cache;
  if (!defaultCache) defaultCache = createHostCache();
  return defaultCache;
}

/**
 * Handle an API or static-miss request.
 * Returns null when the path is not an API route (caller serves static).
 *
 * @param {Request} request
 * @param {HandlerDeps} deps
 * @returns {Promise<Response | null>}
 */
export async function handleRequest(request, deps) {
  const url = new URL(request.url);
  if (!isApiPath(url.pathname)) return null;

  if (request.method !== "GET" && request.method !== "HEAD") {
    return textResponse("error: method not allowed\n", 405, { allow: "GET, HEAD" });
  }

  const token = extractToken({ headers: request.headers, url });
  // Scrub the winning Authorization PAT, any ?token= fallback, and the host's
  // own token so a mistaken log of request.url cannot leak any of them.
  const queryToken = url.searchParams.get("token") || url.searchParams.get("access_token");
  const secrets = [token, queryToken, deps?.serverToken].filter(Boolean);

  return withActiveSecrets(secrets, () => handleRequestInner(request, deps, url, token));
}

/**
 * @param {Request} request
 * @param {HandlerDeps} deps
 * @param {URL} url
 * @param {string | null} callerToken
 */
async function handleRequestInner(request, deps, url, callerToken) {
  let format = "text";
  try {
    const route = parseRoute(url, { accept: request.headers.get("accept") });
    if (!route) return null;

    if (route.kind === "api-index") {
      return bodyResponse(request, apiIndexText(url));
    }
    if (route.kind === "openapi") {
      return bodyResponse(
        request,
        `${JSON.stringify(openApiDocument(url), null, 2)}\n`,
        { contentType: "application/json; charset=utf-8" },
      );
    }
    if (route.kind === "llms") {
      return bodyResponse(request, llmsText(url));
    }

    format = route.format;
    const auth = resolveAuth(callerToken, deps);
    const baseHeaders = {
      "x-wit-auth": auth.source,
      "x-wit-api-version": API_VERSION,
    };

    if (route.kind === "search") {
      const result = await searchRepositories(route, auth);
      return respond(request, route.format, result, baseHeaders, auth);
    }

    if (route.verb === "refs") {
      const result = await listRefs(route, auth);
      return respond(request, route.format, result, baseHeaders, auth);
    }
    if (route.verb === "commits") {
      const result = await listCommits(route, auth);
      return respond(request, route.format, result, baseHeaders, auth);
    }

    const ttlMs = ttlFromSearchParams(url.search);
    const result = await runSnapshotVerb(route, deps, auth, ttlMs);
    return respond(request, route.format, result, { ...baseHeaders, ...result.headers }, auth);
  } catch (err) {
    const { status, body, retryAfter } = errorBody(err, { format });
    // Never log raw token-bearing URLs / PATs (safeConsole scrubs every arg).
    safeConsole.error("url-api error", err?.message || err, String(url));
    /** @type {Record<string, string>} */
    const extra = {};
    if (retryAfter != null) extra["retry-after"] = String(retryAfter);
    return textResponse(body, status, extra, format === "json" ? "application/json; charset=utf-8" : undefined);
  }
}

/**
 * @typedef {{ token: string | null, source: 'caller' | 'host' | 'anonymous' }} Auth
 */

/**
 * Caller token wins; the host token is the fallback; otherwise anonymous.
 * @param {string | null} callerToken
 * @param {HandlerDeps} deps
 * @returns {Auth}
 */
function resolveAuth(callerToken, deps) {
  if (callerToken) return { token: callerToken, source: "caller" };
  const host = typeof deps?.serverToken === "string" ? deps.serverToken.trim() : "";
  if (host) return { token: host, source: "host" };
  return { token: null, source: "anonymous" };
}

/**
 * @typedef {{ text: string, json: Record<string, unknown>, headers?: Record<string, string> }} VerbResult
 */

/**
 * Snapshot-backed verbs: open the pinned commit through the wasm backend and
 * run the requested view over it.
 *
 * @param {import('./routes.js').RepoRoute} route
 * @param {HandlerDeps} deps
 * @param {Auth} auth
 * @param {number | null} ttlMs per-request TTL override (`?ttl=` / `?ttlMs=`)
 * @returns {Promise<VerbResult>}
 */
async function runSnapshotVerb(route, deps, auth, ttlMs) {
  const cache = getCache(deps);
  const wasmSource = await deps.loadWasmBytes();
  const api = await loadWasm(wasmSource, cache);
  const persistent = deps.persistentCache ?? null;

  if (route.fresh) {
    cache.evictOpenEntry(route.ownerRepo, route.branch ?? undefined);
  } else if (persistent) {
    // Persistence is best-effort: a KV failure must never fail the read.
    try {
      await persistent.hydrateOpen(cache, route.ownerRepo, route.branch);
    } catch (err) {
      safeConsole.error("persistent cache hydrate failed", err?.message || err);
    }
  }

  const opened = await prefetchOpen(cache, route.ownerRepo, route.branch, auth.token, {
    tokenSource: auth.source,
    ttlMs,
  });
  wasmOpen(api, route.ownerRepo, route.branch);

  const entry = cache.findOpenEntry(route.ownerRepo, route.branch ?? undefined);
  const requestedRef = route.branch ?? opened.ref;
  const provenance = {
    repo: route.ownerRepo,
    requested_ref: requestedRef,
    // A request for a commit SHA is reported as that SHA even when the cache
    // answered it from the branch entry whose head it is.
    ref: /^[0-9a-f]{40}$/i.test(requestedRef)
      ? requestedRef
      : (entry?.resolvedRef ?? resolveRefName(requestedRef)),
    commit: opened.commitSha,
    cache: opened.cached ? "hit" : "miss",
  };
  const headers = {
    "x-wit-repo": provenance.repo,
    "x-wit-ref": provenance.ref,
    "x-wit-commit": provenance.commit,
    "x-wit-cache": provenance.cache,
  };

  const ctx = {
    route,
    deps,
    auth,
    cache,
    api,
    persistent,
    provenance,
    ttlMs,
    isIgnored: ignoreMatcher(route.ignore),
  };

  let result;
  switch (route.verb) {
    case "ls":
      result = verbLs(ctx);
      break;
    case "tree":
      result = verbTree(ctx);
      break;
    case "stats":
      result = verbStats(ctx);
      break;
    case "cat":
      result = await verbCat(ctx);
      break;
    case "head":
      result = await verbHead(ctx);
      break;
    case "tail":
      result = await verbTail(ctx);
      break;
    case "outline":
      result = await verbOutline(ctx);
      break;
    case "rg":
      result = await verbRg(ctx);
      break;
    default:
      throw new SafeError("unknown verb", { status: 400, code: "bad_verb" });
  }

  if (persistent) {
    const persisted = persistent
      .persistRepo(cache, route.ownerRepo)
      .catch((err) => {
        safeConsole.error("persistent cache write failed", err?.message || err);
      });
    if (deps.waitUntil) deps.waitUntil(persisted);
    else await persisted;
  }

  return {
    text: result.text,
    json: { api_version: API_VERSION, verb: route.verb, ...provenance, ...result.json },
    headers,
  };
}

/**
 * `?ignore=` globs (repeatable / comma separated) exclude paths from
 * tree/ls/stats/rg, like the CLI's global `--ignore`.
 * @param {string[]} globs
 * @returns {(path: string) => boolean}
 */
function ignoreMatcher(globs) {
  const regexes = [];
  for (const glob of globs) {
    const re = globToRegExp(glob);
    if (re) regexes.push(re);
    // `dir/**` should hide `dir` itself in listings, not only its contents.
    if (glob.endsWith("/**")) {
      const dir = globToRegExp(glob.slice(0, -3));
      if (dir) regexes.push(dir);
    }
  }
  if (regexes.length === 0) return () => false;
  return (path) => {
    const parts = path.split("/");
    return regexes.some(
      (re) => re.test(path) || parts.some((_, i) => re.test(parts.slice(0, i + 1).join("/"))),
    );
  };
}

/**
 * @typedef {{
 *   route: import('./routes.js').RepoRoute,
 *   deps: HandlerDeps,
 *   auth: Auth,
 *   cache: import('./repo-cache.js').RepoSnapshotCache,
 *   api: WebAssembly.Exports,
 *   persistent: import('./persistent-cache.js').KvRepoCache | null,
 *   provenance: { repo: string, requested_ref: string, ref: string, commit: string, cache: string },
 *   ttlMs: number | null,
 *   isIgnored: (path: string) => boolean,
 * }} VerbCtx
 */

/** @param {VerbCtx} ctx */
function verbLs(ctx) {
  const { route, api } = ctx;
  const entries = wasmList(api, route.path)
    .map((e) => ({
      name: e.name,
      kind: e.kind === "Dir" || e.kind === "dir" ? "dir" : "file",
      path: e.path,
      size_bytes: e.size_bytes ?? null,
      blob_sha: e.blob_sha ?? null,
    }))
    .filter((e) => !ctx.isIgnored(e.path));
  return {
    text: formatLs(entries, { long: route.long }),
    json: {
      path: route.path || ".",
      entries: entries.map((e) => ({
        name: e.name,
        kind: e.kind,
        path: e.path,
        size_bytes: e.size_bytes,
        tokens_est: e.kind === "file" && e.size_bytes != null ? Math.ceil(e.size_bytes / 4) : null,
        blob_sha: e.kind === "file" ? e.blob_sha : null,
      })),
    },
  };
}

/** @param {VerbCtx} ctx */
function verbTree(ctx) {
  const { route, api } = ctx;
  const files = collectTreeFiles(api, route.path, route.depth).filter((f) => !ctx.isIgnored(f.path));
  const root = route.path ? route.path.split("/").filter(Boolean).pop() || route.path : ".";
  return {
    text: formatTree(
      { root, entries: files },
      { path: route.path, depth: route.depth, long: route.long },
    ),
    json: {
      path: route.path || ".",
      depth: route.depth,
      files: files.map((f) => ({
        path: f.path,
        size_bytes: f.size_bytes ?? null,
        tokens_est: f.size_bytes != null ? Math.ceil(f.size_bytes / 4) : null,
      })),
    },
  };
}

/** @param {VerbCtx} ctx */
function verbStats(ctx) {
  const { route, cache, provenance } = ctx;
  const rows = treeRowsFor(cache, route.ownerRepo, route.branch);
  if (route.path) {
    const exists = rows.some((r) => r.path === route.path || r.path.startsWith(`${route.path}/`));
    if (!exists) {
      throw new SafeError(`path not found: ${route.path}`, { status: 404, code: "not_found" });
    }
  }
  const stats = computeStats(rows, route.path, { largest: route.largest, isIgnored: ctx.isIgnored });
  return { text: formatStats(stats, provenance), json: { ...stats } };
}

/**
 * Read one file's text through the wasm backend, prefetching its blob from
 * KV → raw.githubusercontent.com → blob endpoint in that order.
 *
 * @param {VerbCtx} ctx
 * @param {string} path
 * @returns {Promise<{ text: string, blob_sha: string, size_bytes: number }>}
 */
async function readFileText(ctx, path) {
  const { route, cache, api, persistent, auth, deps, provenance } = ctx;
  if (ctx.isIgnored(path)) {
    throw new SafeError(`File '${path}' is excluded by ?ignore=`, { status: 404, code: "ignored" });
  }
  const sha = blobShaForPath(cache, route.ownerRepo, route.branch, path);
  if (!sha) {
    const rows = treeRowsFor(cache, route.ownerRepo, route.branch);
    const isDir = rows.some((r) => r.path.startsWith(`${path}/`));
    throw new SafeError(isDir ? `Not a file: ${path}` : `File not found: ${path}`, {
      status: isDir ? 400 : 404,
      code: isDir ? "not_a_file" : "not_found",
    });
  }
  if (persistent) {
    try {
      await persistent.hydrateBlob(cache, route.ownerRepo, sha);
    } catch (err) {
      safeConsole.error("persistent blob hydrate failed", err?.message || err);
    }
  }
  await prefetchBlob(cache, route.ownerRepo, sha, auth.token, {
    path,
    commitSha: provenance.commit,
    raw: deps.rawBlobs !== false,
    tokenSource: auth.source,
    ttlMs: ctx.ttlMs,
  });
  const file = wasmRead(api, path);
  return { text: file.text, blob_sha: file.blob_sha ?? sha, size_bytes: file.size_bytes ?? 0 };
}

/** @param {VerbCtx} ctx */
async function verbCat(ctx) {
  const { route } = ctx;
  const file = await readFileText(ctx, route.path);
  const selected = sliceLines(file.text, route.lines);
  const ranged = route.lines != null;
  if (ranged && selected.lines.length === 0 && selected.total > 0) {
    throw new SafeError(
      `lines ${route.lines.start ?? 1}-${route.lines.end ?? ""} is outside ${route.path} (${selected.total} lines)`,
      { status: 416, code: "range_out_of_bounds" },
    );
  }
  const text = ranged
    ? formatCat(selected.lines, { number: route.number, startLine: selected.start })
    : formatCat(file.text, { number: route.number });
  return {
    text,
    json: {
      path: route.path,
      blob_sha: file.blob_sha,
      size_bytes: file.size_bytes,
      total_lines: selected.total,
      start_line: selected.total === 0 ? 0 : selected.start,
      end_line: selected.end,
      text: selected.lines.join("\n"),
    },
  };
}

/** @param {VerbCtx} ctx */
async function verbHead(ctx) {
  const { route } = ctx;
  const file = await readFileText(ctx, route.path);
  const total = rustLines(file.text).length;
  const count = route.count ?? 10;
  return {
    text: headFromText(file.text, count, route.number),
    json: {
      path: route.path,
      blob_sha: file.blob_sha,
      total_lines: total,
      start_line: total === 0 ? 0 : 1,
      end_line: Math.min(count, total),
      text: rustLines(file.text).slice(0, count).join("\n"),
    },
  };
}

/** @param {VerbCtx} ctx */
async function verbTail(ctx) {
  const { route } = ctx;
  const file = await readFileText(ctx, route.path);
  const all = rustLines(file.text);
  const total = all.length;
  const count = route.count ?? 10;
  const start = route.fromLine != null ? Math.min(route.fromLine, total + 1) : Math.max(1, total - count + 1);
  return {
    text: tailFromText(file.text, count, route.fromLine ?? null, route.number),
    json: {
      path: route.path,
      blob_sha: file.blob_sha,
      total_lines: total,
      start_line: total === 0 ? 0 : start,
      end_line: total,
      text: all.slice(start - 1).join("\n"),
    },
  };
}

/** @param {VerbCtx} ctx */
async function verbOutline(ctx) {
  const { route } = ctx;
  const file = await readFileText(ctx, route.path);
  const outline = outlineFile(route.path, file.text, { maxSymbols: route.maxSymbols });
  return {
    text: formatOutline(outline, route.path),
    json: {
      path: route.path,
      blob_sha: file.blob_sha,
      language: outline.language,
      supported: outline.supported,
      total_lines: outline.total_lines,
      truncated: outline.truncated,
      symbols: outline.symbols.map(({ indent, ...rest }) => rest),
    },
  };
}

/**
 * Bounded ripgrep-style search: text blobs under `path` matching `glob`, at
 * most `max_files` files and `max` matches. Blobs come from KV / raw /
 * API through the same host cache the wasm reads use.
 *
 * @param {VerbCtx} ctx
 */
async function verbRg(ctx) {
  const { route, cache, api, persistent, auth, deps, provenance } = ctx;
  let regex;
  try {
    regex = compileSearchRegex(route.pattern, {
      ignoreCase: route.ignoreCase,
      smartCase: route.smartCase,
      wordRegexp: route.wordRegexp,
    });
  } catch (err) {
    throw new SafeError(String(err.message || err), { status: 400, code: "bad_pattern" });
  }
  const glob = route.glob ? globToRegExp(route.glob) : null;
  const rows = treeRowsFor(cache, route.ownerRepo, route.branch);
  const base = route.path;
  if (base && !rows.some((r) => r.path === base || r.path.startsWith(`${base}/`))) {
    throw new SafeError(`path not found: ${base}`, { status: 404, code: "not_found" });
  }

  const candidates = rows.filter(
    (r) =>
      r.type === "blob" &&
      (base === "" || r.path === base || r.path.startsWith(`${base}/`)) &&
      (!glob || glob.test(r.path)) &&
      !ctx.isIgnored(r.path) &&
      (r.size == null || r.size <= MAX_BLOB_BYTES),
  );
  const maxFiles = route.maxFiles ?? 200;
  const maxMatches = route.maxMatches ?? 200;
  const files = candidates.slice(0, maxFiles);
  const truncatedFiles = candidates.length > files.length;

  /** @type {import('./textops.js').GrepLine[]} */
  const matchLines = [];
  /** @type {Array<{ path: string, lines: number | null }>} */
  const fileHits = [];
  /** @type {Array<{ path: string, count: number }>} */
  const counts = [];
  let totalMatches = 0;
  let scanned = 0;
  let skippedBinary = 0;
  let stopReason = null;

  const concurrency = Math.max(1, deps.blobConcurrency ?? 8);
  for (let i = 0; i < files.length && stopReason == null; i += concurrency) {
    const batch = files.slice(i, i + concurrency);
    const results = await Promise.allSettled(
      batch.map(async (row) => {
        if (persistent) {
          try {
            await persistent.hydrateBlob(cache, route.ownerRepo, row.sha);
          } catch (err) {
            safeConsole.error("persistent blob hydrate failed", err?.message || err);
          }
        }
        await prefetchBlob(cache, route.ownerRepo, row.sha, auth.token, {
          path: row.path,
          commitSha: provenance.commit,
          raw: deps.rawBlobs !== false,
          tokenSource: auth.source,
          ttlMs: ctx.ttlMs,
        });
      }),
    );
    for (let j = 0; j < batch.length; j += 1) {
      const row = batch[j];
      const settled = results[j];
      if (settled.status === "rejected") {
        const err = settled.reason;
        if (err && err.code === "rate_limited") {
          stopReason = "rate_limited";
          break;
        }
        continue; // missing / forbidden blob: skip, like the CLI does
      }
      let file;
      try {
        file = wasmRead(api, row.path);
      } catch (err) {
        if (err && (err.code === "binary" || err.code === "oversized")) skippedBinary += 1;
        continue;
      }
      scanned += 1;
      const remaining = maxMatches - totalMatches;
      if (remaining <= 0) {
        stopReason = "max_matches";
        break;
      }
      const found = grepText(row.path, file.text, regex, {
        invert: route.invert,
        before: route.before,
        after: route.after,
        maxMatches: remaining,
      });
      if (found.matchCount === 0) continue;
      totalMatches += found.matchCount;
      if (route.filesWithMatches) {
        fileHits.push({ path: row.path, lines: route.long ? rustLines(file.text).length : null });
      } else if (route.countOnly) {
        counts.push({ path: row.path, count: found.matchCount });
      } else {
        matchLines.push(...found.lines);
      }
      if (totalMatches >= maxMatches) {
        stopReason = "max_matches";
        break;
      }
    }
  }

  const hasContext = (route.before ?? 0) > 0 || (route.after ?? 0) > 0;
  let text;
  if (route.filesWithMatches) text = formatRgFiles(fileHits, { long: route.long });
  else if (route.countOnly) text = formatRgCounts(counts);
  else text = formatRgMatches(matchLines, { hasContext });

  const truncated = stopReason != null || truncatedFiles;
  const notes = [];
  if (stopReason === "rate_limited") {
    notes.push("# truncated: GitHub rate limit reached while fetching blobs; retry later or send a token");
  } else if (stopReason === "max_matches") {
    notes.push(`# truncated: reached max=${maxMatches} matches`);
  }
  if (truncatedFiles) {
    notes.push(
      `# truncated: scanned ${files.length} of ${candidates.length} candidate files (raise max_files= or narrow path=/glob=)`,
    );
  }
  if (notes.length > 0) text = text ? `${text}\n${notes.join("\n")}` : notes.join("\n");

  return {
    text,
    json: {
      pattern: route.pattern,
      path: base || ".",
      glob: route.glob,
      files_scanned: scanned,
      files_candidate: candidates.length,
      files_skipped_binary: skippedBinary,
      match_count: totalMatches,
      truncated,
      truncated_reason: stopReason ?? (truncatedFiles ? "max_files" : null),
      ...(route.filesWithMatches
        ? { files: fileHits }
        : route.countOnly
          ? { counts }
          : { matches: matchLines.filter((m) => !(m.line === 0 && m.text === "--")) }),
    },
  };
}

/**
 * `GET /refs/{owner}/{repo}`: default branch, branches, and tags from the
 * REST API (100 each — enough for `?ref=` discovery without pagination).
 *
 * @param {import('./routes.js').RepoRoute} route
 * @param {Auth} auth
 * @returns {Promise<VerbResult>}
 */
async function listRefs(route, auth) {
  const repoPath = `/repos/${route.ownerRepo}`;
  const repoRes = await githubGet(repoPath, auth.token);
  if (repoRes.status !== 200) {
    throw githubFailure(repoRes, {
      notFound: `repository '${route.ownerRepo}' was not found`,
      label: repoPath,
      tokenSource: auth.source,
    });
  }
  const meta = JSON.parse(repoRes.body);
  const [branchesRes, tagsRes] = await Promise.all([
    githubGet(`${repoPath}/branches?per_page=100`, auth.token),
    githubGet(`${repoPath}/tags?per_page=100`, auth.token),
  ]);
  for (const [res, label] of [[branchesRes, "branches"], [tagsRes, "tags"]]) {
    if (res.status !== 200) {
      throw githubFailure(res, {
        notFound: `${label} not found for '${route.ownerRepo}'`,
        label,
        tokenSource: auth.source,
      });
    }
  }
  const pick = (body) =>
    (Array.isArray(JSON.parse(body)) ? JSON.parse(body) : []).map((r) => ({
      name: String(r.name ?? ""),
      sha: String(r.commit?.sha ?? ""),
    }));
  const refs = {
    default_branch: String(meta.default_branch ?? ""),
    branches: pick(branchesRes.body),
    tags: pick(tagsRes.body),
  };
  return {
    text: formatRefs(refs),
    json: { api_version: API_VERSION, verb: "refs", repo: route.ownerRepo, ...refs },
  };
}

/**
 * `GET /commits/{owner}/{repo}?path=&n=&ref=`: recent history, optionally
 * for one path — one REST call.
 *
 * @param {import('./routes.js').RepoRoute} route
 * @param {Auth} auth
 * @returns {Promise<VerbResult>}
 */
async function listCommits(route, auth) {
  const params = new URLSearchParams();
  params.set("per_page", String(route.count ?? 10));
  if (route.branch) params.set("sha", route.branch);
  if (route.path) params.set("path", route.path);
  const path = `/repos/${route.ownerRepo}/commits?${params}`;
  const res = await githubGet(path, auth.token);
  if (res.status !== 200) {
    throw githubFailure(res, {
      notFound: `repository or ref not found for '${route.ownerRepo}'`,
      label: "commits",
      tokenSource: auth.source,
    });
  }
  const items = Array.isArray(JSON.parse(res.body)) ? JSON.parse(res.body) : [];
  const commits = items.map((c) => ({
    sha: String(c.sha ?? ""),
    author: String(c.commit?.author?.name ?? c.author?.login ?? ""),
    date: String(c.commit?.author?.date ?? c.commit?.committer?.date ?? ""),
    message: String(c.commit?.message ?? ""),
  }));
  return {
    text: formatCommits(commits),
    json: {
      api_version: API_VERSION,
      verb: "commits",
      repo: route.ownerRepo,
      ref: route.branch,
      path: route.path || null,
      commits,
    },
  };
}

/**
 * Compose the GitHub repository-search query like the CLI does:
 * `-p` adds `NAME in:name`, `--lang` adds `language:X`, `-q` passes through.
 * @param {{ query: string | null, pattern: string | null, lang: string | null }} route
 */
export function buildSearchQuery(route) {
  const parts = [];
  if (route.pattern) parts.push(`${route.pattern} in:name`);
  if (route.query) parts.push(route.query);
  if (route.lang) parts.push(`language:${route.lang}`);
  return parts.join(" ").trim();
}

/**
 * `GET /search?q=`: GitHub repository search (not code search).
 * @param {Extract<ReturnType<typeof parseRoute>, { kind: 'search' }>} route
 * @param {Auth} auth
 * @returns {Promise<VerbResult>}
 */
async function searchRepositories(route, auth) {
  const q = buildSearchQuery(route);
  const params = new URLSearchParams();
  params.set("q", q);
  if (route.sort !== "best") {
    params.set("sort", route.sort);
    params.set("order", "desc");
  }
  params.set("per_page", String(route.limit));
  const res = await githubGet(`/search/repositories?${params}`, auth.token);
  if (res.status !== 200) {
    throw githubFailure(res, {
      notFound: "search endpoint not found",
      label: "search",
      tokenSource: auth.source,
    });
  }
  const body = JSON.parse(res.body);
  const items = (Array.isArray(body.items) ? body.items : []).slice(0, route.limit).map((r) => ({
    full_name: String(r.full_name ?? r.name ?? ""),
    description: r.description ?? null,
    language: r.language ?? null,
    stars: Number(r.stargazers_count) || 0,
    forks: Number(r.forks_count) || 0,
    html_url: r.html_url ?? null,
    default_branch: r.default_branch ?? null,
    pushed_at: r.pushed_at ?? null,
    archived: Boolean(r.archived),
    topics: Array.isArray(r.topics) ? r.topics : [],
  }));
  return {
    text: formatSearch(items),
    json: {
      api_version: API_VERSION,
      verb: "search",
      query: q,
      sort: route.sort,
      total_count: Number(body.total_count) || items.length,
      items,
    },
  };
}

/**
 * Text or JSON 200 response with provenance headers.
 * @param {Request} request
 * @param {'text' | 'json'} format
 * @param {VerbResult} result
 * @param {Record<string, string>} headers
 * @param {Auth} auth
 */
function respond(request, format, result, headers, auth) {
  const cacheControl =
    auth.source === "caller" ? "private, no-store" : "public, max-age=60, stale-while-revalidate=600";
  if (format === "json") {
    return bodyResponse(request, `${JSON.stringify(result.json)}\n`, {
      contentType: "application/json; charset=utf-8",
      headers: { ...headers, "cache-control": cacheControl },
    });
  }
  // Formatters join lines with "\n" (the CLI println!s each line), so a
  // non-empty body always gets exactly one trailing newline appended.
  const body = result.text === "" ? "" : `${result.text}\n`;
  return bodyResponse(request, body, { headers: { ...headers, "cache-control": cacheControl } });
}

/**
 * 200 response for a successful body; HEAD gets the same headers, no body.
 * @param {Request} request
 * @param {string} body
 * @param {{ contentType?: string, headers?: Record<string, string> }} [opts]
 */
function bodyResponse(request, body, opts = {}) {
  const headers = responseHeaders(body, opts.contentType, opts.headers);
  if (request.method === "HEAD") return new Response(null, { status: 200, headers });
  return new Response(body, { status: 200, headers });
}

/**
 * @param {string} body
 * @param {number} status
 * @param {Record<string, string>} [extra]
 * @param {string} [contentType]
 */
function textResponse(body, status, extra = {}, contentType) {
  return new Response(body, { status, headers: responseHeaders(body, contentType, extra) });
}

/**
 * @param {string} body
 * @param {string} [contentType]
 * @param {Record<string, string>} [extra]
 */
function responseHeaders(body, contentType = "text/plain; charset=utf-8", extra = {}) {
  return {
    "content-type": contentType,
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
    "access-control-allow-origin": "*",
    "access-control-expose-headers":
      "x-wit-repo, x-wit-ref, x-wit-commit, x-wit-cache, x-wit-auth, x-wit-api-version, retry-after",
    "content-length": String(new TextEncoder().encode(body).length),
    ...extra,
  };
}

export { createHostCache, extractToken, parseRoute, scrubSecrets };
