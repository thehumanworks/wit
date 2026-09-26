/**
 * Canonical cache keys. Owner and repo are lowercased (GitHub names are
 * case-insensitive) and validated so no request can influence the key beyond
 * `{owner}/{repo}/{commit}`.
 */

const COMPONENT = /^[A-Za-z0-9._-]{1,100}$/;
const SHA1 = /^[0-9a-f]{40}$/;
const BRANCH_CHARS = /^[A-Za-z0-9._\/+-]{1,255}$/;

/** @param {string} value */
export function isSafeComponent(value) {
  return COMPONENT.test(value) && value !== "." && value !== ".." && !value.endsWith(".git");
}

/** @param {string} value */
export function isCommitSha(value) {
  return SHA1.test(value);
}

/**
 * Conservative subset of git's ref-name rules: enough for real branch names,
 * strict enough that the value is safe inside a pkt-line and a header.
 * @param {string} value
 */
export function isSafeBranch(value) {
  if (!BRANCH_CHARS.test(value)) return false;
  if (value.startsWith("-") || value.startsWith("/") || value.endsWith("/")) return false;
  if (value.endsWith(".") || value.endsWith(".lock")) return false;
  if (value.includes("..") || value.includes("//") || value.includes("/.")) return false;
  return !value.startsWith(".");
}

/**
 * @param {string} owner
 * @param {string} repo
 */
export function repoId(owner, repo) {
  return `${owner.toLowerCase()}/${repo.toLowerCase()}`;
}

/**
 * @param {string} owner
 * @param {string} repo
 * @param {string} commit
 */
export function packKey(owner, repo, commit) {
  return `v1/github/${repoId(owner, repo)}/${commit}.pack`;
}

/**
 * @param {string} owner
 * @param {string} repo
 */
export function repoPrefix(owner, repo) {
  return `v1/github/${repoId(owner, repo)}/`;
}

/**
 * Parse `/v1/github/{owner}/{repo}/{commit}.pack`.
 * @param {string} pathname
 * @returns {{ owner: string, repo: string, commit: string } | null}
 */
export function parsePackPath(pathname) {
  const m = pathname.match(/^\/v1\/github\/([^/]+)\/([^/]+)\/([^/]+)\.pack$/);
  if (!m) return null;
  const [, owner, repo, commit] = m;
  if (!isSafeComponent(owner) || !isSafeComponent(repo) || !isCommitSha(commit)) return null;
  return { owner, repo, commit };
}

/**
 * Parse `/v1/github/{owner}/{repo}` (admin takedown route).
 * @param {string} pathname
 * @returns {{ owner: string, repo: string } | null}
 */
export function parseRepoPath(pathname) {
  const m = pathname.match(/^\/v1\/github\/([^/]+)\/([^/]+)$/);
  if (!m) return null;
  const [, owner, repo] = m;
  if (!isSafeComponent(owner) || !isSafeComponent(repo)) return null;
  return { owner, repo };
}
