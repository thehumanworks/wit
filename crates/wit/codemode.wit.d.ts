// Generated from Rust operation contracts. Do not edit by hand.

export type BudgetInfo = { requested_bytes: number; serialized_bytes: number };

export type CacheProvenance = { last_checked_at?: number | null; last_error?: string | null; last_updated_at?: number | null; state: string };

export type ContextItem = { blob_sha: string; commit_sha: string; end_line: number; lines: Array<SourceLine>; path: string; queries: Array<string>; ranking_reasons: Array<string>; repo: string; score: number; snapshot_id: string; start_line: number };

export type Freshness = "allow_stale" | "require_fresh";

export type ListItem = { blob_sha?: string | null; commit_sha: string; kind: string; lines?: number | null; path: string; repo: string; size_bytes?: number | null; snapshot_id: string };

export type ReadLineItem = { blob_sha: string; commit_sha: string; end_line: number; path: string; repo: string; snapshot_id: string; start_line: number; text: string };

export type RefItem = { commit_sha: string; is_default: boolean; kind: string; name: string; repo: string; resolved_ref: string };

export type RepositoryItem = { description?: string | null; full_name: string; html_url?: string | null; language?: string | null; name: string; stars: number };

export type SearchItem = { blob_sha: string; commit_sha: string; end_line: number; lines: Array<SourceLine>; match_line: number; path: string; query: string; repo: string; snapshot_id: string; start_line: number };

export type SnapshotCapabilities = { branches: boolean; full_commit_sha: boolean; pull_request_heads: string; tags: boolean };

export type SourceLine = { line_number: number; text: string };

export type FindRepositoriesInput = { cursor?: string | null; include_rendered_text?: boolean; lang?: string | null; max_bytes?: number | null; max_items?: number | null; pattern?: string | null; query?: string | null };

export type FindRepositoriesResult = { api_version: string; budget: BudgetInfo; has_more: boolean; items: Array<RepositoryItem>; next_cursor?: string | null; rendered_text?: string | null; returned_items: number };

export type RefsInput = { cursor?: string | null; include_rendered_text?: boolean; max_bytes?: number | null; max_items?: number | null; ref?: string | null; repo: string };

export type RefsResult = { api_version: string; budget: BudgetInfo; has_more: boolean; items: Array<RefItem>; next_cursor?: string | null; rendered_text?: string | null; returned_items: number };

export type OpenInput = { freshness?: Freshness; ref?: string | null; repo: string };

export type OpenResult = { api_version: string; cache: CacheProvenance; capabilities: SnapshotCapabilities; commit_sha: string; repo: string; requested_ref: string; resolved_ref: string; snapshot_id: string };

export type ListInput = { cursor?: string | null; depth?: number | null; include_metadata?: boolean; include_rendered_text?: boolean; max_bytes?: number | null; max_items?: number | null; path?: string | null; snapshot_id: string };

export type ListResult = { api_version: string; budget: BudgetInfo; has_more: boolean; items: Array<ListItem>; next_cursor?: string | null; rendered_text?: string | null; returned_items: number };

export type SearchCodeInput = { context_lines?: number | null; cursor?: string | null; globs?: Array<string>; include_rendered_text?: boolean; max_bytes?: number | null; max_results?: number | null; queries: Array<string>; snapshot_id: string };

export type SearchCodeResult = { api_version: string; budget: BudgetInfo; has_more: boolean; items: Array<SearchItem>; next_cursor?: string | null; rendered_text?: string | null; returned_items: number };

export type ReadInput = { cursor?: string | null; end_line?: number | null; include_rendered_text?: boolean; max_bytes?: number | null; max_lines?: number | null; number_lines?: boolean; path: string; snapshot_id: string; start_line?: number | null };

export type ReadResult = { api_version: string; budget: BudgetInfo; has_more: boolean; items: Array<ReadLineItem>; next_cursor?: string | null; rendered_text?: string | null; returned_items: number };

export type ContextInput = { context_lines?: number | null; cursor?: string | null; globs?: Array<string>; include_rendered_text?: boolean; max_bytes?: number | null; max_results?: number | null; queries: Array<string>; snapshot_id: string };

export type ContextResult = { api_version: string; budget: BudgetInfo; has_more: boolean; items: Array<ContextItem>; next_cursor?: string | null; rendered_text?: string | null; returned_items: number };

export interface WitCodeModeApi {
  /** Discover GitHub repositories; use this only when owner/repo is unknown, then call wit_open */
  findRepositories(arguments: FindRepositoriesInput): Promise<FindRepositoriesResult>;
  /** Discover default branch, branches, and tags, or resolve one ref before opening an immutable snapshot */
  refs(arguments: RefsInput): Promise<RefsResult>;
  /** Open one immutable repository snapshot before listing, searching, or reading; reuse its snapshot_id to prevent mixed revisions */
  open(arguments: OpenInput): Promise<OpenResult>;
  /** List bounded repository structure from a snapshot with explicit depth; use before code search when paths are unknown */
  list(arguments: ListInput): Promise<ListResult>;
  /** Search one immutable snapshot with one or more regex queries and return bounded atomic context groups with provenance */
  searchCode(arguments: SearchCodeInput): Promise<SearchCodeResult>;
  /** Read an explicit one-based inclusive line range from a snapshot; use after list or search identifies a file */
  read(arguments: ReadInput): Promise<ReadResult>;
  /** Gather deterministic ranked multi-file evidence from a snapshot; use when one answer needs several bounded supporting snippets */
  context(arguments: ContextInput): Promise<ContextResult>;
}

export {};

declare global {
  const codemode: {
    readonly wit: WitCodeModeApi;
  };
}
