#!/bin/sh
# Installs the latest `da` release on Linux or macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/LnYo-Cly/do-anything/main/install.sh | sh
#   curl -fsSL https://cdn.jsdelivr.net/gh/LnYo-Cly/do-anything@main/install.sh | sh
#
# Environment:
#   DA_VERSION       release tag to install, for example v0.2.0 (default: latest)
#   DA_INSTALL_DIR   directory that receives `da` (default: ~/.local/bin)
#   DA_DOWNLOAD_URL  base URL that holds the release files, for a mirror or proxy
#                    (default: https://github.com/LnYo-Cly/do-anything/releases/download)
set -eu

REPO="LnYo-Cly/do-anything"
INSTALL_DIR="${DA_INSTALL_DIR:-$HOME/.local/bin}"

fail() {
  printf 'da install: %s\n' "$1" >&2
  exit 1
}

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL --retry 3 --connect-timeout 15 -o "$2" "$1"; }
  final_url() { curl -fsSL --retry 3 --connect-timeout 15 -o /dev/null -w '%{url_effective}' "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q --tries=3 --timeout=15 -O "$2" "$1"; }
  final_url() { wget --tries=3 --timeout=15 --max-redirect=5 -S --spider "$1" 2>&1 | sed -n 's/^ *[Ll]ocation: *//p' | tail -n 1 | tr -d '\r'; }
else
  fail "curl or wget is required"
fi

case "$(uname -s)" in
  Linux) os="unknown-linux-musl" ;;
  Darwin) os="apple-darwin" ;;
  *) fail "unsupported system $(uname -s); on Windows use install.ps1" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  aarch64 | arm64) arch="aarch64" ;;
  *) fail "unsupported CPU $(uname -m); build from source with cargo instead" ;;
esac
# A shell running under Rosetta reports x86_64 on Apple silicon; prefer the native build.
if [ "$os" = "apple-darwin" ] && [ "$arch" = "x86_64" ] &&
  [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = "1" ]; then
  arch="aarch64"
fi
target="${arch}-${os}"

version="${DA_VERSION:-}"
if [ -z "$version" ]; then
  # The /releases/latest page redirects to /releases/tag/<tag>; this avoids the
  # rate-limited GitHub API.
  latest="$(final_url "https://github.com/${REPO}/releases/latest")" ||
    fail "could not reach github.com to find the latest release; set DA_VERSION and DA_DOWNLOAD_URL to use a mirror"
  version="${latest##*/}"
  case "$version" in
    v*) ;;
    *) fail "could not read the latest release tag from ${latest}; set DA_VERSION, for example DA_VERSION=v0.2.0" ;;
  esac
fi

base="${DA_DOWNLOAD_URL:-https://github.com/${REPO}/releases/download}"
base="${base%/}/${version}"
archive="da-${version}-${target}.tar.gz"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

printf 'Downloading da %s for %s\n' "$version" "$target"
fetch "${base}/${archive}" "${tmp}/${archive}" ||
  fail "could not download ${base}/${archive}; check that ${version} exists and has a build for ${target}"
fetch "${base}/SHA256SUMS" "${tmp}/SHA256SUMS" ||
  fail "could not download ${base}/SHA256SUMS"

expected="$(awk -v name="$archive" '$2 == name || $2 == "*" name { print $1 }' "${tmp}/SHA256SUMS")"
[ -n "$expected" ] || fail "SHA256SUMS has no entry for ${archive}"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "${tmp}/${archive}" | awk '{ print $1 }')"
elif command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "${tmp}/${archive}" | awk '{ print $1 }')"
else
  fail "sha256sum or shasum is required to verify the download"
fi
[ "$expected" = "$actual" ] ||
  fail "checksum mismatch for ${archive} (expected ${expected}, got ${actual}); nothing was installed"

tar -xzf "${tmp}/${archive}" -C "$tmp"
mkdir -p "$INSTALL_DIR"
# Copy then rename, so a running `da` is never replaced by a half-written file.
cp "${tmp}/da-${version}-${target}/da" "${INSTALL_DIR}/.da.new"
chmod 755 "${INSTALL_DIR}/.da.new"
mv -f "${INSTALL_DIR}/.da.new" "${INSTALL_DIR}/da"

printf 'Installed %s to %s\n' "$("${INSTALL_DIR}/da" --version)" "${INSTALL_DIR}/da"
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *) printf '%s is not on PATH. Add this line to your shell profile:\n  export PATH="%s:%s"\n' "$INSTALL_DIR" "$INSTALL_DIR" "\$PATH" ;;
esac
