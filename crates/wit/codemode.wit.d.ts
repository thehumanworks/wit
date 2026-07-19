// Generated from Rust operation contracts. Do not edit by hand.

export type BudgetInfo = { remaining_bytes: number; requested_bytes: number; serialized_bytes: number; warning?: string | null };

export type CacheProvenance = { last_checked_at?: number | null; last_error?: string | null; last_updated_at?: number | null; state: string };

export type CompactListFormat = "paths";

export type CompactListPage = { api_version: string; budget: BudgetInfo; commit_sha: string; format: CompactListFormat; has_more: boolean; next_cursor?: string | null; paths: Array<string>; repo: string; returned_items: number; snapshot_id: string };

export type CompactReadLinesFormat = "lines";

export type CompactReadLinesPage = { api_version: string; blob_sha?: string | null; budget: BudgetInfo; commit_sha: string; end_line?: number | null; format: CompactReadLinesFormat; has_more: boolean; lines: Array<SourceLine>; next_cursor?: string | null; path: string; repo: string; returned_lines: number; snapshot_id: string; start_line?: number | null };

export type CompactReadTextFormat = "text";

export type CompactReadTextPage = { api_version: string; blob_sha?: string | null; budget: BudgetInfo; commit_sha: string; end_line?: number | null; format: CompactReadTextFormat; has_more: boolean; next_cursor?: string | null; path: string; repo: string; returned_lines: number; snapshot_id: string; start_line?: number | null; text: string };

export type ContextItem = { blob_sha: string; commit_sha: string; end_line: number; lines: Array<SourceLine>; path: string; queries: Array<string>; ranking_reasons: Array<string>; repo: string; score: number; snapshot_id: string; start_line: number };

export type Freshness = "allow_stale" | "require_fresh";

export type ListFormat = "structured" | "paths";

export type ListItem = { blob_sha?: string | null; commit_sha: string; kind: string; lines?: number | null; path: string; repo: string; size_bytes?: number | null; snapshot_id: string };

export type ReadFormat = "structured" | "text" | "lines";

export type ReadLineItem = { blob_sha: string; commit_sha: string; end_line: number; path: string; repo: string; snapshot_id: string; start_line: number; text: string };

export type RefItem = { commit_sha: string; is_default: boolean; kind: string; name: string; repo: string; resolved_ref: string };

export type RepositoryItem = { description?: string | null; full_name: string; html_url?: string | null; language?: string | null; name: string; stars: number };

export type SearchItem = { blob_sha: string; commit_sha: string; end_line: number; lines: Array<SourceLine>; match_line: number; path: string; query: string; repo: string; snapshot_id: string; start_line: number };

export type SnapshotCapabilities = { branches: boolean; full_commit_sha: boolean; pull_request_heads: string; tags: boolean };

export type SourceLine = { line_number: number; text: string };

export type StructuredListPage = { api_version: string; budget: BudgetInfo; has_more: boolean; items: Array<ListItem>; next_cursor?: string | null; rendered_text?: string | null; returned_items: number };

export type StructuredReadPage = { api_version: string; budget: BudgetInfo; has_more: boolean; items: Array<ReadLineItem>; next_cursor?: string | null; rendered_text?: string | null; returned_items: number };

export type FindRepositoriesInput = { cursor?: string | null; include_rendered_text?: boolean; lang?: string | null; max_bytes?: number | null; max_items?: number | null; pattern?: string | null; query?: string | null };

export type FindRepositoriesResult = { api_version: string; budget: BudgetInfo; has_more: boolean; items: Array<RepositoryItem>; next_cursor?: string | null; rendered_text?: string | null; returned_items: number };

export type RefsInput = { cursor?: string | null; include_rendered_text?: boolean; max_bytes?: number | null; max_items?: number | null; ref?: string | null; repo: string };

export type RefsResult = { api_version: string; budget: BudgetInfo; has_more: boolean; items: Array<RefItem>; next_cursor?: string | null; rendered_text?: string | null; returned_items: number };

export type OpenInput = { freshness?: Freshness; ref?: string | null; repo: string };

export type OpenResult = { api_version: string; cache: CacheProvenance; capabilities: SnapshotCapabilities; commit_sha: string; repo: string; requested_ref: string; resolved_ref: string; snapshot_id: string };

export type ListInput = { cursor?: string | null; depth?: number | null; format?: ListFormat; include_metadata?: boolean; include_rendered_text?: boolean; max_bytes?: number | null; max_items?: number | null; path?: string | null; snapshot_id: string };

export type ListResult = StructuredListPage | CompactListPage;

export type SearchCodeInput = { context_lines?: number | null; cursor?: string | null; exclude?: Array<string>; glob?: string | null; globs?: Array<string>; include_rendered_text?: boolean; max_bytes?: number | null; max_results?: number | null; path_prefix?: string | null; queries: Array<string>; snapshot_id: string };

export type SearchCodeResult = { api_version: string; budget: BudgetInfo; has_more: boolean; items: Array<SearchItem>; next_cursor?: string | null; rendered_text?: string | null; returned_items: number };

export type ReadInput = { cursor?: string | null; end_line?: number | null; format?: ReadFormat; include_rendered_text?: boolean; max_bytes?: number | null; max_lines?: number | null; number_lines?: boolean; path: string; snapshot_id: string; start_line?: number | null };

export type ReadResult = StructuredReadPage | CompactReadTextPage | CompactReadLinesPage;

export type ContextInput = { context_lines?: number | null; cursor?: string | null; globs?: Array<string>; include_rendered_text?: boolean; max_bytes?: number | null; max_results?: number | null; queries: Array<string>; snapshot_id: string };

export type ContextResult = { api_version: string; budget: BudgetInfo; has_more: boolean; items: Array<ContextItem>; next_cursor?: string | null; rendered_text?: string | null; returned_items: number };

export type WitCodeModeMethod = "findRepositories" | "refs" | "open" | "list" | "searchCode" | "read" | "context";

export type WitCodeModeHelpEntry = { name: WitCodeModeMethod; signature: string; description: string; example: string };

export type WitCodeModeHelp = { namespace: "codemode.wit"; methods: Array<WitCodeModeHelpEntry>; limits: { final_result_bytes: number; host_result_bytes: number }; guidance: string };

export interface WitCodeModeApi {
  /** List all methods and signatures, or describe one method without a host call. */
  help(): WitCodeModeHelp;
  help(method: WitCodeModeMethod): WitCodeModeHelpEntry;
  /** Discover GitHub repositories when owner/repo is unknown; for fuzzy names use pattern plus a small max_items (for example { pattern: 'ratatuizilla', max_items: 5 }), then call open */
  findRepositories(arguments: FindRepositoriesInput): Promise<FindRepositoriesResult>;
  /** Discover default branch, branches, and tags, or resolve one ref before opening an immutable snapshot */
  refs(arguments: RefsInput): Promise<RefsResult>;
  /** Open one immutable repository snapshot before listing, searching, or reading; reuse its snapshot_id to prevent mixed revisions */
  open(arguments: OpenInput): Promise<OpenResult>;
  /** List bounded repository structure from a snapshot with explicit depth; use format: 'paths' for a compact paths-only result */
  list(arguments: ListInput & { format: "paths" }): Promise<CompactListPage>;
  list(arguments: ListInput & { format?: "structured" }): Promise<StructuredListPage>;
  /** Search one immutable snapshot with regex queries; narrow results with path_prefix, glob/globs, and exclude filters */
  searchCode(arguments: SearchCodeInput): Promise<SearchCodeResult>;
  /** Read an explicit one-based inclusive line range; Code Mode defaults to compact text and supports lines or structured formats */
  read(arguments: ReadInput & { format: "structured" }): Promise<StructuredReadPage>;
  read(arguments: ReadInput & { format: "lines" }): Promise<CompactReadLinesPage>;
  read(arguments: ReadInput & { format?: "text" }): Promise<CompactReadTextPage>;
  /** Gather deterministic ranked multi-file evidence from a snapshot; use when one answer needs several bounded supporting snippets */
  context(arguments: ContextInput): Promise<ContextResult>;
}

export {};

declare global {
  const codemode: {
    readonly wit: WitCodeModeApi;
  };
}
