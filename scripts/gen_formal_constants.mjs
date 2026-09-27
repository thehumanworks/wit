#!/usr/bin/env node
// Extract the constants the Lean proofs rely on from their sources of truth
// and print formal/Wit/Generated/Constants.lean. scripts/check_formal.sh
// compares the output with the committed file; `--write` updates it.
//
// Every extraction fails loudly when its source pattern is missing, so a
// refactor that moves a constant breaks the check instead of silently
// freezing an old value.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const out = join(root, "formal/Wit/Generated/Constants.lean");
const read = (rel) => readFileSync(join(root, rel), "utf8");

function fail(msg) {
  console.error(`gen_formal_constants: ${msg}`);
  process.exit(1);
}

function match(rel, text, re, what) {
  const m = text.match(re);
  if (!m) fail(`${rel}: cannot find ${what} (pattern ${re})`);
  return m;
}

function nat(value, what) {
  if (!Number.isSafeInteger(value) || value < 0) fail(`${what} must be a natural number, got ${value}`);
  return value;
}

/** Evaluate `a * b * c` integer products as written in Rust constants. */
function product(expr, what) {
  const parts = expr.split("*").map((p) => p.trim().replaceAll("_", ""));
  if (!parts.every((p) => /^\d+$/.test(p))) fail(`${what}: unsupported expression '${expr}'`);
  return nat(parts.reduce((acc, p) => acc * Number(p), 1), what);
}

/** Expand a regex character class body such as `A-Za-z0-9._-` into its characters. */
function expandClass(body, what) {
  const chars = [];
  for (let i = 0; i < body.length; i++) {
    const c = body[i];
    if (c === "\\") fail(`${what}: escapes in character classes are not supported`);
    if (body[i + 1] === "-" && i + 2 < body.length) {
      const lo = c.charCodeAt(0);
      const hi = body.charCodeAt(i + 2);
      if (hi < lo) fail(`${what}: bad range ${c}-${body[i + 2]}`);
      for (let code = lo; code <= hi; code++) chars.push(code);
      i += 2;
    } else {
      chars.push(c.charCodeAt(0));
    }
  }
  return [...new Set(chars)].sort((a, b) => a - b);
}

/** Minimal TOML reader for the subset wrangler.toml uses. */
function parseToml(text) {
  const doc = {};
  let table = doc;
  for (const raw of text.split("\n")) {
    const line = raw.replace(/\s+#.*$/, "").replace(/^#.*$/, "").trim();
    if (!line) continue;
    let m;
    if ((m = line.match(/^\[\[([\w.]+)\]\]$/))) {
      const path = m[1].split(".");
      let parent = doc;
      for (const key of path.slice(0, -1)) parent = parent[key] ??= {};
      const last = path.at(-1);
      (parent[last] ??= []).push((table = {}));
    } else if ((m = line.match(/^\[([\w.]+)\]$/))) {
      table = doc;
      for (const key of m[1].split(".")) table = table[key] ??= {};
    } else if ((m = line.match(/^([\w-]+)\s*=\s*(.+)$/))) {
      table[m[1]] = tomlValue(m[2]);
    } else {
      fail(`wrangler.toml: cannot parse line '${raw}'`);
    }
  }
  return doc;
}

function tomlValue(v) {
  v = v.trim();
  if (v.startsWith('"')) return JSON.parse(v);
  if (v === "true" || v === "false") return v === "true";
  if (/^-?[\d_]+$/.test(v)) return Number(v.replaceAll("_", ""));
  if (v.startsWith("[")) return JSON.parse(v);
  if (v.startsWith("{")) {
    const obj = {};
    for (const pair of v.slice(1, -1).split(",")) {
      const [k, val] = pair.split("=").map((s) => s.trim());
      obj[k] = tomlValue(val);
    }
    return obj;
  }
  fail(`wrangler.toml: unsupported value '${v}'`);
}

// --- services/wit-cache/src/config.js (imported, so these are the real values) ---
const configUrl = pathToFileURL(join(root, "services/wit-cache/src/config.js")).href;
const { DEFAULTS, NEGATIVE_TTL, REPO_SCOPED, PENDING_TTL_SECONDS, fillReserveBytes, limitsFromEnv } = await import(
  configUrl
);

const limitFields = {
  MAX_PACK_BYTES: "maxPackBytes",
  STORAGE_CAP_BYTES: "storageCapBytes",
  DAILY_FILL_LIMIT: "dailyFillLimit",
  DAILY_FILL_BYTES: "dailyFillBytes",
  MAX_INFLIGHT_FILLS: "maxInflightFills",
  PART_BYTES: "partBytes",
  FILL_TIMEOUT_MS: "fillTimeoutMs",
  RETENTION_DAYS: "retentionDays",
};
for (const name of Object.keys(DEFAULTS)) {
  if (!(name in limitFields)) fail(`config.js DEFAULTS.${name} is not modeled in formal/Wit/Basic.lean (Limits)`);
}
for (const name of Object.keys(limitFields)) {
  if (!(name in DEFAULTS)) fail(`config.js DEFAULTS.${name} disappeared; update the formal model`);
}

const reasonCtor = (reason) => reason.replace(/_([a-z])/g, (_, c) => c.toUpperCase());

// --- services/wit-cache/src/coordinator.js ---
const coordinatorRel = "services/wit-cache/src/coordinator.js";
const coordinator = read(coordinatorRel);
const rateLimitedCap = Number(
  match(coordinatorRel, coordinator, /Math\.min\(outcome\.retryAfterSeconds, (\d+)\)/, "the Retry-After cap")[1],
);
const ledgerGraceDays = Number(
  match(
    coordinatorRel,
    coordinator,
    /filled_at <= \?`, now - \(this\.limits\.RETENTION_DAYS \+ (\d+)\) \* 86400\)/,
    "the ledger prune grace",
  )[1],
);
match(
  coordinatorRel,
  coordinator,
  /until = MAX\(until, excluded\.until\)/,
  "negative upserts that keep the later expiry (Wit.Coordinator models MAX)",
);
if (/DELETE FROM packs WHERE key = \?`, key\);\s*this\.rows\(`INSERT INTO pending/.test(coordinator)) {
  fail(`${coordinatorRel}: requestFill must not drop the ledger row of the key it queues (Wit.Coordinator)`);
}
match(coordinatorRel, coordinator, /if \(this\.isBlocked\(repo, now\)\)/, "the takedown check in complete");
// Byte reservations (Wit.Coordinator.requestFill / settle / complete).
const requestFillBody = match(coordinatorRel, coordinator, /\n  requestFill\(job\) \{[\s\S]*?\n  \}\n/, "requestFill")[0];
match(coordinatorRel, requestFillBody, /const reserve = fillReserveBytes\(this\.limits\);/, "the reservation size");
match(
  coordinatorRel,
  requestFillBody,
  /used\.bytes \+ reserve > this\.limits\.DAILY_FILL_BYTES\) \{\s*return \{ status: "skipped", reason: "daily_budget" \}/,
  "the byte gate that admits a fill only when its whole reservation fits",
);
match(coordinatorRel, requestFillBody, /this\.bumpDaily\(day, 1, reserve\);/, "the reservation charge");
const settleBody = match(coordinatorRel, coordinator, /\n  settle\(key, id, charge\) \{[\s\S]*?\n  \}\n/, "settle")[0];
match(coordinatorRel, settleBody, /if \(!row \|\| \(id !== undefined && row\.id !== id\)\) return false;/, "the reservation id check");
match(coordinatorRel, settleBody, /Math\.min\(charge, row\.reserved\) : row\.reserved;/, "the settlement cap");
const byteCharges = [...coordinator.matchAll(/this\.bumpDaily\((.*)\);/g)].filter((m) => !/, 0\)?$/.test(m[1]));
if (/UPDATE daily SET bytes = [^`]*\+/.test(coordinator) || byteCharges.length !== 1 || byteCharges[0][1] !== "day, 1, reserve") {
  fail(`${coordinatorRel}: bytes may only be charged by requestFill's reservation (Wit.Coordinator)`);
}
match(
  coordinatorRel,
  coordinator,
  /async complete\(outcome\) \{[^}]*?\n    this\.settle\(key, outcome\.reservation, outcome\.bytesRead \?\? \(outcome\.ok \? outcome\.bytes : undefined\)\);/,
  "complete settling the reservation before anything else",
);
const takedownBody = match(coordinatorRel, coordinator, /async takedown\(target\) \{[\s\S]*?\n  \}\n/, "takedown")[0];
const blockAt = takedownBody.indexOf("INSERT INTO negatives");
const firstAwait = takedownBody.indexOf("await ");
if (blockAt < 0 || (firstAwait >= 0 && firstAwait < blockAt)) {
  fail(`${coordinatorRel}: takedown must record the block before its first await (Wit.Coordinator models it as one step)`);
}

// --- services/wit-cache/src/index.js: a redelivered fill job never fetches (Wit.Coordinator.fetch) ---
const indexRel = "services/wit-cache/src/index.js";
const indexSrc = read(indexRel);
match(
  indexRel,
  indexSrc,
  /const outcome = \(deps\.attempts \?\? 1\) > 1 \? await recheckOutcome\(env, job\) : await fillOutcome\(env, job, deps\);/,
  "runFillJob dispatching redeliveries to recheckOutcome",
);
match(indexRel, indexSrc, /await runFillJob\(env, job, \{ attempts: message\.attempts \}\);/, "the queue passing message.attempts");
const recheckBody = match(indexRel, indexSrc, /async function recheckOutcome\(env, job\) \{[\s\S]*?\n\}\n/, "recheckOutcome")[0];
if (/fillPack|fetch|bytesRead/.test(recheckBody)) {
  fail(`${indexRel}: recheckOutcome must not fetch or report bytes read (one reservation per job)`);
}

// --- services/wit-cache/wrangler.toml ---
const wrangler = parseToml(read("services/wit-cache/wrangler.toml"));
const vars = wrangler.vars ?? {};
for (const name of Object.keys(vars)) {
  if (!(name in limitFields)) fail(`wrangler.toml [vars].${name} is not a modeled limit`);
}
const consumers = wrangler.queues?.consumers ?? [];
if (consumers.length !== 1) fail("wrangler.toml must declare exactly one queue consumer");
const consumer = consumers[0];
// Budget.runsPerMessage = 1 + max_retries; a dead-letter consumer would add runs.
if ("dead_letter_queue" in consumer) fail("wrangler.toml: a dead_letter_queue would re-run fills outside the Class A bound");
const limiter = (name) => {
  const rl = (wrangler.ratelimits ?? []).find((r) => r.name === name);
  if (!rl?.simple) fail(`wrangler.toml: rate limiter ${name} is missing`);
  return rl.simple;
};
const readLimiter = limiter("READ_LIMITER");
const fillLimiter = limiter("FILL_LIMITER");
const reserveDefaults = nat(fillReserveBytes(DEFAULTS), "fillReserveBytes(DEFAULTS)");
const reserveDeployed = nat(fillReserveBytes(limitsFromEnv(vars)), "fillReserveBytes(limitsFromEnv(vars))");
const cpuMs = wrangler.limits?.cpu_ms;
if (cpuMs == null) fail("wrangler.toml [limits].cpu_ms is missing");
if (wrangler.observability?.enabled == null) fail("wrangler.toml [observability].enabled is missing");

// --- services/wit-cache/src/pktline.js ---
const pktRel = "services/wit-cache/src/pktline.js";
const maxPktLen = Number(match(pktRel, read(pktRel), /export const MAX_PKT_LEN = (\d+);/, "MAX_PKT_LEN")[1]);

// --- services/wit-cache/src/keys.js ---
const keysRel = "services/wit-cache/src/keys.js";
const keys = read(keysRel);
const componentChars = expandClass(
  match(keysRel, keys, /const COMPONENT = \/\^\[([^\]]+)\]\{1,100\}\$\/;/, "COMPONENT")[1],
  "COMPONENT",
);
const packTemplate = match(
  keysRel,
  keys,
  /return `([^`$]*)\$\{repoId\(owner, repo\)\}\/\$\{commit\}([^`$]*)`;/,
  "the packKey template",
);
const repoTemplate = match(
  keysRel,
  keys,
  /return `([^`$]*)\$\{repoId\(owner, repo\)\}\/`;/,
  "the repoPrefix template",
);
if (packTemplate[1] !== repoTemplate[1]) fail(`${keysRel}: packKey and repoPrefix must share a prefix`);
match(keysRel, keys, /return `\$\{owner\.toLowerCase\(\)\}\/\$\{repo\.toLowerCase\(\)\}`;/, "repoId");

// --- .github/workflows/cache-worker-deploy.yml (R2 lifecycle rules) ---
const deployRel = ".github/workflows/cache-worker-deploy.yml";
const deploy = read(deployRel);
const expire = match(
  deployRel,
  deploy,
  /lifecycle add "\$BUCKET" expire-30d (\S+) --expire-days (\d+)/,
  "the expire lifecycle rule",
);
const abortDays = Number(
  match(deployRel, deploy, /abort-multipart-1d "" --abort-multipart-days (\d+)/, "the abort-multipart rule")[1],
);

// --- Worker request handling: which caller headers are read, which upstream headers are sent ---
const workerSources = ["index.js", "fill.js", "coordinator.js", "keys.js", "store.js", "upload-pack.js", "pktline.js"];
const callerHeaders = new Set();
const index = read("services/wit-cache/src/index.js");
for (const m of index.matchAll(/request\.headers\.get\("([^"]+)"\)/g)) callerHeaders.add(m[1].toLowerCase());
for (const file of workerSources) {
  const src = read(`services/wit-cache/src/${file}`);
  const authImport = src.match(/import \{([^}]*)\} from "\.\/auth\.js"/);
  if (authImport && /\b(extractToken|githubAuthHeader|withActiveSecrets)\b/.test(authImport[1])) {
    callerHeaders.add("authorization");
  }
  if (/\.headers\.get\(\s*["']authorization["']/i.test(src)) callerHeaders.add("authorization");
}
const methods = new Set([...index.matchAll(/method [!=]== "([A-Z]+)"/g)].map((m) => m[1]));

const uploadRel = "services/wit-cache/src/upload-pack.js";
const upload = read(uploadRel);
const postHeaders = match(uploadRel, upload, /headers: \{([^}]*)\}/, "the upstream request headers")[1];
const upstreamHeaders = [...postHeaders.matchAll(/(?:"([^"]+)"|(\w+)):/g)].map((m) => (m[1] ?? m[2]).toLowerCase());
const upstreamRedirect = match(uploadRel, upload, /redirect: "(\w+)"/, "the upstream redirect mode")[1];

// --- crates/wit/src/gitops/cloud.rs ---
const cloudRel = "crates/wit/src/gitops/cloud.rs";
const cloudAll = read(cloudRel);
const cloud = cloudAll.split("#[cfg(test)]\npub(crate) mod tests")[0];
const clientMaxBytes = product(
  match(cloudRel, cloud, /const DEFAULT_MAX_BYTES: u64 = ([\d_ *]+);/, "DEFAULT_MAX_BYTES")[1],
  "DEFAULT_MAX_BYTES",
);
const secs = (name) =>
  nat(
    Number(match(cloudRel, cloud, new RegExp(`const ${name}: Duration = Duration::from_secs\\((\\d+)\\);`), name)[1]) *
      1000,
    name,
  );
const clientTimeoutMs = secs("DEFAULT_TIMEOUT");
const clientConnectMs = secs("CONNECT_TIMEOUT");
const disabled = match(cloudRel, cloud, /fn is_disabled[\s\S]*?\{[\s\S]*?value\.to_ascii_lowercase\(\)\.as_str\(\),\s*([^=]+?)\s*\)/, "is_disabled")[1]
  .split("|")
  .map((s) => JSON.parse(s.trim()));
const clientHeaders = [];
const fetchFn = match(cloudRel, cloud, /pub fn fetch_pack_into\([\s\S]*?\n\}\n/, "fetch_pack_into")[0];
if (/\.user_agent\(/.test(fetchFn)) clientHeaders.push("user-agent");
for (const m of fetchFn.matchAll(/\.header\(\s*"([^"]+)"/g)) clientHeaders.push(m[1].toLowerCase());
if (/\.header\(\s*(reqwest::header::)?AUTHORIZATION|bearer_auth|basic_auth|default_headers/.test(cloud)) {
  clientHeaders.push("authorization");
}
const clientRedirects = /redirect\(reqwest::redirect::Policy::none\(\)\)/.test(fetchFn) ? "none" : "follow";
const clientMethods = [
  ...new Set([...cloud.matchAll(/\bclient\s*\.\s*(get|post|put|patch|delete|head|request)\(/g)].map((m) => m[1].toUpperCase())),
].sort();
if (!clientMethods.length) fail(`${cloudRel}: found no HTTP request in the cloud client`);

// --- crates/wit/src/gitops/ops.rs (branch directory encoding, ADR 0002) ---
const opsRel = "crates/wit/src/gitops/ops.rs";
const ops = read(opsRel);
const encodeFn = match(opsRel, ops, /fn encode_branch_for_path\(branch: &str\) -> String \{[\s\S]*?\n\}\n/, "encode_branch_for_path")[0];
const branchPrefix = match(opsRel, encodeFn, /encoded\.push_str\("([^"]*)"\);/, "the branch prefix")[1];
const safeArm = match(opsRel, encodeFn, /match byte \{\s*([^\n]+?)\s*=> encoded\.push\(byte as char\)/, "the safe-byte arm")[1];
const branchSafe = [];
for (const alt of safeArm.split("|").map((s) => s.trim())) {
  const range = alt.match(/^b'(.)'\.\.=b'(.)'$/);
  const single = alt.match(/^b'(.)'$/);
  if (range) for (let c = range[1].charCodeAt(0); c <= range[2].charCodeAt(0); c++) branchSafe.push(c);
  else if (single) branchSafe.push(single[1].charCodeAt(0));
  else fail(`${opsRel}: unsupported pattern '${alt}' in encode_branch_for_path`);
}
match(opsRel, encodeFn, /_ => encoded\.push_str\(&format!\("%\{byte:02X\}"\)\)/, "the %XX escape");

// --- Cache source resolution (Wit.CacheSource): backend choice, cloud defaults, disk read order ---
const strConst = (rel, text, name) =>
  match(rel, text, new RegExp(`pub const ${name}: &str = "([^"\\\\]*)";`), name)[1];
const body = (rel, text, signature, what) =>
  match(rel, text, new RegExp(`\\n( *)(?:pub(?:\\(crate\\))? )?(?:async )?fn ${signature}[\\s\\S]*?\\n\\1\\}\\n`), what)[0];
const quotedAlts = (alts, what) =>
  alts.split("|").map((s) => {
    const m = s.trim().match(/^"([^"\\]*)"$/);
    if (!m) fail(`${what}: unsupported pattern '${s.trim()}'`);
    return m[1];
  });

const snapRel = "crates/wit/src/snapshot/mod.rs";
const snap = read(snapRel);
const fromEnvOrFlag = body(snapRel, snap, "from_env_or_flag\\(", "CliSnapshotBackend::from_env_or_flag");
const flagFirst = match(
  snapRel,
  fromEnvOrFlag,
  /if let Some\(value\) = flag \{\s*return Self::parse\(value\);\s*\}/,
  "--backend taking precedence over the environment",
).index;
const backendEnvMatch = match(snapRel, fromEnvOrFlag, /std::env::var\("(WIT_[A-Z_]+)"\)\s*&& !value\.trim\(\)\.is_empty\(\)\s*\{\s*return Self::parse\(value\.trim\(\)\);\s*\}/, "the blank-aware environment read");
if (backendEnvMatch.index < flagFirst) fail(`${snapRel}: from_env_or_flag must read --backend before the environment`);
const backendEnvVar = backendEnvMatch[1];
const backendCtor = (rust) => ({ Disk: ".disk", Memory: ".memory" })[rust] ?? fail(`unknown backend ${rust}`);
const backendDefault = backendCtor(
  match(snapRel, fromEnvOrFlag, /\n\s*Ok\(Self::(\w+)\)\s*\n\s*\}\n$/, "the default backend")[1],
);
const parseBackendFn = body(snapRel, snap, "parse\\(value: &str\\)", "CliSnapshotBackend::parse");
match(snapRel, parseBackendFn, /match value\.trim\(\)\.to_ascii_lowercase\(\)\.as_str\(\) \{/, "trimmed, ASCII-lowercased backend values");
const aliases = (ctor) =>
  quotedAlts(
    match(snapRel, parseBackendFn, new RegExp(`\\n\\s*([^\\n]+?)\\s*=> Ok\\(Self::${ctor}\\)`), `the ${ctor} aliases`)[1],
    `${ctor} aliases`,
  );
const diskAliases = aliases("Disk");
const memoryAliases = aliases("Memory");

const cloudUrlEnv = strConst(cloudRel, cloud, "WIT_CACHE_URL_ENV");
const cloudTimeoutEnv = strConst(cloudRel, cloud, "WIT_CACHE_TIMEOUT_MS_ENV");
const cloudMaxBytesEnv = strConst(cloudRel, cloud, "WIT_CACHE_MAX_BYTES_ENV");
const hostedCacheUrl = strConst(cloudRel, cloud, "HOSTED_CACHE_URL");
const builtIn = match(
  cloudRel,
  cloud,
  /fn built_in_default\(\) -> Option<&'static str> \{\s*match option_env!\("(WIT_[A-Z_]+)"\) \{\s*Some\(url\) => Some\(url\),\s*None if cfg!\(debug_assertions\) => (None|Some\(HOSTED_CACHE_URL\)),\s*None => (None|Some\(HOSTED_CACHE_URL\)),\s*\}\s*\}/,
  "built_in_default (baked value, then debug, then release)",
);
const bakedDefaultEnv = builtIn[1];
const profileDefault = (arm) => (arm === "None" ? "none" : `some ${JSON.stringify(hostedCacheUrl)}`);
const debugDefault = profileDefault(builtIn[2]);
const releaseDefault = profileDefault(builtIn[3]);
match(cloudRel, cloud, /let url = std::env::var\(WIT_CACHE_URL_ENV\)\.ok\(\);[\s\S]*?built_in_default\(\),/, "from_env passing WIT_CACHE_URL and the built-in default");
match(
  cloudRel,
  cloud,
  /let raw = url\.or\(default_url\)\?\.trim\(\);\s*if is_disabled\(raw\) \{\s*return None;\s*\}\s*let base_url = parse_base_url\(raw\)\?;/,
  "from_values: WIT_CACHE_URL over the default, then the disable check, then URL validation",
);

const cacheDirEnv = strConst(opsRel, ops, "WIT_CACHE_DIR_ENV");
const cacheSubdir = strConst(opsRel, ops, "WIT_CACHE_SUBDIR");
match(
  opsRel,
  ops,
  /pub fn wit_cache_dir\(\) -> PathBuf \{\s*if let Some\(path\) = std::env::var_os\(WIT_CACHE_DIR_ENV\)\.filter\(\|value\| !value\.is_empty\(\)\) \{\s*return PathBuf::from\(path\);\s*\}\s*std::env::temp_dir\(\)\.join\(WIT_CACHE_SUBDIR\)\s*\}/,
  "wit_cache_dir (non-empty WIT_CACHE_DIR, else the temp directory)",
);
// Disk reads: a usable local entry returns before any fill; a fill tries the cloud pack first
// and clones from GitHub only when it fails. Both the CLI and the operation (MCP) paths.
const markerOrder = (rel, text, markers, what) => {
  const found = Object.entries(markers)
    .map(([source, needle]) => [source, text.indexOf(needle)])
    .filter(([, at]) => at >= 0)
    .sort((a, b) => a[1] - b[1])
    .map(([source]) => source);
  if (!found.length) fail(`${rel}: ${what} reaches no fill source`);
  return found;
};
const fillOrders = [
  ["recache_repo(", { cloud: "cloud::fill_from_cloud(", github: "clone_from_github(" }],
  ["refresh_repo_with_context(", { cloud: "cloud::fill_from_cloud(", github: "clone_with_git_cli_in_context(" }],
].map(([fn, markers]) => markerOrder(opsRel, body(opsRel, ops, fn.replace("(", "\\("), fn), markers, fn).join(","));
if (new Set(fillOrders).size !== 1) fail(`${opsRel}: recache_repo and refresh_repo_with_context try fill sources in different orders`);
match(
  opsRel,
  body(opsRel, ops, "recache_repo\\(", "recache_repo"),
  /if cloud::fill_from_cloud\([\s\S]*?\) \{[\s\S]*?return Ok\(\(repo, FillSource::Cloud\)\);\s*\}\s*clone_from_github\(/,
  "recache_repo returning the verified cloud fill before cloning",
);
match(
  opsRel,
  body(opsRel, ops, "refresh_repo_with_context\\(", "refresh_repo_with_context"),
  /let source = if cloud::fill_from_cloud\([\s\S]*?\) \{\s*FillSource::Cloud\s*\} else \{\s*clone_with_git_cli_in_context\(/,
  "refresh_repo_with_context cloning only when the cloud fill fails",
);
for (const [fn, fill] of [
  ["cache_github_repo_target(", "recache_repo("],
  ["cache_github_repo_target_with_context(", "refresh_repo_with_context("],
]) {
  const src = body(opsRel, ops, fn.replace("(", "\\("), fn);
  const warm = src.search(/if cache_path\.exists\(\)\s*&& !mode\.is_force_invalidate\(\)/);
  const served = src.indexOf("return Ok(repo);");
  const filled = src.indexOf(fill);
  if (warm < 0 || served < warm || filled < served) {
    fail(`${opsRel}: ${fn} must serve a usable local cache before calling ${fill}`);
  }
}
const sourceCtor = { localCache: ".localCache", cloud: ".cloud", github: ".github" };
const diskReadOrder = ["localCache", ...fillOrders[0].split(",")];

// The memory backend (`--backend memory`) never reaches the disk cache or the cloud client.
const stripComments = (src) => src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n]*/g, "");
const cliRel = "crates/wit/src/cli.rs";
const cli = read(cliRel);
const memorySources = [
  read("crates/wit/src/snapshot/memory_ops.rs"),
  read("crates/wit-snapshot/src/memory.rs"),
  body(cliRel, cli, "open_memory_snapshot\\(", "open_memory_snapshot"),
].map(stripComments);
const memoryUsesDisk = memorySources.some((s) => /cache_github_repo|wit_cache_dir|WIT_CACHE_DIR|std::fs::|File::create|create_dir/.test(s));
const memoryUsesCloud = memorySources.some((s) => /fill_from_cloud|cloud::|WIT_CACHE_URL|CloudCacheConfig/.test(s));

// --- crates/wit/src/cli.rs: the claims `wit --help` makes about the above ---
function rustLiteral(rel, text, name) {
  const raw = match(rel, text, new RegExp(`const ${name}: &str = "((?:[^"\\\\]|\\\\[\\s\\S])*)";`), name)[1];
  return raw.replace(/\\(\n\s*|.)/g, (_, esc) => {
    if (esc.startsWith("\n")) return "";
    const plain = { n: "\n", t: "\t", '"': '"', "\\": "\\", "'": "'" }[esc];
    if (plain === undefined) fail(`${rel}: unsupported escape \\${esc} in ${name}`);
    return plain;
  });
}
const rootHelp = rustLiteral(cliRel, cli, "ROOT_AFTER_HELP");
const claim = (re, what, text = rootHelp, name = "ROOT_AFTER_HELP") =>
  match(cliRel, text, re, `the ${name} claim about ${what}`);
const backendWord = (w) => ({ disk: ".disk", memory: ".memory" })[w];
const helpDefaultBackend = backendWord(
  claim(/Snapshot backends: repo commands \([^)]+\) default to the (disk|memory) backend\./, "the default backend")[1],
);
const helpBackendEnvVar = claim(/--backend wins over (WIT_[A-Z_]+)\./, "--backend precedence")[1];
const helpMemoryDir = claim(/The memory backend [^.]*with no (WIT_[A-Z_]+) writes and no shared cloud cache/, "the memory backend")[1];
const helpCacheDir = claim(
  /Disk cache: shallow bare repos in (\S+) under the system temp directory \(override with (WIT_[A-Z_]+)\)\./,
  "the cache directory",
);
const sourceWords = { "local cache": "localCache", "shared cloud pack cache": "cloud", "GitHub clone": "github" };
const helpDiskReadOrder = claim(/Disk read order: ([^.]+)\./, "the disk read order")[1]
  .split(/, then /)
  .map((w) => sourceWords[w] ?? fail(`${cliRel}: ROOT_AFTER_HELP names unknown source '${w}' in the disk read order`));
claim(/falls back to a depth-1 GitHub clone when the cloud cache is off, misses, or fails verification\./, "the GitHub fallback");
const helpCloudUrlEnv = claim(/Shared cloud cache: (WIT_[A-Z_]+) sets its base URL[^;]*; a valid URL turns it on in any build\./, "the cloud URL")[1];
const helpDefaults = claim(
  /Release builds default (WIT_[A-Z_]+) to (\S+?); debug builds \([^)]*\) default to (off|\S+?)\./,
  "the release and debug defaults",
);
const helpBakedEnv = claim(/bake another default with (WIT_[A-Z_]+) at build time\./, "the build-time default")[1];
const helpDisable = claim(/Disable it with (WIT_[A-Z_]+)=(\S+) \(also: ([^;)]+); any case\)\./, "the disable values");
const helpDisableValues = [helpDisable[2], ...helpDisable[3].split(",").map((s) => s.trim())].map((v) =>
  v === "empty" ? "" : v,
);
const helpTimeout = claim(/(WIT_[A-Z_]+) \(default (\d+)\) bounds the pack download/, "the download timeout");
const helpMaxBytes = claim(/(WIT_[A-Z_]+) \(default (\d+)\) caps its size/, "the pack size cap");
claim(/Requests to the cloud cache carry no credentials\./, "credentials");
const flagHelps = ["BACKEND_HELP", "BRANCHES_BACKEND_HELP"].map((name) => {
  const m = claim(/^Snapshot backend: (disk|memory) \(default;[^)]*\) or \w+ \([^)]*\)\. Overrides (WIT_[A-Z_]+)$/, "--backend", rustLiteral(cliRel, cli, name), name);
  return { name, backend: backendWord(m[1]), env: m[2] };
});

// --- Emit Lean ---
const str = (s) => JSON.stringify(s);
const list = (xs, f = String) => `[${xs.map(f).join(", ")}]`;
const optNat = (name) => (name in vars ? `some ${nat(Number(vars[name]), name)}` : "none");
const lines = [];
const emit = (s = "") => lines.push(s);

emit("import Wit.Basic");
emit("");
emit("/-!");
emit("# Constants extracted from the repository (generated)");
emit("");
emit("Generated by `scripts/gen_formal_constants.mjs`; do not edit. `scripts/check_formal.sh`");
emit("fails when this file no longer matches the sources, so every proof that uses these");
emit("values is about the code and config as committed.");
emit("-/");
emit("");
emit("namespace Wit.Src");
emit("");
emit("/-- `DEFAULTS` in `services/wit-cache/src/config.js`. -/");
emit("def workerDefaults : Limits where");
for (const [js, lean] of Object.entries(limitFields)) emit(`  ${lean} := ${nat(DEFAULTS[js], js)}`);
emit("");
emit("/-- `[vars]` in `services/wit-cache/wrangler.toml`. -/");
emit("def wranglerVars : LimitVars where");
for (const [js, lean] of Object.entries(limitFields)) emit(`  ${lean} := ${optNat(js)}`);
emit("");
emit("/-- `NEGATIVE_TTL` (seconds) in `services/wit-cache/src/config.js`. -/");
emit("def negativeTtl : Reason → Nat");
for (const [reason, ttl] of Object.entries(NEGATIVE_TTL)) emit(`  | .${reasonCtor(reason)} => ${nat(ttl, reason)}`);
emit("");
emit("/-- `REPO_SCOPED` in `services/wit-cache/src/config.js`. -/");
emit("def repoScoped : Reason → Bool");
for (const reason of Object.keys(NEGATIVE_TTL)) emit(`  | .${reasonCtor(reason)} => ${REPO_SCOPED.has(reason)}`);
for (const reason of REPO_SCOPED) if (!(reason in NEGATIVE_TTL)) fail(`REPO_SCOPED has unknown reason ${reason}`);
emit("");
emit("/-- `PENDING_TTL_SECONDS` in `services/wit-cache/src/config.js`. -/");
emit(`def pendingTtlSeconds : Nat := ${nat(PENDING_TTL_SECONDS, "PENDING_TTL_SECONDS")}`);
emit("/-- `fillReserveBytes` in `services/wit-cache/src/config.js`, at `DEFAULTS` and at the deployed limits. -/");
emit(`def fillReserveDefaults : Nat := ${reserveDefaults}`);
emit(`def fillReserveDeployed : Nat := ${reserveDeployed}`);
emit("/-- Retry-After cap in `FillCoordinator.complete`. -/");
emit(`def rateLimitedTtlCap : Nat := ${nat(rateLimitedCap, "rate limit cap")}`);
emit("/-- Days a ledger row outlives `RETENTION_DAYS` in `FillCoordinator.prune`. -/");
emit(`def ledgerGraceDays : Nat := ${nat(ledgerGraceDays, "ledger grace")}`);
emit("");
emit("/-- `services/wit-cache/wrangler.toml`: `[limits]`, queue consumer, rate limiters. -/");
emit(`def cpuMsPerInvocation : Nat := ${nat(cpuMs, "cpu_ms")}`);
emit(`def queueMaxConcurrency : Nat := ${nat(consumer.max_concurrency, "max_concurrency")}`);
emit(`def queueMaxRetries : Nat := ${nat(consumer.max_retries, "max_retries")}`);
emit(`def queueMaxBatchSize : Nat := ${nat(consumer.max_batch_size, "max_batch_size")}`);
emit(`def readLimitPerPeriod : Nat := ${nat(readLimiter.limit, "READ_LIMITER.limit")}`);
emit(`def readLimitPeriodSeconds : Nat := ${nat(readLimiter.period, "READ_LIMITER.period")}`);
emit(`def fillLimitPerPeriod : Nat := ${nat(fillLimiter.limit, "FILL_LIMITER.limit")}`);
emit(`def fillLimitPeriodSeconds : Nat := ${nat(fillLimiter.period, "FILL_LIMITER.period")}`);
emit(`def observabilityEnabled : Bool := ${wrangler.observability.enabled}`);
emit("");
emit("/-- `MAX_PKT_LEN` in `services/wit-cache/src/pktline.js`. -/");
emit(`def maxPktLen : Nat := ${nat(maxPktLen, "MAX_PKT_LEN")}`);
emit("");
emit("/-- `services/wit-cache/src/keys.js`: key templates and the `COMPONENT` character class. -/");
emit(`def packKeyPrefix : String := ${str(packTemplate[1])}`);
emit(`def packKeySuffix : String := ${str(packTemplate[2])}`);
emit(`def componentChars : List Char := ${list(componentChars, (c) => `'${c === 39 ? "\\'" : String.fromCharCode(c)}'`)}`);
emit("");
emit("/-- R2 lifecycle rules created by `.github/workflows/cache-worker-deploy.yml`. -/");
emit(`def lifecycleExpirePrefix : String := ${str(expire[1])}`);
emit(`def lifecycleExpireDays : Nat := ${nat(Number(expire[2]), "expire days")}`);
emit(`def lifecycleAbortMultipartDays : Nat := ${nat(abortDays, "abort days")}`);
emit("");
emit("/-- Caller request headers the Worker reads (`index.js` and the modules it runs). -/");
emit(`def callerHeadersRead : List String := ${list([...callerHeaders].sort(), str)}`);
emit("/-- HTTP methods `route` in `index.js` dispatches on. -/");
emit(`def workerMethods : List String := ${list([...methods].sort(), str)}`);
emit("/-- Headers of the Worker's upstream requests to GitHub (`upload-pack.js`). -/");
emit(`def upstreamHeaders : List String := ${list(upstreamHeaders, str)}`);
emit(`def upstreamRedirect : String := ${str(upstreamRedirect)}`);
emit("");
emit("/-- `crates/wit/src/gitops/cloud.rs`. -/");
emit(`def clientDefaultMaxBytes : Nat := ${clientMaxBytes}`);
emit(`def clientDefaultTimeoutMs : Nat := ${clientTimeoutMs}`);
emit(`def clientConnectTimeoutMs : Nat := ${clientConnectMs}`);
emit(`def clientDisableValues : List String := ${list(disabled, str)}`);
emit(`def clientHeaders : List String := ${list(clientHeaders, str)}`);
emit(`def clientRedirect : String := ${str(clientRedirects)}`);
emit(`def clientMethods : List String := ${list(clientMethods, str)}`);
emit("");
emit("/-- `encode_branch_for_path` in `crates/wit/src/gitops/ops.rs` (ADR 0002). -/");
emit(`def branchDirPrefix : String := ${str(branchPrefix)}`);
emit(`def branchSafeBytes : List Nat := ${list(branchSafe)}`);
emit("");
emit("/-- `CliSnapshotBackend` in `crates/wit/src/snapshot/mod.rs`. -/");
emit(`def backendEnvVar : String := ${str(backendEnvVar)}`);
emit(`def backendDefault : Backend := ${backendDefault}`);
emit(`def diskBackendAliases : List String := ${list(diskAliases, str)}`);
emit(`def memoryBackendAliases : List String := ${list(memoryAliases, str)}`);
emit("/-- `crates/wit/src/gitops/cloud.rs`: variable names and `built_in_default` per build profile. -/");
emit(`def cloudUrlEnvVar : String := ${str(cloudUrlEnv)}`);
emit(`def cloudTimeoutEnvVar : String := ${str(cloudTimeoutEnv)}`);
emit(`def cloudMaxBytesEnvVar : String := ${str(cloudMaxBytesEnv)}`);
emit(`def bakedDefaultEnvVar : String := ${str(bakedDefaultEnv)}`);
emit(`def hostedCacheUrl : String := ${str(hostedCacheUrl)}`);
emit(`def debugDefaultUrl : Option String := ${debugDefault}`);
emit(`def releaseDefaultUrl : Option String := ${releaseDefault}`);
emit("/-- `crates/wit/src/gitops/ops.rs`: cache directory and the sources a disk read tries, in order. -/");
emit(`def cacheDirEnvVar : String := ${str(cacheDirEnv)}`);
emit(`def cacheSubdir : String := ${str(cacheSubdir)}`);
emit(`def diskReadOrder : List Source := ${list(diskReadOrder, (s) => sourceCtor[s])}`);
emit("/-- Whether the memory backend's code reaches the disk cache or the cloud client. -/");
emit(`def memoryUsesDiskCache : Bool := ${memoryUsesDisk}`);
emit(`def memoryUsesCloud : Bool := ${memoryUsesCloud}`);
emit("");
emit("/-- Claims of `ROOT_AFTER_HELP`, `BACKEND_HELP`, and `BRANCHES_BACKEND_HELP` in `crates/wit/src/cli.rs`. -/");
emit(`def helpDefaultBackend : Backend := ${helpDefaultBackend}`);
emit(`def helpBackendEnvVar : String := ${str(helpBackendEnvVar)}`);
emit(`def helpFlagDefaultBackends : List Backend := ${list(flagHelps, (f) => f.backend)}`);
emit(`def helpFlagEnvVars : List String := ${list(flagHelps, (f) => str(f.env))}`);
emit(`def helpMemoryCacheDirEnvVar : String := ${str(helpMemoryDir)}`);
emit(`def helpCacheSubdir : String := ${str(helpCacheDir[1])}`);
emit(`def helpCacheDirEnvVar : String := ${str(helpCacheDir[2])}`);
emit(`def helpDiskReadOrder : List Source := ${list(helpDiskReadOrder, (s) => sourceCtor[s])}`);
emit(`def helpCloudUrlEnvVars : List String := ${list([helpCloudUrlEnv, helpDefaults[1], helpDisable[1]], str)}`);
emit(`def helpReleaseDefaultUrl : Option String := some ${str(helpDefaults[2])}`);
emit(`def helpDebugDefaultUrl : Option String := ${helpDefaults[3] === "off" ? "none" : `some ${str(helpDefaults[3])}`}`);
emit(`def helpBakedDefaultEnvVar : String := ${str(helpBakedEnv)}`);
emit(`def helpDisableValues : List String := ${list(helpDisableValues, str)}`);
emit(`def helpCloudTimeoutEnvVar : String := ${str(helpTimeout[1])}`);
emit(`def helpCloudTimeoutMs : Nat := ${nat(Number(helpTimeout[2]), "help timeout")}`);
emit(`def helpCloudMaxBytesEnvVar : String := ${str(helpMaxBytes[1])}`);
emit(`def helpCloudMaxBytes : Nat := ${nat(Number(helpMaxBytes[2]), "help max bytes")}`);
emit("");
emit("end Wit.Src");

const text = lines.join("\n") + "\n";
if (process.argv.includes("--write")) {
  writeFileSync(out, text);
} else {
  process.stdout.write(text);
}
