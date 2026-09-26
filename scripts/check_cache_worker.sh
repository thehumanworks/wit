#!/usr/bin/env bash
# Guard for the ADR 0009 shared pack cache: the wit-cache Worker tests pass,
# its deploy workflow reads Cloudflare credentials only from secrets, the
# wrangler config holds no credentials, and the CLI's hosted default points
# at the deployed Worker name.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

worker_dir="services/wit-cache"
wrangler_config="$worker_dir/wrangler.toml"
deploy_workflow=".github/workflows/cache-worker-deploy.yml"
cloud_rs="crates/wit/src/gitops/cloud.rs"

fail() {
  echo "error: $1" >&2
  exit 1
}

[ -f "$wrangler_config" ] || fail "$wrangler_config must exist"
[ -f "$deploy_workflow" ] || fail "$deploy_workflow must exist"

grep -Eq '^name = "wit-cache"$' "$wrangler_config" || fail "$wrangler_config must deploy the Worker named wit-cache"
if grep -Eiq '^[[:space:]]*(ADMIN_KEY|GITHUB_TOKEN|CLOUDFLARE_[A-Z_]+)[[:space:]]*=' "$wrangler_config"; then
  fail "$wrangler_config must not define credentials; use wrangler secret put"
fi

while IFS= read -r line; do
  case "$line" in
    *'${{ secrets.CLOUDFLARE_'*) ;;
    *) fail "$deploy_workflow must set Cloudflare credentials only from \${{ secrets.* }}: $line" ;;
  esac
done < <(grep -E '^[[:space:]]*CLOUDFLARE_(API_TOKEN|ACCOUNT_ID):' "$deploy_workflow")
if grep -Eq '^[[:space:]]*pull_request' "$deploy_workflow"; then
  fail "$deploy_workflow must not run on pull_request (forks never get deploy secrets)"
fi

hosted="$(sed -n 's/^pub const HOSTED_CACHE_URL: &str = "\(.*\)";$/\1/p' "$cloud_rs")"
case "$hosted" in
  https://wit-cache.*) ;;
  *) fail "$cloud_rs HOSTED_CACHE_URL must point at the wit-cache Worker (got '$hosted')" ;;
esac
grep -Fq "$hosted" "$deploy_workflow" || fail "$deploy_workflow must smoke-test $hosted"

(cd "$worker_dir" && npm test --silent)
echo "wit-cache worker checks passed"
