/**
 * Parse the try-it subset: `wit tree|ls|cat` with CLI owner/repo + path shapes.
 * Rejects rg/sed/head/tail and unknown flags. `-r`/`--repo` and `--` match CLI.
 */

export const USAGE = `usage: wit tree|ls|cat owner/repo [path]
       wit tree owner/repo
       wit ls owner/repo
       wit cat owner/repo PATH
       wit tree -r owner/repo [path]
       wit cat -r owner/repo PATH

This page only runs tree, ls, and cat through wit_snapshot.wasm
(open / list / read). Fixture repo: demo/repo`;

const ALLOWED = new Set(["tree", "ls", "cat"]);

/**
 * @param {string} line
 * @returns {{
 *   kind: 'empty' | 'help' | 'clear' | 'run' | 'error',
 *   command?: 'tree' | 'ls' | 'cat',
 *   repo?: string,
 *   path?: string | null,
 *   message?: string,
 * }}
 */
export function parseCommand(line) {
  let tokens;
  try {
    tokens = tokenize(line);
  } catch (err) {
    return { kind: "error", message: String(err.message || err) };
  }
  if (tokens.length === 0) {
    return { kind: "empty" };
  }

  if (tokens.length === 1 && tokens[0] === "clear") {
    return { kind: "clear" };
  }
  if (
    tokens[0] === "help" ||
    (tokens[0] === "wit" && (tokens[1] === "--help" || tokens[1] === "-h"))
  ) {
    return { kind: "help", message: USAGE };
  }

  if (tokens[0] !== "wit") {
    return {
      kind: "error",
      message: `bad command: expected 'wit tree|ls|cat ...', got ${JSON.stringify(line.trim())}`,
    };
  }

  const rest = tokens.slice(1);
  if (rest.length === 0) {
    return { kind: "error", message: USAGE };
  }

  let repoFlag = null;
  let command = null;
  const args = [];

  for (let i = 0; i < rest.length; i += 1) {
    const token = rest[i];
    if (token === "--") {
      args.push(...rest.slice(i + 1));
      break;
    }
    if (token === "-r" || token === "--repo") {
      const value = rest[i + 1];
      if (!value || value.startsWith("-")) {
        return {
          kind: "error",
          message: "missing repository: pass owner/repo as a positional argument or with -r/--repo",
        };
      }
      repoFlag = value;
      i += 1;
      continue;
    }
    if (token.startsWith("-")) {
      return {
        kind: "error",
        message: `unknown flag ${token} (this page accepts only wit tree|ls|cat; no -l/-n/rg/sed/head/tail)`,
      };
    }
    if (!command) {
      command = token;
      continue;
    }
    args.push(token);
  }

  if (!command) {
    return { kind: "error", message: USAGE };
  }
  if (!ALLOWED.has(command)) {
    return {
      kind: "error",
      message: `bad command: '${command}' is not available here (only tree, ls, cat)`,
    };
  }

  try {
    const resolved =
      command === "cat"
        ? resolveRepoAndRequiredPath(repoFlag, args)
        : resolveRepoAndOptionalPath(repoFlag, args);
    return {
      kind: "run",
      command,
      repo: resolved.repo,
      path: resolved.path,
    };
  } catch (err) {
    return { kind: "error", message: String(err.message || err) };
  }
}

/**
 * @param {string} line
 * @returns {string[]}
 */
export function tokenize(line) {
  const out = [];
  let current = "";
  let quote = null;
  const text = String(line ?? "");
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i];
    if (quote) {
      if (ch === quote) {
        quote = null;
      } else {
        current += ch;
      }
      continue;
    }
    if (ch === "'" || ch === '"') {
      quote = ch;
      continue;
    }
    if (/\s/.test(ch)) {
      if (current) {
        out.push(current);
        current = "";
      }
      continue;
    }
    current += ch;
  }
  if (quote) {
    throw new Error("unclosed quote");
  }
  if (current) {
    out.push(current);
  }
  return out;
}

function resolveRepoAndOptionalPath(repoFlag, args) {
  if (!repoFlag) {
    if (args.length === 0) {
      throw new Error(
        "missing repository: pass owner/repo as a positional argument or with -r/--repo",
      );
    }
    if (args.length === 1) {
      return { repo: args[0], path: null };
    }
    if (args.length === 2) {
      return { repo: args[0], path: args[1] };
    }
    throw new Error("too many arguments: expected owner/repo [path]");
  }
  if (args.length === 0) {
    return { repo: repoFlag, path: null };
  }
  if (args.length === 1) {
    if (args[0] === repoFlag) {
      return { repo: repoFlag, path: null };
    }
    return { repo: repoFlag, path: args[0] };
  }
  if (args.length === 2) {
    if (args[0] !== repoFlag) {
      throw new Error(
        `conflicting repository arguments: -r/--repo '${repoFlag}' vs positional '${args[0]}'`,
      );
    }
    return { repo: repoFlag, path: args[1] };
  }
  throw new Error("too many arguments: expected [-r owner/repo] [owner/repo] [path]");
}

function resolveRepoAndRequiredPath(repoFlag, args) {
  if (!repoFlag) {
    if (args.length === 2) {
      return { repo: args[0], path: args[1] };
    }
    if (args.length < 2) {
      throw new Error(
        "missing arguments: expected owner/repo PATH (or -r owner/repo PATH)",
      );
    }
    throw new Error("too many arguments: expected owner/repo PATH");
  }
  if (args.length === 1) {
    return { repo: repoFlag, path: args[0] };
  }
  if (args.length === 2) {
    if (args[0] !== repoFlag) {
      throw new Error(
        `conflicting repository arguments: -r/--repo '${repoFlag}' vs positional '${args[0]}'`,
      );
    }
    return { repo: repoFlag, path: args[1] };
  }
  if (args.length === 0) {
    throw new Error("missing path: expected PATH after repository");
  }
  throw new Error("too many arguments: expected [-r owner/repo] [owner/repo] PATH");
}
