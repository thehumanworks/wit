#!/usr/bin/env bash
set -euo pipefail

release_workflow=".github/workflows/release.yml"
npm_workflow=".github/workflows/publish-npm.yml"
npm_targets="npm/targets.json"

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
