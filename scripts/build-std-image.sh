#!/bin/sh
# Build the stdlib startup image before the suite runs, so the suite exercises the load path
# users actually get (KI-78).
#
# The image is DEFAULT-ON since v0.15.0 (ADR-256/281), and default-on is safe by construction:
# with no image on disk `%std-image-install` returns nil in ~30 us and `require` reads source.
# The runtime deliberately never BUILDS one — ~1 s, which would land on exactly the short-lived
# runs the image exists to speed up — so `nest` writes it.
#
# Nothing in CI did. Every job therefore ran the source path while the shipped default was the
# imaged one, and the suite's coverage of the default was not merely absent but
# NONDETERMINISTIC: `image_matches_source.rs` (ADR-280) builds an image itself and writes it to
# ~/.cache/brood, and nextest gives each case its own process in no guaranteed order, so whether
# any other case ran imaged depended on scheduling.
#
# One build up front removes the ambiguity in both directions: with this script the suite runs
# imaged, and the one job that must cover source sets BROOD_NO_STDIMAGE=1 explicitly (ci.yml's
# tree-walker job) rather than getting the source path by accident.
#
# Like `warm-boot-cache.sh` beside it, this is infrastructure and must never redden a run: every
# failure path exits 0. If `nest` is not built yet there is simply no image, and the suite behaves
# exactly as it did before — which is a correct configuration, not a broken one.
#
# Off switch: BROOD_NO_STDIMAGE=1, the same variable that turns the image off at runtime. If you
# have opted out of images there is no reason to spend a second building one, and a source-path
# job gets a source-path setup without needing a second flag.
set -u

if [ -n "${BROOD_NO_STDIMAGE:-}" ]; then
    echo "build-std-image: skipped (BROOD_NO_STDIMAGE set) — the suite will run the source path"
    exit 0
fi

REPO="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$REPO/target}"
PROFILE="${1:-debug}"
NEST="$TARGET/$PROFILE/nest"

if [ ! -x "$NEST" ]; then
    echo "build-std-image: no $PROFILE nest binary yet — nothing to build (suite runs source)"
    exit 0
fi

# `nest stdimage` is idempotent-ish: it reports `:present` when a current image already exists
# rather than rebuilding, so this costs ~30 ms on a warm tree and ~1 s after a commit (the id
# carries the git sha and a content hash of every baked-in .blsp, so any change invalidates it).
if out="$("$NEST" stdimage 2>&1)"; then
    printf 'build-std-image: %s\n' "$(printf '%s' "$out" | tail -1)"
else
    echo "build-std-image: nest stdimage failed, continuing on the source path"
fi
exit 0
