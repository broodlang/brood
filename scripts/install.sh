#!/bin/sh
# Brood installer. Usage:
#   curl -fsSL https://brood.fly.dev/install.sh | sh
#
# Downloads the prebuilt brood + nest + brood-lsp for your OS/arch from the latest
# GitHub release, checks it against the release's .sha256, and installs them. A Linux
# without glibc (Alpine) gets the static musl build. On Windows, use install.ps1. Override
# with env vars:
#   BROOD_VERSION=v0.1.0     install a specific release (default: latest)
#   BROOD_INSTALL_DIR=/path  install location (default: $HOME/.local/bin)
#   BROOD_TARGET=triple      the build to install (default: this machine's)
set -eu

REPO="broodlang/brood"
INSTALL_DIR="${BROOD_INSTALL_DIR:-$HOME/.local/bin}"

say()  { printf '%s\n' "brood-install: $*"; }
die()  { printf '%s\n' "brood-install: error: $*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# --- detect platform ---------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$arch" in
  x86_64|amd64)  arch_part="x86_64" ;;
  aarch64|arm64) arch_part="aarch64" ;;
  *) die "unsupported architecture '$arch'" ;;
esac
case "$os" in
  Linux)
    # glibc or musl: a musl system cannot run the glibc build. `ldd --version` names its
    # libc; with no ldd at all, a missing glibc loader says the same. Each glob is tested
    # on its own: Fedora has /lib64's loader and no /lib/<triple>/ directory.
    glibc_loader=""
    for loader in /lib*/ld-linux*.so* /lib/*/ld-linux*.so*; do
      [ -e "$loader" ] && glibc_loader="$loader"
    done
    if { ldd --version 2>&1 || true; } | grep -qi musl || [ -z "$glibc_loader" ]; then
      os_part="unknown-linux-musl"
      [ "$arch_part" = x86_64 ] || die "the static musl build is x86_64 only; on $arch, use a glibc Linux or build from source"
    else
      os_part="unknown-linux-gnu"
    fi ;;
  Darwin) os_part="apple-darwin" ;;
  MINGW*|MSYS*|CYGWIN*)
    die "on Windows, install with PowerShell: irm https://brood.fly.dev/install.ps1 | iex" ;;
  *) die "unsupported OS '$os' (Linux, macOS and Windows)" ;;
esac
target="${BROOD_TARGET:-${arch_part}-${os_part}}"

# --- downloader --------------------------------------------------------------
if have curl; then dl() { curl -fsSL "$1"; }; dlo() { curl -fsSL "$1" -o "$2"; }
elif have wget; then dl() { wget -qO- "$1"; }; dlo() { wget -qO "$2" "$1"; }
else die "need curl or wget"; fi

# --- resolve version ---------------------------------------------------------
version="${BROOD_VERSION:-}"
if [ -z "$version" ]; then
  say "resolving latest release…"
  version="$(dl "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | head -1 | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
  [ -n "$version" ] || die "could not determine the latest release (set BROOD_VERSION)"
fi

name="brood-${version}-${target}"
url="https://github.com/${REPO}/releases/download/${version}/${name}.tar.gz"

# --- download + verify -------------------------------------------------------
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
say "downloading ${name}…"
dlo "$url" "$tmp/pkg.tar.gz" || die "download failed: $url"

# The check is not optional: a sidecar that will not download, or no tool to hash with,
# stops the install rather than installing what nothing has checked.
dl "${url}.sha256" > "$tmp/pkg.sha256" 2>/dev/null && [ -s "$tmp/pkg.sha256" ] \
  || die "could not download ${url}.sha256 to check the archive against"
expected="$(cut -d' ' -f1 < "$tmp/pkg.sha256")"
if have sha256sum; then actual="$(sha256sum "$tmp/pkg.tar.gz" | cut -d' ' -f1)"
elif have shasum;   then actual="$(shasum -a 256 "$tmp/pkg.tar.gz" | cut -d' ' -f1)"
else die "need sha256sum or shasum to check the download"; fi
[ "$expected" = "$actual" ] || die "checksum mismatch (expected $expected, got $actual)"
say "checksum ok"

# --- install -----------------------------------------------------------------
tar -C "$tmp" -xzf "$tmp/pkg.tar.gz"
mkdir -p "$INSTALL_DIR"
for bin in brood nest brood-lsp; do
  install -m 0755 "$tmp/${name}/${bin}" "$INSTALL_DIR/${bin}"
done
say "installed brood ${version} to ${INSTALL_DIR}"

# --- PATH hint ---------------------------------------------------------------
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) say "add it to your PATH:  export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
"$INSTALL_DIR/brood" --version 2>/dev/null || true
say "done — try:  nest new hello && cd hello && nest run"
