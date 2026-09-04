import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { formatOutline, outlineFile, outlineLanguages, rulesForPath } from "../lib/outline.js";

const names = (out) => out.symbols.map((s) => `${s.kind} ${s.name}@${s.line}-${s.end_line}`);

describe("outlineFile", () => {
  it("indexes Rust items and nests impl methods", () => {
    const src = [
      "pub struct A;",
      "pub(crate) enum E { X }",
      "pub trait T {",
      "    fn required(&self);",
      "}",
      "impl<T: Clone> T for A where T: Send {",
      "    pub async fn go(&self) {}",
      "}",
      "macro_rules! m { () => {} }",
      "pub const LIMIT: usize = 1;",
      "static COUNTER: u8 = 0;",
      "mod tests {",
      "    #[test]",
      "    fn it_works() {}",
      "}",
    ].join("\n");
    const out = outlineFile("x.rs", src);
    assert.equal(out.language, "Rust");
    assert.deepEqual(names(out), [
      "struct A@1-1",
      "enum E@2-2",
      "trait T@3-5",
      "fn required@4-5",
      "impl T for A@6-8",
      "fn go@7-8",
      "macro m@9-9",
      "const LIMIT@10-10",
      "const COUNTER@11-11",
      "mod tests@12-15",
      "fn it_works@14-15",
    ]);
  });

  it("indexes TypeScript declarations, arrow functions, and class methods", () => {
    const src = [
      "import x from 'y';",
      "export interface Shape { area(): number }",
      "export type Pair<T> = [T, T];",
      "export const add = (a: number, b: number) => a + b;",
      "const mul = async (a) => a;",
      "export default class Circle implements Shape {",
      "  private r = 1;",
      "  constructor(r: number) {",
      "    this.r = r;",
      "  }",
      "  area(): number {",
      "    return 3;",
      "  }",
      "  static unit() {",
      "    return new Circle(1);",
      "  }",
      "}",
      "export function helper() {}",
      "enum Color { Red }",
    ].join("\n");
    const out = outlineFile("shape.ts", src);
    assert.deepEqual(names(out), [
      "interface Shape@2-2",
      "type Pair@3-3",
      "const add@4-4",
      "const mul@5-5",
      "class Circle@6-17",
      "method constructor@8-10",
      "method area@11-13",
      "method unit@14-17",
      "function helper@18-18",
      "enum Color@19-19",
    ]);
  });

  it("indexes Go, Java, C, Ruby, PHP, shell, Elixir, YAML, TOML, SQL", () => {
    assert.deepEqual(
      names(outlineFile("a.go", "package a\n\ntype S struct{}\n\nfunc (s *S) M() {}\n\nfunc F() {}\nvar V = 1\n")),
      ["type S@3-4", "func M@5-6", "func F@7-7", "var V@8-8"],
    );
    assert.deepEqual(
      names(outlineFile("A.java", "public class A {\n    private int x;\n    public static void main(String[] args) {\n    }\n    A(int x) {}\n}\n")),
      ["class A@1-6", "method main@3-4", "constructor A@5-6"],
    );
    assert.deepEqual(
      names(outlineFile("a.c", "#define MAX 10\ntypedef struct node node_t;\nstruct node { int v; };\nstatic int add(int a, int b) {\n    return a + b;\n}\nint main(void) {\n  if (x) {\n  }\n}\n")),
      ["define MAX@1-1", "typedef node_t@2-2", "struct node@3-3", "function add@4-6", "function main@7-10"],
    );
    assert.deepEqual(
      names(outlineFile("a.rb", "module M\n  class K\n    def self.build; end\n    def go?\n    end\n  end\nend\n")),
      ["class M@1-7", "class K@2-7", "def build@3-3", "def go?@4-7"],
    );
    assert.deepEqual(
      names(outlineFile("a.php", "<?php\nfinal class A {\n    public static function make() {}\n}\nfunction helper() {}\n")),
      ["class A@2-4", "function make@3-4", "function helper@5-5"],
    );
    assert.deepEqual(
      names(outlineFile("a.sh", "#!/bin/sh\nusage() {\n  echo hi\n}\nfunction deploy {\n}\n")),
      ["function usage@2-4", "function deploy@5-6"],
    );
    assert.deepEqual(
      names(outlineFile("a.ex", "defmodule A.B do\n  def go(x), do: x\n  defp hidden, do: 1\nend\n")),
      ["module A.B@1-4", "def go@2-2", "def hidden@3-4"],
    );
    assert.deepEqual(
      names(outlineFile("ci.yml", "name: ci\non:\n  push:\njobs:\n  build:\n    runs-on: x\n")),
      ["key name@1-1", "key on@2-3", "key jobs@4-6"],
    );
    assert.deepEqual(
      names(outlineFile("Cargo.toml", "[package]\nname = 'x'\n\n[[bin]]\nname = 'y'\n")),
      ["section package@1-3", "section bin@4-5"],
    );
    assert.deepEqual(
      names(outlineFile("s.sql", "CREATE TABLE users (id int);\ncreate or replace view v as select 1;\n")),
      ["table users@1-1", "table v@2-2"],
    );
  });

  it("indexes markdown headings by level and skips fenced code", () => {
    const out = outlineFile("README.md", "# Top\n\n```md\n# not a heading\n```\n\n## A\ntext\n### A.1\n## B\n");
    assert.deepEqual(names(out), ["heading Top@1-10", "heading A@7-9", "heading A.1@9-9", "heading B@10-10"]);
  });

  it("reports unsupported files and caps symbols", () => {
    const none = outlineFile("data.csv", "a,b\n1,2\n");
    assert.equal(none.supported, false);
    assert.equal(rulesForPath("Dockerfile"), null);
    assert.match(formatOutline(none, "data.csv"), /no rules for data\.csv/);
    const many = outlineFile("m.py", Array.from({ length: 50 }, (_, i) => `def f${i}(): pass`).join("\n"));
    const capped = outlineFile("m.py", Array.from({ length: 50 }, (_, i) => `def f${i}(): pass`).join("\n"), { maxSymbols: 5 });
    assert.equal(many.symbols.length, 50);
    assert.equal(capped.symbols.length, 5);
    assert.equal(capped.truncated, true);
    assert.match(formatOutline(capped, "m.py"), /symbol limit reached/);
    const empty = outlineFile("e.py", "# nothing here\n");
    assert.match(formatOutline(empty, "e.py"), /no symbols found in e\.py \(1 lines\)/);
  });

  it("advertises a stable language list", () => {
    const langs = outlineLanguages();
    for (const expected of ["Rust", "Python", "TypeScript", "Go", "Markdown"]) {
      assert.ok(langs.includes(expected), expected);
    }
    assert.deepEqual(langs, [...langs].sort());
  });
});
