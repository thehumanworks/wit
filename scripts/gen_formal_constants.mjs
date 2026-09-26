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
const { DEFAULTS, NEGATIVE_TTL, REPO_SCOPED, PENDING_TTL_SECONDS } = await import(configUrl);

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
const takedownBody = match(coordinatorRel, coordinator, /async takedown\(target\) \{[\s\S]*?\n  \}\n/, "takedown")[0];
const blockAt = takedownBody.indexOf("INSERT INTO negatives");
const firstAwait = takedownBody.indexOf("await ");
if (blockAt < 0 || (firstAwait >= 0 && firstAwait < blockAt)) {
  fail(`${coordinatorRel}: takedown must record the block before its first await (Wit.Coordinator models it as one step)`);
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
const limiter = (name) => {
  const rl = (wrangler.ratelimits ?? []).find((r) => r.name === name);
  if (!rl?.simple) fail(`wrangler.toml: rate limiter ${name} is missing`);
  return rl.simple;
};
const readLimiter = limiter("READ_LIMITER");
const fillLimiter = limiter("FILL_LIMITER");
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
emit("end Wit.Src");

const text = lines.join("\n") + "\n";
if (process.argv.includes("--write")) {
  writeFileSync(out, text);
} else {
  process.stdout.write(text);
}
