#!/usr/bin/env bash
# check-cost.sh — the instruction cost of `brood --check` (the run pre-flight's walk), this
# working tree against a git ref, per program, with the loads each check makes.
#
#   scripts/check-cost.sh [--base <ref>] [--scan-alive] [--files <f1 f2 …>]
#
# Deterministic where `make ab` is statistical: callgrind counts instructions, so one run per
# arm is the whole measurement (two runs of one arm agree to 0.03%). What it protects you from
# is the four ways this reading went wrong in one session (devlog 2026-09-20 (6)):
#
#   1. The stdlib image's WRITER is part of the measurement. A debug-written image of the
#      same std moved the same release binary's count +2.3% (KI-166's class: the interner
#      order decides the checker's iteration order). Each arm's image is written ONCE, here,
#      by the binary under test, and asserted `:live` in the same shell as the count.
#   2. A rebuild colds the prelude image (keyed on the binary's mtime): the first runs boot
#      the prelude from SOURCE, 1.5 billion instructions against 80M. Each binary is run
#      three times before it is counted.
#   3. The background JIT compiler's work is timing-dependent even under callgrind: traced
#      HEAD runs varied ±20% at the default tier. Both arms count at `BROOD_TIER=1`.
#   4. The tree moves. The base arm is a detached worktree at `<ref>` (as `make ab` does),
#      so an edit to `std/` that lands mid-session cannot change one arm's stdlib hash
#      between builds.
#
# `--scan-alive` adds a third column: the base arm with `BROOD_IMAGE_TRACE=1`, which is
# the only way to run a base older than KI-171's fix with its transitive scan on (the flag
# was on the scan's path). The verdict cache is off (`BROOD_NO_CHECK_CACHE=1`) — this
# measures the walk, and a replayed verdict has none.
#
# Needs `valgrind` (callgrind); `perf` needs a sysctl that moves under you on this box.
set -euo pipefail

die() { echo "check-cost: $*" >&2; exit 1; }
command -v valgrind >/dev/null || die "valgrind is not installed (callgrind is the instrument)"

root="$(cd "$(dirname "$0")/.." && pwd)"
base_ref="HEAD"
scan_alive=0
files=()
while [ $# -gt 0 ]; do
  case "$1" in
    --base) base_ref="$2"; shift 2 ;;
    --scan-alive) scan_alive=1; shift ;;
    --files) shift; while [ $# -gt 0 ] && [ "" = "$1" ]; do files+=("$1"); shift; done ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

if [  -eq 0 ]; then
  scratch="$(mktemp -d)"
  trap 'rm -rf "$scratch"' EXIT
  printf '(io/puts (str (os/env "HOME")))\n' >"$scratch/os-env.blsp"
  printf '(io/puts (str (datetime/utc-now)))\n' >"$scratch/datetime.blsp"
  printf '(io/puts (json/encode {:a [1 2 3]}))\n' >"$scratch/json-encode.blsp"
  files=("$scratch/os-env.blsp" "$scratch/datetime.blsp" "$scratch/json-encode.blsp")
  for bench in ../brood-benchmarks/bench/brood ../brood-benchmark/bench/brood; do
    if [ -d "$root/$bench" ]; then
      for row in base64 pipeline errors-deep json strings http wordcount; do
        [ -f "$root/$bench/$row.blsp" ] && files+=("$root/$bench/$row.blsp")
      done
      break
    fi
  done
fi

# ---- the base arm: a detached worktree at <ref>, built with THIS tree's Makefile ----------
base_sha="$(git -C "$root" rev-parse --short "$base_ref")" || die "no such ref: $base_ref"
wt="$root/target/check-cost/$base_sha"
if [ ! -d "$wt" ]; then
  echo "check-cost: creating baseline worktree $base_sha -> target/check-cost/$base_sha" >&2
  git -C "$root" worktree add --detach "$wt" "$base_sha" >/dev/null
fi
[ -f "$root/config.mk" ] && cp "$root/config.mk" "$wt/config.mk"
echo "check-cost: building baseline ($base_sha) ..." >&2
make -f "$root/Makefile" -C "$wt" release-brood >/dev/null || die "baseline build failed"
echo "check-cost: building working tree ..." >&2
make -C "$root" release-brood >/dev/null || die "working-tree build failed"
base_bin="$wt/target/release-fast/brood"
new_bin="$root/target/release-fast/brood"
cmp -s "$base_bin" "$new_bin" && die "the two binaries are byte-identical — nothing to compare"

# ---- warm, write the image ONCE per arm, assert it live — in this shell -----------------
prep() { # $1 binary
  local bin="$1" probe
  probe="$(mktemp --suffix=.blsp)"
  printf '(io/puts "warm")\n' >"$probe"
  for _ in 1 2 3; do "$bin" "$probe" >/dev/null 2>&1 || true; done
  printf '(stdimage/build)\n(io/puts (str (get (stdimage/status) :state)))\n' >"$probe"
  local state
  state="$(cd /tmp && "$bin" "$probe" 2>/dev/null | tail -1)"
  rm -f "$probe"
  [ "$state" = ":live" ] || die "$bin: stdlib image is '$state' after building it — a count now would be the source path"
}
prep "$base_bin"
prep "$new_bin"

count() { # $1 binary, $2 file, [$3 extra env]
  ( cd /tmp && env BROOD_TIER=1 BROOD_NO_CHECK_CACHE=1 ${3:-} \
      valgrind --tool=callgrind --callgrind-out-file=/dev/null "$1" --check "$2" 2>&1 \
    | grep -oE "refs: *[0-9,]+" | tr -d ' ,' | cut -d: -f2 )
}
loads() { # $1 binary, $2 file
  ( cd /tmp && env BROOD_TIER=1 BROOD_NO_CHECK_CACHE=1 BROOD_IMAGE_TRACE=1 "$1" --check "$2" 2>&1 \
    | grep -cE '^\[image\] [a-z][a-z/-]*$' || true )
}
pct() { awk -v a="$1" -v b="$2" 'BEGIN { if (a == 0) print "n/a"; else printf "%+.1f%%", (b - a) * 100 / a }'; }

if [ "$scan_alive" -eq 1 ]; then
  printf '%-16s %12s %12s %12s %8s %8s  %s\n' program "base" "base+scan" "new" "vs base" "vs scan" "loads base→new"
else
  printf '%-16s %12s %12s %8s  %s\n' program "base" "new" "delta" "loads base→new"
fi
for f in ""; do
  name="$(basename "$f" .blsp)"
  a="$(count "$base_bin" "$f")"
  n="$(count "$new_bin" "$f")"
  la="$(loads "$base_bin" "$f")"
  ln="$(loads "$new_bin" "$f")"
  if [ "$scan_alive" -eq 1 ]; then
    t="$(count "$base_bin" "$f" "BROOD_IMAGE_TRACE=1")"
    printf '%-16s %12s %12s %12s %8s %8s  %s→%s\n' "$name" "$a" "$t" "$n" "$(pct "$a" "$n")" "$(pct "$t" "$n")" "$la" "$ln"
  else
    printf '%-16s %12s %12s %8s  %s→%s\n' "$name" "$a" "$n" "$(pct "$a" "$n")" "$la" "$ln"
  fi
done
echo "check-cost: base $base_sha in $wt (git worktree remove it when done); counts are callgrind Ir at BROOD_TIER=1" >&2
