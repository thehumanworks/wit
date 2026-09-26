#!/usr/bin/env bash
# Guard for the Lean proofs in formal/ (ADR 0010):
#   1. formal/Wit/Generated/Constants.lean matches what
#      scripts/gen_formal_constants.mjs extracts from the Worker, wrangler.toml,
#      the deploy workflow, and the Rust client, so the proofs are about the
#      committed values (`--write` regenerates it instead of failing);
#   2. no proof escape hatch appears in the Lean sources;
#   3. `lake build` succeeds (installs elan and the pinned toolchain if needed);
#   4. every theorem depends on no axioms beyond Lean's standard three.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

formal="formal"
generated="$formal/Wit/Generated/Constants.lean"

fail() {
  echo "error: $1" >&2
  exit 1
}

if [ "${1:-}" = "--write" ]; then
  node scripts/gen_formal_constants.mjs --write
else
  expected="$(mktemp)"
  trap 'rm -f "$expected"' EXIT
  node scripts/gen_formal_constants.mjs >"$expected"
  if ! cmp -s "$expected" "$generated"; then
    diff -u "$generated" "$expected" >&2 || true
    fail "$generated is stale: the sources above changed a modeled value. Run 'bash scripts/check_formal.sh --write', rebuild, and fix any proof that no longer holds"
  fi
fi

# Comments may mention these words; code may not.
forbidden='\b(sorry|admit|axiom|native_decide|implemented_by|extern|unsafe|opaque|skipKernelTC)\b'
while IFS= read -r file; do
  hits="$(perl -0777 -pe 's{/-.*?-/}{}gs; s{--[^\n]*}{}g' "$file" | grep -nE "$forbidden" || true)"
  [ -z "$hits" ] || fail "$file uses a forbidden construct:
$hits"
done < <(find "$formal" -name '*.lean' -not -path '*/.lake/*' | sort)

export PATH="$HOME/.elan/bin:$PATH"
if ! command -v lake >/dev/null 2>&1; then
  curl -sSfL https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh |
    sh -s -- -y --default-toolchain none --no-modify-path
fi

(cd "$formal" && lake build)

# `#print axioms` for every theorem; `sorryAx` or any declared axiom shows up here.
probe="$formal/.lake/AxiomProbe.lean"
mkdir -p "$formal/.lake"
{
  echo "import Wit"
  for file in $(find "$formal/Wit" -name '*.lean' | sort); do
    awk '
      /^namespace / { ns = $2 }
      /^end / && $2 == ns { ns = "" }
      {
        for (i = 1; i < NF; i++) if ($i == "theorem") { print "#print axioms " ns "." $(i + 1); break }
      }
    ' "$file"
  done
} >"$probe"
count="$(grep -c '^#print axioms' "$probe")"
[ "$count" -gt 0 ] || fail "found no theorems to check"
report="$(cd "$formal" && lake env lean .lake/AxiomProbe.lean 2>&1)" || fail "axiom probe failed:
$report"
bad="$(printf '%s\n' "$report" | grep -E 'depends on axioms' |
  sed -E 's/.*depends on axioms: \[(.*)\]/\1/' | tr ',' '\n' | sed 's/^ *//' |
  grep -vxE 'propext|Classical\.choice|Quot\.sound' || true)"
[ -z "$bad" ] || fail "theorems depend on non-standard axioms: $(echo "$bad" | sort -u | tr '\n' ' ')"
if printf '%s\n' "$report" | grep -qE 'error|unknown'; then
  fail "axiom probe reported errors:
$report"
fi
echo "formal proofs checked: $count theorems, standard axioms only, constants match the sources"
