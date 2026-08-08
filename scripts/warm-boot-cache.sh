#!/bin/sh
# Warm the expanded-prelude boot cache before the suite fans out (KI-38).
#
# The cache (`~/.cache/brood/prelude-expanded-<hash>.blsp`) is keyed on
# `build_id` = version + git sha + **the running executable's own mtime**, so a
# rebuild colds it for every binary at once. A cold boot costs ~1.23 s against
# ~0.11 s warm, essentially all of it macro-expanding the prelude.
#
# That matters because the suite spawns `brood`/`nest` children by the dozen in
# one dense window, and they all share ONE cache file (keyed on the child
# binary, not the caller). Cold, the first child does not finish writing it
# until ~1.2 s in, so every child that starts inside that window misses too and
# the herd pays the expansion N times over. Cold-boot cost times herd size is
# linear and crosses `wait_until_listening`'s 20 s deadline at ~70 concurrent
# boots — which is how three boot-wait tests came to fail together (KI-38).
#
# One boot of each binary up front collapses that: the herd hits a warm cache.
#
# This is a pure optimisation and must never redden a run — every failure path
# here exits 0. If the binaries are missing (nothing built yet) there is simply
# nothing to warm, and the suite behaves exactly as it did before.
#
# Off switch: `BROOD_NO_WARM_BOOT_CACHE=1` skips the warm-up, which is how you
# reproduce the original KI-38 failure (there is no nextest-side way to skip a
# setup script — `--config 'profile.default.scripts=[]'` does NOT disable it,
# verified). Named after `BROOD_NO_BOOT_CACHE`, which disables the cache itself.
set -u

if [ -n "${BROOD_NO_WARM_BOOT_CACHE:-}" ]; then
    echo "warm-boot-cache: skipped (BROOD_NO_WARM_BOOT_CACHE set)"
    exit 0
fi

REPO="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$REPO/target}"
PROFILE="${1:-debug}"
BIN="$TARGET/$PROFILE"

WORK="$(mktemp -d 2>/dev/null)" || exit 0
trap 'rm -rf "$WORK"' EXIT
# A program that boots the runtime and does nothing else. The boot is the point;
# the cache is written on the way through.
echo '(def warm-boot-cache 1)' > "$WORK/warm.blsp" 2>/dev/null || exit 0

warmed=""

# `brood <file>` boots the prelude and writes the cache. This is the binary the
# boot-wait tests actually spawn, so it is the one that matters most.
if [ -x "$BIN/brood" ]; then
    "$BIN/brood" "$WORK/warm.blsp" >/dev/null 2>&1 && warmed="brood"
fi

# `nest` carries its own cache file (different mtime => different key). Note
# `nest --version` does NOT boot the prelude; `nest complete --` does, and is
# the cheapest command that reliably goes through the runtime.
if [ -x "$BIN/nest" ]; then
    "$BIN/nest" complete -- >/dev/null 2>&1 && warmed="${warmed:+$warmed }nest"
fi

[ -n "$warmed" ] && echo "warmed the boot cache for: $warmed"
exit 0
