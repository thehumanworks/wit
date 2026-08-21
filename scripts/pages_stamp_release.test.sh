#!/usr/bin/env bash
# Unit checks for scripts/pages_stamp_release.sh (no network, no cargo).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
script="$root/scripts/pages_stamp_release.sh"

fail() {
  echo "error: $*" >&2
  exit 1
}

got="$("$script" pick-tag <<'JSON'
[
  {"tag_name":"v9.9.9","draft":false,"assets":[{"name":"wit-linux-x86_64.tar.gz"}]},
  {"tag_name":"v9.9.1","draft":false,"assets":[{"name":"wit_snapshot.wasm"},{"name":"wit-linux-x86_64.tar.gz"}]},
  {"tag_name":"v9.8.0","draft":false,"assets":[{"name":"wit_snapshot.wasm"}]}
]
JSON
)"
[[ "$got" == "v9.9.1" ]] || fail "pick-tag should skip a newer release without the wasm (got ${got})"

if "$script" pick-tag <<'JSON'
[{"tag_name":"v9.9.9","draft":false,"assets":[{"name":"wit-linux-x86_64.tar.gz"}]}]
JSON
then
  fail "pick-tag should fail when no release has wit_snapshot.wasm"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/try"
printf 'tag=__WIT_RELEASE_TAG__\nhttps://github.com/thehumanworks/wit/releases/download/__WIT_RELEASE_TAG__/wit_snapshot.wasm\n' >"$tmp/index.html"
printf 'export const RELEASE_TAG = "__WIT_RELEASE_TAG__";\n' >"$tmp/try/host.js"
"$script" stamp "$tmp" "v9.9.1"
grep -Fq 'tag=v9.9.1' "$tmp/index.html" || fail "stamp did not replace the display tag"
grep -Fq 'releases/download/v9.9.1/wit_snapshot.wasm' "$tmp/index.html" || fail "stamp did not replace the wasm URL"
grep -Fq 'export const RELEASE_TAG = "v9.9.1";' "$tmp/try/host.js" || fail "stamp did not replace host.js"
grep -Fq '"tag":"v9.9.1"' "$tmp/try/release.json" || fail "stamp did not write release.json"
if grep -Fq '__WIT_RELEASE_TAG__' "$tmp/index.html" "$tmp/try/host.js"; then
  fail "placeholder remains after a real-tag stamp"
fi

tmp_local="$(mktemp -d)"
mkdir -p "$tmp_local/try"
printf '<a href="https://github.com/thehumanworks/wit/releases/download/__WIT_RELEASE_TAG__/wit_snapshot.wasm">https://github.com/thehumanworks/wit/releases/download/__WIT_RELEASE_TAG__/wit_snapshot.wasm</a>\n' >"$tmp_local/index.html"
printf 'export const RELEASE_TAG = "__WIT_RELEASE_TAG__";\n' >"$tmp_local/try/host.js"
"$script" stamp "$tmp_local" "local"
if grep -Fq 'releases/download/' "$tmp_local/index.html"; then
  fail "local stamp must not invent a GitHub release download URL"
fi
grep -Fq 'local build (no GitHub release URL)' "$tmp_local/index.html" || fail "local stamp should drop the release URL"
grep -Fq '"tag":"local"' "$tmp_local/try/release.json" || fail "local stamp should write tag=local"

echo "pages_stamp_release tests: ok"
