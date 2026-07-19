/// <reference path="../../codemode.wit.d.ts" />

async function inspectRepository(): Promise<string> {
  const repositories = await codemode.wit.findRepositories({
    pattern: "wit",
    lang: "Rust",
    max_items: 5,
  });

  const refs = await codemode.wit.refs({
    repo: "thehumanworks/wit",
    max_items: 20,
  });

  const opened = await codemode.wit.open({
    repo: "thehumanworks/wit",
    freshness: "allow_stale",
  });

  const listing = await codemode.wit.list({
    snapshot_id: opened.snapshot_id,
    path: "crates/wit/src",
    depth: 2,
    max_bytes: 16_384,
  });

  const search = await codemode.wit.searchCode({
    snapshot_id: opened.snapshot_id,
    queries: ["OperationDescriptor"],
    globs: ["**/*.rs"],
  });

  const firstPath: string | undefined = listing.items[0]?.path;
  const firstLine: number | undefined = search.items[0]?.match_line;

  const read = await codemode.wit.read({
    snapshot_id: opened.snapshot_id,
    path: firstPath ?? "README.md",
    start_line: firstLine ?? 1,
    max_lines: 20,
  });

  const context = await codemode.wit.context({
    snapshot_id: opened.snapshot_id,
    queries: ["OperationDescriptor", "dispatch"],
    max_results: 10,
  });

  const repositoryName: string | undefined = repositories.items[0]?.full_name;
  const resolvedRef: string | undefined = refs.items[0]?.resolved_ref;
  const readText: string | undefined = read.items[0]?.text;
  const contextScore: number | undefined = context.items[0]?.score;
  return `${repositoryName ?? "none"}:${resolvedRef ?? "none"}:${firstPath ?? "none"}:${firstLine ?? 0}:${readText?.length ?? 0}:${contextScore ?? 0}`;
}

void inspectRepository();
