#!/usr/bin/env bash
set -euo pipefail

release_workflow=".github/workflows/release.yml"
ci_workflow=".github/workflows/ci.yml"
npm_workflow=".github/workflows/publish-npm.yml"
npm_targets="npm/targets.json"
npm_builder="scripts/npm/build-packages.mjs"
mcp_smoke="scripts/smoke_mcp_modes.mjs"

fail() {
  echo "error: $1" >&2
  exit 1
}

assert_contains() {
  local file="$1"
  local needle="$2"
  local message="$3"

  grep -Fq "$needle" "$file" || fail "$message"
}

assert_not_contains() {
  local file="$1"
  local needle="$2"
  local message="$3"

  if grep -Fq "$needle" "$file"; then
    fail "$message"
  fi
}

assert_contains "$release_workflow" 'uses: ./.github/workflows/publish-npm.yml' \
  "release workflow must call the reusable npm publish workflow"
assert_contains "$release_workflow" 'release_tag: ${{ github.ref_name }}' \
  "release workflow must pass the pushed tag to npm publish"
assert_not_contains "$release_workflow" 'gh workflow run publish-npm.yml' \
  "release workflow must not dispatch npm publish via gh CLI"
assert_not_contains "$release_workflow" 'continue-on-error: ${{ matrix.experimental == true }}' \
  "release workflow must not mark advertised release artifacts as optional"
assert_not_contains "$release_workflow" 'experimental: true' \
  "release workflow must not mark advertised release artifacts as optional"
assert_contains "$release_workflow" 'cross build --locked --release --target "${{ matrix.target }}" -p wit --bins' \
  "cross release builds must build only wit's two binary targets"
assert_contains "$release_workflow" 'cargo build --locked --release --target "${{ matrix.target }}" -p wit --bins' \
  "cargo release builds must build only wit's two binary targets"

assert_contains "$npm_workflow" 'workflow_call:' \
  "publish-npm workflow must be reusable via workflow_call"
assert_contains "$npm_workflow" 'workflow_dispatch:' \
  "publish-npm workflow must remain manually re-runnable"
assert_not_contains "$npm_workflow" 'release:' \
  "publish-npm workflow must not auto-trigger from release events"

assert_contains "$release_workflow" 'runner: ubuntu-latest' \
  "release workflow must build Linux artifacts on an Ubuntu runner"
assert_contains "$release_workflow" 'runner: macos-14' \
  "release workflow must build macOS artifacts on a macOS runner"
assert_contains "$release_workflow" 'runner: windows-latest' \
  "release workflow must build Windows artifacts on a Windows runner"
assert_contains "$release_workflow" 'mcp_bin: wit-mcp' \
  "release workflow must package the Unix wit-mcp binary"
assert_contains "$release_workflow" 'mcp_bin: wit-mcp.exe' \
  "release workflow must package the Windows wit-mcp binary"
assert_contains "$npm_targets" '"mcpBinaryName": "wit-mcp"' \
  "npm targets must expose the wit-mcp launcher"
assert_contains "$npm_targets" '"mcpBinaryFile": "wit-mcp"' \
  "npm targets must install the Unix wit-mcp binary"
assert_contains "$npm_targets" '"mcpBinaryFile": "wit-mcp.exe"' \
  "npm targets must install the Windows wit-mcp binary"
assert_contains "$npm_workflow" 'wit-mcp --version' \
  "npm smoke tests must exercise the wit-mcp launcher"
assert_contains "$release_workflow" 'node scripts/smoke_mcp_modes.mjs' \
  "release smoke tests must inspect direct and Code Mode tools/list surfaces"
assert_contains "$npm_workflow" 'node scripts/smoke_mcp_modes.mjs wit-mcp' \
  "npm smoke tests must inspect direct and Code Mode tools/list surfaces"
assert_contains "$mcp_smoke" '["code"]' \
  "MCP smoke test must assert the one-tool Code Mode surface"
assert_contains "$mcp_smoke" '"wit_open"' \
  "MCP smoke test must assert the direct tool surface"
assert_contains "$npm_builder" 'must contain exactly the two configured binaries' \
  "npm package validation must reject release archives containing a third binary"
assert_contains "$npm_builder" 'archiveFileEntries' \
  "npm package validation must inspect full normalized archive entries"
assert_contains "$npm_builder" 'entries must exactly match' \
  "npm package validation must reject nested, duplicate, or extra entries"
assert_contains "$release_workflow" 'output-file: dist/wit-sbom.spdx.json' \
  "release workflow must generate the Code Mode dependency SBOM"
assert_contains "$release_workflow" 'THIRD-PARTY-LICENSES.md' \
  "release workflow must attach Code Mode dependency notices"
assert_contains "$npm_builder" 'wit-sbom.spdx.json' \
  "npm package must include the release SBOM"
assert_contains "$npm_builder" 'THIRD-PARTY-LICENSES.md' \
  "npm package must include Code Mode dependency notices"

for target in \
  x86_64-unknown-linux-musl \
  aarch64-unknown-linux-musl \
  x86_64-apple-darwin \
  aarch64-apple-darwin \
  x86_64-pc-windows-msvc \
  aarch64-pc-windows-msvc
do
  assert_contains "$release_workflow" "target: $target" \
    "release workflow omits mandatory target $target"
  assert_contains "$ci_workflow" "target: $target" \
    "pull-request CI omits mandatory package target $target"
done

assert_contains "$ci_workflow" 'cross build --locked --release --target "${{ matrix.target }}" -p wit --bins' \
  "pull-request cross-target gate must compile both public binaries"
assert_contains "$ci_workflow" 'cargo build --locked --release --target "${{ matrix.target }}" -p wit --bins' \
  "pull-request native-target gate must compile both public binaries"
assert_contains "$ci_workflow" 'tar -C "target/${{ matrix.target }}/release" -czf "$artifact" "${{ matrix.bin }}" "${{ matrix.mcp_bin }}"' \
  "pull-request Unix package gate must include wit and wit-mcp"
assert_contains "$ci_workflow" 'Compress-Archive -Path $files -DestinationPath $artifact -Force' \
  "pull-request Windows package gate must include wit.exe and wit-mcp.exe"

target_count="$(grep -c '"id":' "$npm_targets")"
[ "$target_count" -eq 6 ] || fail "npm targets must contain exactly six release targets"
