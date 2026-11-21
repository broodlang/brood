#!/usr/bin/env bash
# contract-cost.sh — the instruction cost of ONE armed contracted call, per call shape, this
# working tree against a git ref (ADR-748, ADR-773). Deterministic where `make ab` is
# statistical: callgrind counts instructions, and a call's cost is the difference between two
# loop lengths divided by the difference in calls, so boot, module loads and compilation
# cancel out.
#
#   scripts/contract-cost.sh [--base <ref>] [--tier 1|default] [--annotate[-unarmed] <row>] [rows…]
#
# Rows are `scripts/contract-calls.blsp`'s (default: all of them). Columns: unarmed
# (`BROOD_CONTRACTS=0`) base and this tree, the base armed, this tree armed, and this tree with
# `BROOD_NO_CONTRACT_DISPATCH=1` (every call through its Brood shim). `--tier 1` (the
# default) counts on the bytecode VM — the tier a contracted call's path is decided on; the
# JIT's background compiles are timing-dependent even under callgrind. `--annotate <row>`
# also prints callgrind's inclusive top functions for that row, both arms, which is what a
# change to the call path is attributed with; `--annotate-unarmed <row>` profiles it unarmed.
#
# Builds both arms with `make perf-symbols` (release-fast, unstripped), the base in a
# detached worktree as `make ab` does. Needs `valgrind`. Never on the development desktop
# (CLAUDE.md "Where tests run"): the Diagnose workflow runs it (`what=contract-cost`).
set -euo pipefail

die() { echo "contract-cost: $*" >&2; exit 1; }
command -v valgrind >/dev/null || die "valgrind is not installed (callgrind is the instrument)"

root="$(cd "$(dirname "$0")/.." && pwd)"
base_ref="HEAD"
tier="1"
annotate=""
annotate_mode="1"
rows=()
while [ $# -gt 0 ]; do
  case "$1" in
    --base) base_ref="$2"; shift 2 ;;
    --tier) tier="$2"; shift 2 ;;
    --annotate) annotate="$2"; shift 2 ;;
    --annotate-unarmed) annotate="$2"; annotate_mode="0"; shift 2 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) rows+=("$1"); shift ;;
  esac
done
[ ${#rows[@]} -gt 0 ] || rows=(int char-at any maybe record shape fresh-shape vector map tuple)
program="$root/scripts/contract-calls.blsp"
short_calls=20000
long_calls=120000

base_sha="$(git -C "$root" rev-parse --short "$base_ref")" || die "no such ref: $base_ref"
# Outside `target/`: CI caches `target/`, and a restored copy of a worktree is no worktree.
worktree="${TMPDIR:-/tmp}/contract-cost/$base_sha"
if ! git -C "$worktree" rev-parse --git-dir >/dev/null 2>&1; then
  rm -rf "$worktree"
  git -C "$root" worktree prune
  git -C "$root" worktree add --detach "$worktree" "$base_sha" >/dev/null
fi
[ -f "$root/config.mk" ] && cp "$root/config.mk" "$worktree/config.mk"
echo "contract-cost: building base ($base_sha) ..." >&2
make -f "$root/Makefile" -C "$worktree" perf-symbols >/dev/null || die "base build failed"
echo "contract-cost: building this tree ..." >&2
make -C "$root" perf-symbols >/dev/null || die "working-tree build failed"
base_bin="$worktree/target/release-symbols/brood"
new_bin="$root/target/release-symbols/brood"

# Warm each binary's boot cache and stdlib image, so neither count includes a source boot.
for binary in "$base_bin" "$new_bin"; do
  for _ in 1 2 3; do (cd /tmp && ROW=int N=10 "$binary" "$program" >/dev/null 2>&1) || true; done
  (cd /tmp && printf '(stdimage/build)\n' >/tmp/contract-cost-image.blsp && "$binary" /tmp/contract-cost-image.blsp >/dev/null 2>&1) || true
done

tier_env=""
[ "$tier" = "default" ] || tier_env="BROOD_TIER=$tier"

# The instructions of one run: $1 binary, $2 row, $3 calls, then extra env.
count() {
  local binary="$1" row="$2" calls="$3"
  shift 3
  ( cd /tmp && env $tier_env ROW="$row" N="$calls" "$@" \
      valgrind --tool=callgrind --smc-check=all-non-file --callgrind-out-file=/dev/null \
      "$binary" "$program" 2>&1 \
    | grep -oE "refs: *[0-9,]+" | tr -d ' ,' | cut -d: -f2 )
}
# Instructions per call: the long run minus the short one, over the calls between them.
per_call() {
  local binary="$1" row="$2"
  shift 2
  local short long
  short="$(count "$binary" "$row" "$short_calls" "$@")"
  long="$(count "$binary" "$row" "$long_calls" "$@")"
  [ -n "$short" ] && [ -n "$long" ] || { echo "?"; return; }
  echo $(( (long - short) / (long_calls - short_calls) ))
}

printf 'contract-cost: base %s, tier %s, instructions per call (loop iteration)\n\n' "$base_sha" "$tier"
printf '%-12s %9s %9s %9s %9s %9s %8s\n' row base-off new-off "base" "new" "new-shim" "delta"
printf '%-12s %9s %9s %9s %9s %9s %8s\n' ------------ --------- --------- --------- --------- --------- --------
for row in "${rows[@]}"; do
  base_unarmed="$(per_call "$base_bin" "$row" BROOD_CONTRACTS=0)"
  unarmed="$(per_call "$new_bin" "$row" BROOD_CONTRACTS=0)"
  base="$(per_call "$base_bin" "$row" BROOD_CONTRACTS=1)"
  new="$(per_call "$new_bin" "$row" BROOD_CONTRACTS=1)"
  shim="$(per_call "$new_bin" "$row" BROOD_CONTRACTS=1 BROOD_NO_CONTRACT_DISPATCH=1)"
  delta="$(awk -v a="$base" -v b="$new" 'BEGIN { if (a+0 == 0) print "n/a"; else printf "%+.1f%%", (b - a) * 100 / a }')"
  printf '%-12s %9s %9s %9s %9s %9s %8s\n' "$row" "$base_unarmed" "$unarmed" "$base" "$new" "$shim" "$delta"
done

if [ -n "$annotate" ]; then
  for side in base new; do
    binary="$base_bin"
    [ "$side" = new ] && binary="$new_bin"
    out="/tmp/contract-cost-$side.callgrind"
    ( cd /tmp && env $tier_env ROW="$annotate" N="$long_calls" BROOD_CONTRACTS="$annotate_mode" \
        valgrind --tool=callgrind --smc-check=all-non-file --callgrind-out-file="$out" \
        "$binary" "$program" >/dev/null 2>&1 ) || true
    echo
    echo "== $side, row $annotate (BROOD_CONTRACTS=$annotate_mode), $long_calls calls: inclusive"
    { callgrind_annotate --inclusive=yes "$out" 2>/dev/null || true; } | sed -n '1,70p' | sed "s|/home/runner/work/brood/brood/||g" | cut -c1-200
    echo
    echo "== $side, row $annotate (BROOD_CONTRACTS=$annotate_mode): self"
    { callgrind_annotate "$out" 2>/dev/null || true; } | sed -n '1,60p' | sed "s|/home/runner/work/brood/brood/||g" | cut -c1-200
  done
fi
