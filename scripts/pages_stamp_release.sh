#!/usr/bin/env bash
# Resolve the newest GitHub release that published wit_snapshot.wasm, copy
# that asset, and stamp __WIT_RELEASE_TAG__ into the published Pages site.
# The browser never calls GitHub Releases or the npm registry for a version.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLACEHOLDER="__WIT_RELEASE_TAG__"
ASSET_NAME="wit_snapshot.wasm"
REPO="${GITHUB_REPOSITORY:-thehumanworks/wit}"

usage() {
  cat <<'EOF'
Usage:
  pages_stamp_release.sh pick-tag          # stdin: GitHub /releases JSON array
  pages_stamp_release.sh resolve           # print newest tag that has the wasm
  pages_stamp_release.sh stamp DIR TAG     # replace placeholders; write release.json
  pages_stamp_release.sh assemble [SITE]   # full Pages assemble (default: _site)
EOF
}

pick_tag() {
  python3 - <<'PY'
import json
import sys

raw = sys.stdin.read()
if not raw.strip():
    sys.exit(1)
data = json.loads(raw)
if isinstance(data, dict):
    data = data.get("releases") or data.get("data") or []
if not isinstance(data, list):
    sys.exit(1)

asset = "wit_snapshot.wasm"
for rel in data:
    if not isinstance(rel, dict) or rel.get("draft"):
        continue
    tag = rel.get("tag_name") or rel.get("tag") or ""
    names = []
    for item in rel.get("assets") or []:
        if isinstance(item, str):
            names.append(item)
        elif isinstance(item, dict):
            names.append(item.get("name") or "")
    if tag and asset in names:
        print(tag)
        sys.exit(0)
sys.exit(1)
PY
}

# gh --paginate concatenates JSON arrays as `][` across pages.
_releases_json_from_gh() {
  gh api --paginate "repos/${REPO}/releases?per_page=100" | python3 -c '
import json, sys
raw = sys.stdin.read().replace("][", ",")
json.dump(json.loads(raw), sys.stdout)
'
}

resolve_tag() {
  if [[ -n "${PAGES_RELEASES_JSON:-}" ]]; then
    pick_tag <"$PAGES_RELEASES_JSON"
    return
  fi
  local json
  json="$(_releases_json_from_gh)" || return 1
  pick_tag <<<"$json"
}

is_semver_tag() {
  [[ "${1:-}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]
}

stamp() {
  local dir="$1"
  local tag="${2:-local}"
  local display="$tag"
  if [[ -z "$display" ]]; then
    display="local"
  fi
  mkdir -p "$dir/try"

  local files=()
  [[ -f "$dir/index.html" ]] && files+=("$dir/index.html")
  [[ -f "$dir/try/host.js" ]] && files+=("$dir/try/host.js")

  local f
  if is_semver_tag "$tag"; then
    for f in "${files[@]}"; do
      python3 - "$f" "$PLACEHOLDER" "$tag" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
path.write_text(path.read_text().replace(sys.argv[2], sys.argv[3]))
PY
    done
  else
    local fake_url="https://github.com/${REPO}/releases/download/${PLACEHOLDER}/${ASSET_NAME}"
    for f in "${files[@]}"; do
      python3 - "$f" "$fake_url" "$PLACEHOLDER" "$display" <<'PY'
import re
import sys
from pathlib import Path

path = Path(sys.argv[1])
fake_url, placeholder, display = sys.argv[2], sys.argv[3], sys.argv[4]
text = path.read_text()
text = re.sub(
    r'<a\s+href="' + re.escape(fake_url) + r'">\s*' + re.escape(fake_url) + r"\s*</a>",
    "local build (no GitHub release URL)",
    text,
)
text = text.replace(fake_url, "")
text = text.replace(placeholder, display)
path.write_text(text)
PY
    done
  fi
  printf '{"tag":"%s"}\n' "$display" >"$dir/try/release.json"
}

download_asset() {
  local tag="$1"
  local dest="$2"
  mkdir -p "$(dirname "$dest")"
  if command -v gh >/dev/null 2>&1; then
    local tmp
    tmp="$(mktemp -d)"
    if gh release download "$tag" --repo "$REPO" --pattern "$ASSET_NAME" --dir "$tmp"; then
      mv "$tmp/$ASSET_NAME" "$dest"
      rm -rf "$tmp"
      return 0
    fi
    rm -rf "$tmp"
  fi
  curl -fsSL -o "$dest" "https://github.com/${REPO}/releases/download/${tag}/${ASSET_NAME}"
}

build_local_wasm() {
  local dest="$1"
  echo "release asset missing; building the same no-reqwest module"
  (
    cd "$ROOT"
    cargo build -p wit-snapshot --release --target wasm32-unknown-unknown --no-default-features
  )
  cp "$ROOT/target/wasm32-unknown-unknown/release/wit_snapshot.wasm" "$dest"
}

assemble() {
  local site="${1:-$ROOT/_site}"
  local dest="$ROOT/docs/try/wit_snapshot.wasm"
  local tag=""

  if tag="$(resolve_tag)"; then
    echo "resolved release ${tag} with ${ASSET_NAME}"
    if download_asset "$tag" "$dest"; then
      echo "using release ${tag} ${ASSET_NAME}"
    else
      echo "download failed; falling back to local build"
      tag=""
      build_local_wasm "$dest"
    fi
  else
    echo "no release with ${ASSET_NAME}; falling back to local build"
    tag=""
    build_local_wasm "$dest"
  fi
  test -s "$dest"

  mkdir -p "$site/try/fixtures"
  cp "$ROOT/docs/index.html" "$ROOT/docs/.nojekyll" "$site/"
  shopt -s nullglob
  cp "$ROOT/docs/try/"*.js "$site/try/"
  local extras=("$ROOT/docs/try/"*.mjs)
  if ((${#extras[@]})); then
    cp "${extras[@]}" "$site/try/"
  fi
  shopt -u nullglob
  cp "$ROOT/docs/try/fixtures/"*.json "$site/try/fixtures/"
  cp "$dest" "$site/try/wit_snapshot.wasm"

  if [[ -n "$tag" ]]; then
    stamp "$site" "$tag"
  else
    stamp "$site" "local"
  fi
}

cmd="${1:-}"
case "$cmd" in
  pick-tag)
    pick_tag
    ;;
  resolve)
    resolve_tag
    ;;
  stamp)
    stamp "${2:?dir}" "${3:-local}"
    ;;
  assemble)
    assemble "${2:-}"
    ;;
  -h | --help | help)
    usage
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
