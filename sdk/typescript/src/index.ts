/**
 * wit URL API client — zero-dependency TypeScript SDK for
 * https://wit.thehuman.sh/api (or any self-hosted `showcase/url-api`).
 *
 * Every method maps to one GET request with `?format=json`, returns the
 * typed JSON body (which carries provenance: repo, ref, commit, cache), and
 * throws `WitError` with the API's `code`/`status` on failure. Runs on Node
 * 18+, Deno, Bun, Workers, and browsers — anything with global `fetch`.
 *
 * Chaining example (the workflow an agent should follow):
 *
 *   const wit = new WitClient();
 *   const repo = wit.repo("ratatui/ratatui");
 *   const stats = await repo.stats();                 // how big, which languages
 *   const hits = await repo.rgFiles("impl Widget for", { glob: "*.rs" });
 *   const outline = await repo.outline(hits.files[0].path);
 *   const sym = outline.symbols.find((s) => s.kind === "impl")!;
 *   const code = await repo.cat(hits.files[0].path, { lines: [sym.line, sym.end_line] });
 */

export const DEFAULT_BASE_URL = "https://wit.thehuman.sh/api";

export type FetchLike = (input: string, init?: RequestInit) => Promise<Response>;

export interface WitClientOptions {
  /** API origin including the `/api` prefix. Defaults to the public host. */
  baseUrl?: string;
  /** GitHub token sent as `Authorization: Bearer` (private repos, your own quota). */
  token?: string | null;
  /** Custom fetch (tests, proxies). Defaults to `globalThis.fetch`. */
  fetch?: FetchLike;
  /** Extra headers on every request (e.g. a User-Agent). */
  headers?: Record<string, string>;
  /**
   * How many times to retry a 429 while honouring `retry-after`, sleeping at
   * most `maxRetryDelayMs` per attempt. Default 0 (fail fast).
   */
  retries?: number;
  maxRetryDelayMs?: number;
}

/** Common provenance carried by every snapshot response. */
export interface Provenance {
  api_version: string;
  verb: string;
  repo: string;
  requested_ref: string;
  ref: string;
  commit: string;
  cache: "hit" | "miss";
}

export interface TreeFile {
  path: string;
  size_bytes: number | null;
  tokens_est: number | null;
}
export interface TreeResult extends Provenance {
  path: string;
  depth: number | null;
  files: TreeFile[];
}

export interface LsEntry {
  name: string;
  kind: "file" | "dir";
  path: string;
  size_bytes: number | null;
  tokens_est: number | null;
  blob_sha: string | null;
}
export interface LsResult extends Provenance {
  path: string;
  entries: LsEntry[];
}

export interface CatResult extends Provenance {
  path: string;
  blob_sha: string;
  size_bytes?: number;
  total_lines: number;
  start_line: number;
  end_line: number;
  text: string;
}

export interface StatsBucket {
  files: number;
  bytes: number;
  tokens_est: number;
}
export interface StatsResult extends Provenance {
  path: string;
  files: number;
  bytes: number;
  tokens_est: number;
  directories: Array<StatsBucket & { name: string }>;
  languages: Array<StatsBucket & { language: string }>;
  largest_files: Array<{ path: string; bytes: number; tokens_est: number; binary: boolean }>;
  binary_files: number;
  max_depth: number;
}

export interface OutlineSymbol {
  line: number;
  end_line: number;
  kind: string;
  name: string;
  signature: string;
}
export interface OutlineResult extends Provenance {
  path: string;
  blob_sha: string;
  language: string | null;
  supported: boolean;
  total_lines: number;
  truncated: boolean;
  symbols: OutlineSymbol[];
}

export interface RgMatch {
  path: string;
  line: number;
  text: string;
  is_context: boolean;
}
export interface RgBase extends Provenance {
  pattern: string;
  path: string;
  glob: string | null;
  files_scanned: number;
  files_candidate: number;
  files_skipped_binary: number;
  match_count: number;
  truncated: boolean;
  truncated_reason: "rate_limited" | "max_matches" | "max_files" | null;
}
export interface RgMatchesResult extends RgBase {
  matches: RgMatch[];
}
export interface RgFilesResult extends RgBase {
  files: Array<{ path: string; lines: number | null }>;
}
export interface RgCountsResult extends RgBase {
  counts: Array<{ path: string; count: number }>;
}

export interface RefsResult {
  api_version: string;
  verb: "refs";
  repo: string;
  default_branch: string;
  branches: Array<{ name: string; sha: string }>;
  tags: Array<{ name: string; sha: string }>;
}

export interface Commit {
  sha: string;
  author: string;
  date: string;
  message: string;
}
export interface CommitsResult {
  api_version: string;
  verb: "commits";
  repo: string;
  ref: string | null;
  path: string | null;
  commits: Commit[];
}

export interface RepositoryItem {
  full_name: string;
  description: string | null;
  language: string | null;
  stars: number;
  forks: number;
  html_url: string | null;
  default_branch: string | null;
  pushed_at: string | null;
  archived: boolean;
  topics: string[];
}
export interface SearchResult {
  api_version: string;
  verb: "search";
  query: string;
  sort: string;
  total_count: number;
  items: RepositoryItem[];
}

export interface CommonOptions {
  /** Re-resolve the ref instead of serving the cached pin (`?fresh=1`). */
  fresh?: boolean;
  /** Globs to exclude (`?ignore=`), like the CLI's `--ignore`. */
  ignore?: string[];
}
export interface TreeOptions extends CommonOptions {
  path?: string;
  depth?: number;
}
export interface StatsOptions extends CommonOptions {
  path?: string;
  largest?: number;
}
export interface CatOptions extends CommonOptions {
  /** One-based inclusive `[start, end]`; either side may be null for open ranges. */
  lines?: [number | null, number | null] | string;
}
export interface TailOptions extends CommonOptions {
  lines?: number;
  /** Read from this one-based line to the end (`tail -n +N`). */
  plus?: number;
}
export interface RgOptions extends CommonOptions {
  path?: string;
  glob?: string;
  ignoreCase?: boolean;
  smartCase?: boolean;
  wordRegexp?: boolean;
  invert?: boolean;
  /** Context lines before and after (`C`); `before`/`after` override it. */
  context?: number;
  before?: number;
  after?: number;
  /** Max matches (server default 200, ceiling 2000). */
  max?: number;
  /** Max files scanned (server default 200, ceiling 1000). */
  maxFiles?: number;
  /** With `filesOnly`, include line counts. */
  long?: boolean;
}
export interface SearchOptions {
  /** Raw GitHub repository-search query (`terminal ui stars:>100`). */
  query?: string;
  /** Repository name filter (`NAME in:name`). */
  pattern?: string;
  lang?: string;
  limit?: number;
  sort?: "stars" | "updated" | "forks" | "best";
}

/** Error raised for any non-2xx response, carrying the API's code and status. */
export class WitError extends Error {
  readonly status: number;
  readonly code: string;
  readonly retryAfter: number | null;
  readonly url: string;

  constructor(message: string, info: { status: number; code: string; retryAfter?: number | null; url: string }) {
    super(message);
    this.name = "WitError";
    this.status = info.status;
    this.code = info.code;
    this.retryAfter = info.retryAfter ?? null;
    this.url = info.url;
  }

  /** True for 429s: the GitHub quota behind the host (or your token) is exhausted. */
  get isRateLimited(): boolean {
    return this.status === 429;
  }
}

type Query = Record<string, string | number | boolean | string[] | null | undefined>;

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Build the query string, dropping empty values and expanding arrays. */
export function buildQuery(params: Query): string {
  const q = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value == null || value === false || value === "") continue;
    if (Array.isArray(value)) {
      for (const v of value) if (v != null && v !== "") q.append(key, String(v));
    } else if (value === true) {
      q.set(key, "1");
    } else {
      q.set(key, String(value));
    }
  }
  return q.toString();
}

function linesParam(lines: CatOptions["lines"]): string | undefined {
  if (lines == null) return undefined;
  if (typeof lines === "string") return lines;
  const [start, end] = lines;
  if (start == null && end == null) return undefined;
  return `${start ?? ""}-${end ?? ""}`;
}

export class WitClient {
  readonly baseUrl: string;
  private readonly token: string | null;
  private readonly fetchImpl: FetchLike;
  private readonly headers: Record<string, string>;
  private readonly retries: number;
  private readonly maxRetryDelayMs: number;

  constructor(options: WitClientOptions = {}) {
    this.baseUrl = (options.baseUrl ?? DEFAULT_BASE_URL).replace(/\/+$/, "");
    this.token = options.token ?? null;
    const f = options.fetch ?? (globalThis.fetch as FetchLike | undefined);
    if (!f) throw new Error("WitClient needs a fetch implementation (pass options.fetch)");
    this.fetchImpl = f;
    this.headers = { ...(options.headers ?? {}) };
    this.retries = Math.max(0, options.retries ?? 0);
    this.maxRetryDelayMs = options.maxRetryDelayMs ?? 30_000;
  }

  /** Bind a repository (and optional ref) for chained calls. */
  repo(ownerRepo: string, ref?: string | null): WitRepo {
    return new WitRepo(this, ownerRepo, ref ?? null);
  }

  /** GitHub repository search: find `owner/repo` for "libraries that do X". */
  async search(options: SearchOptions): Promise<SearchResult> {
    return this.getJson<SearchResult>("/search", {
      q: options.query,
      p: options.pattern,
      lang: options.lang,
      limit: options.limit,
      sort: options.sort,
    });
  }

  /** Fetch the agent guide served at `/llms.txt`. */
  async llmsText(): Promise<string> {
    return this.getText("/llms.txt", {});
  }

  /** Low-level: one verb as JSON. */
  async getJson<T>(path: string, params: Query): Promise<T> {
    const res = await this.request(path, { ...params, format: "json" }, "application/json");
    return (await res.json()) as T;
  }

  /** Low-level: one verb as the CLI-identical plaintext. */
  async getText(path: string, params: Query): Promise<string> {
    const res = await this.request(path, params, "text/plain");
    return res.text();
  }

  private async request(path: string, params: Query, accept: string): Promise<Response> {
    const query = buildQuery(params);
    const url = `${this.baseUrl}${path.startsWith("/") ? path : `/${path}`}${query ? `?${query}` : ""}`;
    const headers: Record<string, string> = { Accept: accept, ...this.headers };
    if (this.token) headers.Authorization = `Bearer ${this.token}`;

    let attempt = 0;
    for (;;) {
      const res = await this.fetchImpl(url, { method: "GET", headers });
      if (res.ok) return res;
      const err = await this.toError(res, url);
      if (err.status === 429 && attempt < this.retries) {
        attempt += 1;
        const delay = Math.min((err.retryAfter ?? 1) * 1000, this.maxRetryDelayMs);
        await sleep(delay);
        continue;
      }
      throw err;
    }
  }

  private async toError(res: Response, url: string): Promise<WitError> {
    const retryHeader = res.headers.get("retry-after");
    const retryAfter = retryHeader != null && retryHeader !== "" ? Number(retryHeader) : null;
    const body = await res.text();
    let message = body.trim();
    let code = "error";
    try {
      const parsed = JSON.parse(body) as { error?: string; code?: string };
      if (parsed && typeof parsed.error === "string") {
        message = parsed.error;
        code = parsed.code ?? code;
      }
    } catch {
      message = message.replace(/^error:\s*/, "");
    }
    return new WitError(message || `HTTP ${res.status}`, {
      status: res.status,
      code,
      retryAfter: Number.isFinite(retryAfter as number) ? retryAfter : null,
      url,
    });
  }
}

/** A repository bound to a client and (optionally) a ref. All methods are one request. */
export class WitRepo {
  readonly client: WitClient;
  readonly ownerRepo: string;
  readonly ref: string | null;

  constructor(client: WitClient, ownerRepo: string, ref: string | null) {
    if (!/^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/.test(ownerRepo)) {
      throw new Error(`expected owner/repo, got '${ownerRepo}'`);
    }
    this.client = client;
    this.ownerRepo = ownerRepo;
    this.ref = ref;
  }

  /** Same repository at another branch, tag, or commit SHA. */
  at(ref: string | null): WitRepo {
    return new WitRepo(this.client, this.ownerRepo, ref);
  }

  private verb<T>(verb: string, params: Query, common?: CommonOptions): Promise<T> {
    return this.client.getJson<T>(`/${verb}/${this.ownerRepo}`, {
      ref: this.ref,
      fresh: common?.fresh,
      ignore: common?.ignore,
      ...params,
    });
  }

  /** CLI-identical plaintext for any verb (`text("tree", { path: "src", l: 1 })`). */
  text(verb: string, params: Query = {}): Promise<string> {
    return this.client.getText(`/${verb}/${this.ownerRepo}`, { ref: this.ref, ...params });
  }

  stats(options: StatsOptions = {}): Promise<StatsResult> {
    return this.verb<StatsResult>("stats", { path: options.path, largest: options.largest }, options);
  }

  tree(options: TreeOptions = {}): Promise<TreeResult> {
    return this.verb<TreeResult>("tree", { path: options.path, depth: options.depth, l: true }, options);
  }

  ls(path?: string, options: CommonOptions = {}): Promise<LsResult> {
    return this.verb<LsResult>("ls", { path, l: true }, options);
  }

  cat(path: string, options: CatOptions = {}): Promise<CatResult> {
    return this.verb<CatResult>("cat", { path, lines: linesParam(options.lines) }, options);
  }

  head(path: string, lines?: number, options: CommonOptions = {}): Promise<CatResult> {
    return this.verb<CatResult>("head", { path, lines }, options);
  }

  tail(path: string, options: TailOptions = {}): Promise<CatResult> {
    return this.verb<CatResult>("tail", { path, lines: options.lines, plus: options.plus }, options);
  }

  outline(path: string, options: CommonOptions & { maxSymbols?: number } = {}): Promise<OutlineResult> {
    return this.verb<OutlineResult>("outline", { path, max_symbols: options.maxSymbols }, options);
  }

  private rgParams(pattern: string, options: RgOptions): Query {
    return {
      q: pattern,
      path: options.path,
      glob: options.glob,
      i: options.ignoreCase,
      S: options.smartCase,
      w: options.wordRegexp,
      v: options.invert,
      C: options.context,
      B: options.before,
      A: options.after,
      max: options.max,
      max_files: options.maxFiles,
      long: options.long,
    };
  }

  /** Bounded regex search returning matching lines (with context when asked). */
  rg(pattern: string, options: RgOptions = {}): Promise<RgMatchesResult> {
    return this.verb<RgMatchesResult>("rg", this.rgParams(pattern, options), options);
  }

  /** `rg -l`: only the files that match — the cheapest way to locate code. */
  rgFiles(pattern: string, options: RgOptions = {}): Promise<RgFilesResult> {
    return this.verb<RgFilesResult>("rg", { ...this.rgParams(pattern, options), l: true }, options);
  }

  /** `rg -c`: match counts per file. */
  rgCounts(pattern: string, options: RgOptions = {}): Promise<RgCountsResult> {
    return this.verb<RgCountsResult>("rg", { ...this.rgParams(pattern, options), c: true }, options);
  }

  refs(): Promise<RefsResult> {
    return this.client.getJson<RefsResult>(`/refs/${this.ownerRepo}`, {});
  }

  commits(options: { path?: string; n?: number } = {}): Promise<CommitsResult> {
    return this.client.getJson<CommitsResult>(`/commits/${this.ownerRepo}`, {
      ref: this.ref,
      path: options.path,
      n: options.n,
    });
  }

  /**
   * Read one symbol's source by name: `outline` then `cat?lines=`. Returns
   * null when the outline has no such symbol. Two requests.
   */
  async readSymbol(
    path: string,
    name: string,
    options: { kind?: string; padding?: number } = {},
  ): Promise<(CatResult & { symbol: OutlineSymbol }) | null> {
    const outline = await this.outline(path);
    const symbol = outline.symbols.find(
      (s) => s.name === name && (options.kind == null || s.kind === options.kind),
    );
    if (!symbol) return null;
    const pad = Math.max(0, options.padding ?? 0);
    const start = Math.max(1, symbol.line - pad);
    const end = Math.min(outline.total_lines, symbol.end_line + pad);
    const code = await this.cat(path, { lines: [start, end] });
    return { ...code, symbol };
  }

  /**
   * Locate then read: `rg -l` for the pattern, then `cat` a window around
   * the first match in each file (up to `maxFiles`). Returns provenance-
   * bearing snippets ready to quote. 1 + n requests.
   */
  async context(
    pattern: string,
    options: RgOptions & { window?: number; maxSnippets?: number } = {},
  ): Promise<Array<{ path: string; start_line: number; end_line: number; text: string; commit: string }>> {
    const window = Math.max(0, options.window ?? 20);
    const maxSnippets = Math.max(1, options.maxSnippets ?? 5);
    const hits = await this.rg(pattern, { ...options, max: options.max ?? maxSnippets * 4 });
    const seen = new Set<string>();
    const snippets: Array<{ path: string; start_line: number; end_line: number; text: string; commit: string }> = [];
    for (const match of hits.matches) {
      if (match.is_context || seen.has(match.path)) continue;
      seen.add(match.path);
      const code = await this.cat(match.path, {
        lines: [Math.max(1, match.line - window), match.line + window],
      });
      snippets.push({
        path: match.path,
        start_line: code.start_line,
        end_line: code.end_line,
        text: code.text,
        commit: code.commit,
      });
      if (snippets.length >= maxSnippets) break;
    }
    return snippets;
  }
}
