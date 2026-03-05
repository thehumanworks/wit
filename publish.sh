#!/usr/bin/env bash
set -euo pipefail

TAG="${1:-}"

if [ -z "${TAG}" ]; then
  echo "usage: ./publish.sh vX.Y.Z" >&2
  exit 1
fi

if [ -z "${NPM_AUTH_TOKEN:-}" ]; then
  echo "error: NPM_AUTH_TOKEN is required" >&2
  exit 1
fi

VERSION="$(node scripts/npm/assert-release-version.mjs --tag "${TAG}")"
WORK_DIR="$(mktemp -d)"

cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT INT TERM

gh release download "${TAG}" \
  --repo thehumanworks/wit \
  --pattern 'wit-*' \
  --dir "${WORK_DIR}/dist"

node scripts/npm/build-packages.mjs \
  --version "${VERSION}" \
  --artifacts-dir "${WORK_DIR}/dist" \
  --output-dir "${WORK_DIR}/dist/npm"

(cd "${WORK_DIR}/dist/npm/package" && npm pack >/dev/null)
NODE_AUTH_TOKEN="${NPM_AUTH_TOKEN}" npm publish "${WORK_DIR}/dist/npm/package" --access public
