#!/usr/bin/env sh
set -eu

PROGRAM="wit"
DEFAULT_REPO="thehumanworks/wit"
REPO="${WIT_INSTALL_REPO:-$DEFAULT_REPO}"
VERSION="${WIT_VERSION:-latest}"
BIN_DIR="${WIT_BIN_DIR:-}"
VERIFY_CHECKSUM=1

usage() {
    cat <<EOF
Install ${PROGRAM} from GitHub releases.

Usage:
  sh install.sh [options]

Options:
  -v, --version <tag>     Install a specific tag (default: latest)
  -r, --repo <owner/repo> GitHub repository to install from (default: ${DEFAULT_REPO})
  -b, --bin-dir <path>    Installation directory (default: /usr/local/bin if writable, else ~/.local/bin)
      --no-verify         Skip SHA256 verification
  -h, --help              Show this help message

Environment:
  WIT_VERSION       Same as --version
  WIT_INSTALL_REPO  Same as --repo
  WIT_BIN_DIR       Same as --bin-dir
EOF
}

log() {
    printf "%s\n" "$*"
}

warn() {
    printf "warning: %s\n" "$*" >&2
}

fail() {
    printf "error: %s\n" "$*" >&2
    exit 1
}

has_cmd() {
    command -v "$1" >/dev/null 2>&1
}

download() {
    url="$1"
    out="$2"

    if has_cmd curl; then
        curl -fsSL "$url" -o "$out"
        return 0
    fi

    if has_cmd wget; then
        wget -q "$url" -O "$out"
        return 0
    fi

    fail "curl or wget is required to download release artifacts"
}

resolve_latest_tag() {
    api_url="https://api.github.com/repos/${REPO}/releases/latest"

    if has_cmd curl; then
        json="$(curl -fsSL "$api_url")"
    elif has_cmd wget; then
        json="$(wget -qO- "$api_url")"
    else
        fail "curl or wget is required to resolve the latest release tag"
    fi

    tag="$(printf "%s\n" "$json" | sed -n 's/^[[:space:]]*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
    [ -n "$tag" ] || fail "unable to resolve latest tag from ${api_url}"
    printf "%s" "$tag"
}

while [ $# -gt 0 ]; do
    case "$1" in
        -v|--version)
            [ $# -ge 2 ] || fail "missing value for $1"
            VERSION="$2"
            shift 2
            ;;
        -r|--repo)
            [ $# -ge 2 ] || fail "missing value for $1"
            REPO="$2"
            shift 2
            ;;
        -b|--bin-dir)
            [ $# -ge 2 ] || fail "missing value for $1"
            BIN_DIR="$2"
            shift 2
            ;;
        --no-verify)
            VERIFY_CHECKSUM=0
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            fail "unknown option: $1"
            ;;
    esac
done

if [ "$VERSION" = "latest" ]; then
    log "Resolving latest release from ${REPO}..."
    VERSION="$(resolve_latest_tag)"
fi

uname_s="$(uname -s 2>/dev/null | tr '[:upper:]' '[:lower:]')"
case "$uname_s" in
    linux*)
        os_part="unknown-linux-musl"
        archive_ext="tar.gz"
        binary_name="${PROGRAM}"
        ;;
    darwin*)
        os_part="apple-darwin"
        archive_ext="tar.gz"
        binary_name="${PROGRAM}"
        ;;
    msys*|mingw*|cygwin*)
        os_part="pc-windows-msvc"
        archive_ext="zip"
        binary_name="${PROGRAM}.exe"
        ;;
    *)
        fail "unsupported operating system: ${uname_s}"
        ;;
esac

uname_m="$(uname -m 2>/dev/null)"
case "$uname_m" in
    x86_64|amd64)
        arch_part="x86_64"
        ;;
    aarch64|arm64)
        arch_part="aarch64"
        ;;
    *)
        fail "unsupported architecture: ${uname_m}"
        ;;
esac

target="${arch_part}-${os_part}"
asset="${PROGRAM}-${target}.${archive_ext}"
checksums_file="${PROGRAM}-checksums.txt"
download_base="https://github.com/${REPO}/releases/download/${VERSION}"
asset_url="${download_base}/${asset}"
checksums_url="${download_base}/${checksums_file}"

tmp_dir="$(mktemp -d 2>/dev/null || mktemp -d -t "${PROGRAM}-install")"
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT INT TERM

archive_path="${tmp_dir}/${asset}"

log "Downloading ${asset}..."
if ! download "$asset_url" "$archive_path"; then
    if [ "$target" = "aarch64-pc-windows-msvc" ]; then
        warn "No Windows arm64 artifact found. Falling back to x86_64."
        target="x86_64-pc-windows-msvc"
        asset="${PROGRAM}-${target}.zip"
        archive_ext="zip"
        archive_path="${tmp_dir}/${asset}"
        asset_url="${download_base}/${asset}"
        download "$asset_url" "$archive_path" || fail "unable to download fallback asset: ${asset_url}"
    else
        fail "unable to download release artifact: ${asset_url}"
    fi
fi

if [ "$VERIFY_CHECKSUM" -eq 1 ]; then
    checksum_path="${tmp_dir}/${checksums_file}"
    if download "$checksums_url" "$checksum_path"; then
        expected_checksum="$(awk -v file="$asset" '$2 == file { print $1; exit }' "$checksum_path")"
        if [ -n "$expected_checksum" ]; then
            if has_cmd sha256sum; then
                actual_checksum="$(sha256sum "$archive_path" | awk '{print $1}')"
            elif has_cmd shasum; then
                actual_checksum="$(shasum -a 256 "$archive_path" | awk '{print $1}')"
            elif has_cmd openssl; then
                actual_checksum="$(openssl dgst -sha256 "$archive_path" | awk '{print $NF}')"
            else
                warn "sha256 tool not found; skipping checksum verification"
                actual_checksum=""
            fi

            if [ -n "$actual_checksum" ] && [ "$actual_checksum" != "$expected_checksum" ]; then
                fail "checksum verification failed for ${asset}"
            fi
        else
            warn "checksum entry missing for ${asset}; skipping verification"
        fi
    else
        warn "checksum manifest not available; skipping verification"
    fi
fi

extract_dir="${tmp_dir}/extract"
mkdir -p "$extract_dir"

if [ "$archive_ext" = "tar.gz" ]; then
    has_cmd tar || fail "tar is required to extract ${asset}"
    tar -xzf "$archive_path" -C "$extract_dir"
else
    has_cmd unzip || fail "unzip is required to extract ${asset}"
    unzip -oq "$archive_path" -d "$extract_dir"
fi

binary_path="${extract_dir}/${binary_name}"
if [ ! -f "$binary_path" ]; then
    binary_path="$(find "$extract_dir" -type f -name "$binary_name" | head -n 1 || true)"
fi
[ -n "$binary_path" ] && [ -f "$binary_path" ] || fail "binary ${binary_name} not found in ${asset}"

if [ -z "$BIN_DIR" ]; then
    if [ -d "/usr/local/bin" ] && [ -w "/usr/local/bin" ]; then
        BIN_DIR="/usr/local/bin"
    else
        BIN_DIR="${HOME}/.local/bin"
    fi
fi

mkdir -p "$BIN_DIR"
destination="${BIN_DIR}/${binary_name}"

if [ -w "$BIN_DIR" ]; then
    cp "$binary_path" "$destination"
    chmod +x "$destination" 2>/dev/null || true
else
    if has_cmd sudo; then
        sudo cp "$binary_path" "$destination"
        sudo chmod +x "$destination" 2>/dev/null || true
    else
        fail "cannot write to ${BIN_DIR}; rerun with --bin-dir in a writable directory"
    fi
fi

log "Installed ${PROGRAM} ${VERSION} to ${destination}"
case ":$PATH:" in
    *":${BIN_DIR}:"*) ;;
    *)
        warn "${BIN_DIR} is not in PATH; add it to use ${PROGRAM} globally"
        ;;
esac
