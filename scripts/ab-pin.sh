#!/usr/bin/env bash
# A/B a row against a PINNED baseline binary, with a base-vs-base control.
#
# Why this exists. `make ab` rebuilds its baseline in a throwaway worktree under `target/ab/`,
# which `make ab-clean` removes — so two invocations on different days measure against two
# different binaries, and a few-percent row cannot be told from drift. CLAUDE.md already
# prescribes the cure ("keep the `target/ab/<sha>/…/brood` binary, run `taskset`-pinned
# best-of-15 for base, base again, then new"), and `ab-bench.sh` already has both ingredients
# (`-a` to pin a prebuilt binary, `--floor` for the base-vs-base control). This wraps them so
# the method is one command and the baseline SURVIVES across sessions.
#
# It was written because ADR-228 needed it and did not have it: two best-of-15 runs of the
# same comparison read -9.1% and -5.6%, a 3.5-point spread, so the ADR had to record a range
# instead of a number. A few-percent row deserves a fixed reference.
#
#   scripts/ab-pin.sh 26b04e36 pipeline nqueens     # pin that ref, compare the working tree
#   N=21 scripts/ab-pin.sh HEAD~5 primes            # more reps for a noisier row
#   scripts/ab-pin.sh --list                        # cached baselines
#
# The cache lives in `target/ab-pinned/<sha>/brood` — under `target/` so it is gitignored and
# `cargo clean`able, but NOT under `target/ab/`, so `make ab-clean` leaves it alone.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE="$ROOT/target/ab-pinned"
N="${N:-15}"

if [ "${1:-}" = "--list" ]; then
  echo "pinned baselines in $CACHE:"
  for d in "$CACHE"/*/; do
    [ -d "$d" ] || continue
    sha="$(basename "$d")"
    printf "  %-12s %s\n" "$sha" "$(ls -la "$d/brood" 2>/dev/null | awk '{print $5" bytes  "$6" "$7" "$8}')"
  done
  exit 0
fi

ref="${1:?usage: ab-pin.sh <git-ref> <row>... (or --list)}"
shift
rows=("$@")
[ ${#rows[@]} -gt 0 ] || { echo "ab-pin: name at least one row (scripts/ab-bench.sh --list)" >&2; exit 2; }

sha="$(git -C "$ROOT" rev-parse --short "$ref")"
bin="$CACHE/$sha/brood"

if [ -x "$bin" ]; then
  echo ">>> reusing pinned baseline $sha ($bin)"
else
  echo ">>> building pinned baseline $sha (once; kept for later runs)"
  wt="$(mktemp -d "${TMPDIR:-/tmp}/ab-pin-$sha.XXXX")"
  # `--detach` so this never moves a branch, and the worktree is removed straight after the
  # build: only the binary is kept, which is the whole point.
  git -C "$ROOT" worktree add --detach --quiet "$wt" "$sha"
  trap 'git -C "$ROOT" worktree remove --force "$wt" 2>/dev/null || true' EXIT
  # Same target as `make ab` uses for both sides, so profile/features cannot drift.
  ( cd "$wt" && make release-brood >/dev/null 2>&1 ) || { echo "ab-pin: baseline build failed" >&2; exit 1; }
  built="$wt/target/release-fast/brood"
  [ -x "$built" ] || { echo "ab-pin: no binary at $built" >&2; exit 1; }
  mkdir -p "$(dirname "$bin")"
  cp "$built" "$bin"
  git -C "$ROOT" worktree remove --force "$wt" 2>/dev/null || true
  trap - EXIT
  echo ">>> pinned $sha"
fi

echo ">>> comparing the working tree against pinned $sha, best-of-$N, with a base-vs-base floor"
exec "$ROOT/scripts/ab-bench.sh" -a "$bin" --floor -n "$N" "${rows[@]}"
