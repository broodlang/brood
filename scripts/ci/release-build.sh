#!/usr/bin/env bash
# Build one release target (ADR-714): the toolchain archive (brood, nest, brood-lsp) and the
# runtime `nest release --target` downloads, each beside its `.sha256`, into dist/.
#
#   scripts/ci/release-build.sh TARGET TAG gui|headless
#
# The runtime is the lean `brood` (no dev-tools: no REPL, test runner or observer) that the
# toolchain's `nest` also embeds, so a `--target` of the host and a downloaded one append an
# app to the same bytes. `gui` builds the window into both (the CPU and GPU backends; no
# audio, which links ALSA at load time); `headless` is for a static musl build, which cannot
# load the display libraries the window opens at run time.
#
# The same cargo invocations as the Makefile's `release` (release-fast profile, the
# assertion-free RUSTFLAGS), written out because the Windows runner has no `make`.
set -euo pipefail

target="${1:?usage: release-build.sh TARGET TAG gui|headless}"
tag="${2:?usage: release-build.sh TARGET TAG gui|headless}"
window="${3:?usage: release-build.sh TARGET TAG gui|headless}"

features=(--no-default-features
  --features brood/jit --features brood/stdimage --features brood/treesit-grammars)
case "$window" in
  gui) features+=(--features brood/gui --features brood/gui-gpu) ;;
  headless) ;;
  *) echo "release-build.sh: the third argument is gui or headless, not '$window'" >&2; exit 2 ;;
esac

exe=""
case "$target" in *windows*) exe=".exe" ;; esac

export RUSTFLAGS="${RUSTFLAGS:-} -C debug-assertions=off -C overflow-checks=off"
bindir="target/$target/release-fast"
mkdir -p dist

# A sidecar in `sha256sum`'s format, naming the file by its bare name, which is what
# install.sh, install.ps1 and `nest release` read.
checksum() {
  local file="$1"
  local digest
  if command -v sha256sum >/dev/null 2>&1; then
    digest="$(sha256sum "dist/$file" | cut -d' ' -f1)"
  else
    digest="$(shasum -a 256 "dist/$file" | cut -d' ' -f1)"
  fi
  printf '%s  %s\n' "$digest" "$file" > "dist/$file.sha256"
}

# 1. The lean runtime, published as it is and embedded into nest below.
cargo build --profile release-fast -p cli "${features[@]}" --target "$target"
runtime="brood-runtime-${tag}-${target}${exe}"
cp "$bindir/brood$exe" "dist/$runtime"
checksum "$runtime"

# build.rs reads the embed path with the platform's own path rules.
embed="$PWD/dist/$runtime"
if command -v cygpath >/dev/null 2>&1; then embed="$(cygpath -w "$embed")"; fi

# 2. The toolchain: nest (embedding the runtime), the LSP, and the full brood a person runs.
BROOD_EMBED_RUNTIME="$embed" \
  cargo build --profile release-fast -p nest "${features[@]}" --features brood/dev-tools --target "$target"
cargo build --profile release-fast -p brood-lsp --target "$target"
cargo build --profile release-fast -p cli "${features[@]}" --features brood/dev-tools --target "$target"

name="brood-${tag}-${target}"
mkdir -p "dist/$name"
for binary in brood nest brood-lsp; do
  cp "$bindir/$binary$exe" "dist/$name/"
done
if [ -n "$exe" ]; then
  archive="$name.zip"
  (cd dist && 7z a -tzip -bd "$archive" "$name" >/dev/null)
else
  archive="$name.tar.gz"
  tar -C dist -czf "dist/$archive" "$name"
fi
checksum "$archive"
rm -rf "dist/$name"
echo "release-build: dist/$archive and dist/$runtime, each with its .sha256"
