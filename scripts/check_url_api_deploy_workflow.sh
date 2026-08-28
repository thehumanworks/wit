#!/usr/bin/env bash
# Guard for the ADR 0005 URL API host: showcase/url-api must keep deploying to
# Cloudflare Pages (project wit-url-api) from main, and must never fold onto the
# GitHub Pages host in .github/workflows/pages.yml.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

deploy_workflow=".github/workflows/url-api-deploy.yml"
pages_workflow=".github/workflows/pages.yml"
wrangler_config="showcase/url-api/wrangler.toml"
showcase_readme="showcase/url-api/README.md"

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

first_line_matching() {
  grep -nF "$2" "$1" | head -n 1 | cut -d: -f1
}

[ -f "$deploy_workflow" ] || fail "$deploy_workflow must exist (Cloudflare Pages deploy for showcase/url-api)"

assert_contains "$deploy_workflow" '${{ secrets.CLOUDFLARE_API_TOKEN }}' \
  "$deploy_workflow must read the API token as \${{ secrets.CLOUDFLARE_API_TOKEN }}"
assert_contains "$deploy_workflow" '${{ secrets.CLOUDFLARE_ACCOUNT_ID }}' \
  "$deploy_workflow must read the account id as \${{ secrets.CLOUDFLARE_ACCOUNT_ID }}"

# Every occurrence of a Cloudflare credential value must come from `secrets.`.
while IFS= read -r line; do
  case "$line" in
    *'${{ secrets.CLOUDFLARE_'*) ;;
    *)
      key="${line#"${line%%[![:space:]]*}"}"
      fail "$deploy_workflow must set ${key%%:*} only from \${{ secrets.* }}, never from a literal"
      ;;
  esac
done < <(grep -E '^[[:space:]]*CLOUDFLARE_(API_TOKEN|ACCOUNT_ID):' "$deploy_workflow")

# Missing secrets must fail loudly, naming both secrets, instead of skipping.
assert_contains "$deploy_workflow" 'missing repository secret(s)' \
  "$deploy_workflow must fail with a clear message when a Cloudflare secret is missing"
assert_contains "$deploy_workflow" 'Add CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID under' \
  "the missing-secret failure must name both CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID"
assert_not_contains "$deploy_workflow" 'continue-on-error: true' \
  "$deploy_workflow must not report success when the deploy fails"

# This host is the Worker, not GitHub Pages.
for pages_action in \
  actions/deploy-pages \
  actions/upload-pages-artifact \
  actions/configure-pages
do
  assert_not_contains "$deploy_workflow" "$pages_action" \
    "$deploy_workflow must not deploy the url-api Worker through GitHub Pages ($pages_action)"
done
assert_not_contains "$deploy_workflow" 'pages_stamp_release.sh' \
  "$deploy_workflow must not assemble the GitHub Pages site"

# The GitHub Pages host stays untouched by the Worker deploy.
for cloudflare_marker in wrangler CLOUDFLARE_API_TOKEN showcase/url-api; do
  assert_not_contains "$pages_workflow" "$cloudflare_marker" \
    "$pages_workflow (GitHub Pages) must not take over the Cloudflare url-api deploy ($cloudflare_marker)"
done

# Deploy only from main, plus manual dispatch; never from pull_request (no token on forks).
assert_contains "$deploy_workflow" 'branches: [main]' \
  "$deploy_workflow must deploy from main"
assert_contains "$deploy_workflow" 'workflow_dispatch:' \
  "$deploy_workflow must stay manually dispatchable"
if grep -qE '^[[:space:]]*pull_request' "$deploy_workflow"; then
  fail "$deploy_workflow must not deploy from pull_request events (forks have no Cloudflare token)"
fi

# Same wasm the showcase already uses, staged into public/ before deploying.
assert_contains "$deploy_workflow" 'cargo build -p wit-snapshot --release --target wasm32-unknown-unknown --no-default-features' \
  "$deploy_workflow must build the release wit-snapshot wasm without default features"
assert_contains "$deploy_workflow" 'showcase/url-api/public/wit_snapshot.wasm' \
  "$deploy_workflow must copy the built wasm into showcase/url-api/public/wit_snapshot.wasm"
assert_contains "$deploy_workflow" 'npm run sync-lib' \
  "$deploy_workflow must run npm run sync-lib before deploying"
assert_contains "$deploy_workflow" 'wrangler@4 pages deploy public' \
  "$deploy_workflow must deploy public/ with wrangler v4 (matches npm run dev)"

sync_line="$(first_line_matching "$deploy_workflow" 'npm run sync-lib')"
deploy_line="$(first_line_matching "$deploy_workflow" 'wrangler@4 pages deploy public')"
[ -n "$sync_line" ] && [ -n "$deploy_line" ] && [ "$sync_line" -lt "$deploy_line" ] \
  || fail "$deploy_workflow must run npm run sync-lib before wrangler pages deploy"

# Project name is single-sourced with wrangler.toml.
assert_contains "$deploy_workflow" 'PROJECT_NAME: wit-url-api' \
  "$deploy_workflow must deploy the wit-url-api project"
assert_contains "$wrangler_config" 'name = "wit-url-api"' \
  "$wrangler_config must keep the project name wit-url-api"
assert_contains "$wrangler_config" 'pages_build_output_dir = "public"' \
  "$wrangler_config must keep public/ as the Pages build output dir"
if grep -qE '^[[:space:]]*\[\[rules\]\]' "$wrangler_config"; then
  fail "$wrangler_config must not declare a rules table: wrangler pages deploy rejects \"rules\" in a Pages config"
fi

assert_contains "$showcase_readme" "$deploy_workflow" \
  "$showcase_readme must point the live deploy at $deploy_workflow"

echo "check_url_api_deploy_workflow: ok"
