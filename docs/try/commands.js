/**
 * Parse the try-it subset: repo-reading verbs as host JS views.
 * `-r`/`--repo` and `--` match CLI. Extra verbs print "not available".
 * `search` is out of this page (pending CPO cut) — print, do not run.
 */

export const USAGE = `usage: wit tree|ls|cat|rg|sed|head|tail owner/repo [path]
       wit tree owner/repo
       wit ls owner/repo
       wit cat owner/repo PATH
       wit rg PATTERN owner/repo [path]
       wit rg -i|-l PATTERN owner/repo
       wit sed -n '1,5p' owner/repo PATH
       wit sed -n '/pattern/p' owner/repo PATH
       wit head [-n N] [-N] owner/repo PATH
       wit tail [-n N] [-p LINE] owner/repo PATH
       wit tree -r owner/repo [path]

This page runs tree, ls, cat, rg, sed, head, and tail as host JS views
over wit_snapshot.wasm (open / list / read). Fixture repo: demo/repo`;

const ALLOWED = new Set(["tree", "ls", "cat", "rg", "sed", "head", "tail"]);
const NOT_AVAILABLE = new Set(["skill", "mcp", "cache", "branches", "search"]);

const RG_BOOL = {
  "-i": "ignoreCase",
  "--ignore-case": "ignoreCase",
  "-l": "filesWithMatches",
  "--files-with-matches": "filesWithMatches",
};
const RG_VALUE = { "-r": "repo", "--repo": "repo" };

const SED_BOOL = {
  "-n": "quiet",
  "--quiet": "quiet",
  "--silent": "quiet",
};
const SED_VALUE = { "-r": "repo", "--repo": "repo" };

const HEAD_BOOL = { "-N": "number", "--number": "number" };
const HEAD_VALUE = {
  "-n": "lines",
  "--lines": "lines",
  "-r": "repo",
  "--repo": "repo",
};

const TAIL_BOOL = { "-N": "number", "--number": "number" };
const TAIL_VALUE = {
  "-n": "lines",
  "--lines": "lines",
  "-p": "fromLine",
  "--plus": "fromLine",
  "-r": "repo",
  "--repo": "repo",
};

/**
 * @param {string} line
 * @returns {{
 *   kind: 'empty' | 'help' | 'clear' | 'run' | 'error',
 *   command?: string,
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
      message: `bad command: expected 'wit tree|ls|cat|rg|sed|head|tail ...', got ${JSON.stringify(line.trim())}`,
    };
  }

  const rest = tokens.slice(1);
  if (rest.length === 0) {
    return { kind: "error", message: USAGE };
  }

  let repoFlag = null;
  let command = null;
  const after = [];

  for (let i = 0; i < rest.length; i += 1) {
    const token = rest[i];
    if (token === "--") {
      after.push(...rest.slice(i + 1));
      break;
    }
    if (token === "-r" || token === "--repo") {
      const value = rest[i + 1];
      if (!value || value.startsWith("-")) {
        return {
          kind: "error",
          message:
            "missing repository: pass owner/repo as a positional argument or with -r/--repo",
        };
      }
      repoFlag = value;
      i += 1;
      continue;
    }
    if (!command) {
      if (token.startsWith("-")) {
        return {
          kind: "error",
          message: unknownFlagMessage(token, null),
        };
      }
      command = token;
      continue;
    }
    after.push(token);
  }

  if (!command) {
    return { kind: "error", message: USAGE };
  }
  if (NOT_AVAILABLE.has(command) || !ALLOWED.has(command)) {
    return {
      kind: "error",
      message: `bad command: '${command}' is not available here (only tree, ls, cat, rg, sed, head, tail)`,
    };
  }

  try {
    if (command === "rg") {
      return parseRg(repoFlag, after);
    }
    if (command === "sed") {
      return parseSed(repoFlag, after);
    }
    if (command === "head") {
      return parseHead(repoFlag, after);
    }
    if (command === "tail") {
      return parseTail(repoFlag, after);
    }
    const parsed = parseFlagArgs(after, {}, { "-r": "repo", "--repo": "repo" });
    const resolvedRepo = parsed.flags.repo ?? repoFlag;
    const resolved =
      command === "cat"
        ? resolveRepoAndRequiredPath(resolvedRepo, parsed.args)
        : resolveRepoAndOptionalPath(resolvedRepo, parsed.args);
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

function parseRg(repoFlag, tokens) {
  const parsed = parseFlagArgs(tokens, RG_BOOL, RG_VALUE);
  const repo = parsed.flags.repo ?? repoFlag;
  if (parsed.args.length === 0) {
    throw new Error("missing arguments: expected PATTERN owner/repo [path]");
  }
  const pattern = parsed.args[0];
  const resolved = resolveRepoAndOptionalPath(repo, parsed.args.slice(1));
  return {
    kind: "run",
    command: "rg",
    repo: resolved.repo,
    path: resolved.path,
    pattern,
    ignoreCase: Boolean(parsed.flags.ignoreCase),
    filesWithMatches: Boolean(parsed.flags.filesWithMatches),
  };
}

function parseSed(repoFlag, tokens) {
  const parsed = parseFlagArgs(tokens, SED_BOOL, SED_VALUE);
  const repo = parsed.flags.repo ?? repoFlag;
  if (!repo) {
    if (parsed.args.length < 3) {
      throw new Error(
        "missing arguments: expected SCRIPT owner/repo PATH (or -r owner/repo SCRIPT PATH)",
      );
    }
    if (parsed.args.length > 3) {
      throw new Error("too many arguments: expected SCRIPT owner/repo PATH");
    }
    return {
      kind: "run",
      command: "sed",
      repo: parsed.args[1],
      path: parsed.args[2],
      script: parsed.args[0],
      quiet: Boolean(parsed.flags.quiet),
    };
  }
  if (parsed.args.length === 2) {
    return {
      kind: "run",
      command: "sed",
      repo,
      path: parsed.args[1],
      script: parsed.args[0],
      quiet: Boolean(parsed.flags.quiet),
    };
  }
  if (parsed.args.length === 3) {
    if (parsed.args[0] !== repo) {
      throw new Error(
        `conflicting repository arguments: -r/--repo '${repo}' vs positional '${parsed.args[0]}'`,
      );
    }
    return {
      kind: "run",
      command: "sed",
      repo,
      path: parsed.args[2],
      script: parsed.args[1],
      quiet: Boolean(parsed.flags.quiet),
    };
  }
  throw new Error("missing arguments: expected SCRIPT owner/repo PATH");
}

function parseHead(repoFlag, tokens) {
  const parsed = parseFlagArgs(tokens, HEAD_BOOL, HEAD_VALUE);
  const repo = parsed.flags.repo ?? repoFlag;
  const resolved = resolveRepoAndRequiredPath(repo, parsed.args);
  return {
    kind: "run",
    command: "head",
    repo: resolved.repo,
    path: resolved.path,
    lines: parseCount(parsed.flags.lines, 10, "-n"),
    number: Boolean(parsed.flags.number),
  };
}

function parseTail(repoFlag, tokens) {
  const parsed = parseFlagArgs(tokens, TAIL_BOOL, TAIL_VALUE);
  const repo = parsed.flags.repo ?? repoFlag;
  const resolved = resolveRepoAndRequiredPath(repo, parsed.args);
  return {
    kind: "run",
    command: "tail",
    repo: resolved.repo,
    path: resolved.path,
    lines: parseCount(parsed.flags.lines, 10, "-n"),
    number: Boolean(parsed.flags.number),
    fromLine:
      parsed.flags.fromLine == null
        ? null
        : parseCount(parsed.flags.fromLine, null, "-p"),
  };
}

function parseFlagArgs(tokens, boolFlags, valueFlags) {
  const flags = {};
  const args = [];
  for (let i = 0; i < tokens.length; i += 1) {
    const token = tokens[i];
    if (token === "--") {
      args.push(...tokens.slice(i + 1));
      break;
    }
    if (boolFlags[token]) {
      flags[boolFlags[token]] = true;
      continue;
    }
    if (valueFlags[token]) {
      const value = tokens[i + 1];
      if (value == null || value.startsWith("-")) {
        throw new Error(`missing value for ${token}`);
      }
      flags[valueFlags[token]] = value;
      i += 1;
      continue;
    }
    if (token.startsWith("-")) {
      throw new Error(unknownFlagMessage(token, currentCommandFromFlags(boolFlags, valueFlags)));
    }
    args.push(token);
  }
  return { flags, args };
}

function currentCommandFromFlags(boolFlags, valueFlags) {
  if (boolFlags === RG_BOOL) {
    return "rg";
  }
  if (boolFlags === SED_BOOL) {
    return "sed";
  }
  if (boolFlags === HEAD_BOOL) {
    return "head";
  }
  if (boolFlags === TAIL_BOOL) {
    return "tail";
  }
  return "tree|ls|cat";
}

function unknownFlagMessage(token, command) {
  if (command === "rg") {
    return `unknown flag ${token} (this page accepts -i/-l for rg)`;
  }
  if (command === "sed") {
    return `unknown flag ${token} (this page accepts -n for sed)`;
  }
  if (command === "head") {
    return `unknown flag ${token} (this page accepts -n/-N for head)`;
  }
  if (command === "tail") {
    return `unknown flag ${token} (this page accepts -n/-p/-N for tail)`;
  }
  return `unknown flag ${token} (this page accepts tree|ls|cat|rg|sed|head|tail; rg -i/-l, sed -n, head -n/-N, tail -n/-p/-N)`;
}

function parseCount(value, fallback, flag) {
  if (value == null || value === "") {
    return fallback;
  }
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < 0) {
    throw new Error(`invalid ${flag} value: expected a non-negative integer`);
  }
  return parsed;
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
