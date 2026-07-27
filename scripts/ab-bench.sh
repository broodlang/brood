#!/usr/bin/env bash
# ab-bench.sh — A/B two brood builds across the cross-language benchmark programs.
#
# This is the tool for the question a perf session actually asks twenty times:
# "did my working-tree change make <row> faster, and did it break anything else?"
# It builds a baseline from any git ref, builds the working tree, and runs both
# over `../brood-benchmarks/bench/brood/*.blsp` with the measurement discipline
# the repo has learned the hard way. NOT a substitute for a published run — that
# is `bench/harness.py` in brood-benchmarks, which measures all seven languages
# in one session. This measures brood against brood.
#
# Usage:
#   scripts/ab-bench.sh                        # HEAD vs working tree, default rows
#   scripts/ab-bench.sh -b 26939ea             # against an older commit
#   scripts/ab-bench.sh -n 11 fib pfib         # more reps, specific rows
#   scripts/ab-bench.sh -a /path/to/other-brood   # A/B a binary you already built
#   scripts/ab-bench.sh --all                  # every row (slow)
#   scripts/ab-bench.sh --list                 # show available rows
#
# Env: BROOD_BENCH_DIR (default ../brood-benchmarks), AB_PIN_CPU (default 2).
#
# ---------------------------------------------------------------------------
# The five footguns this encodes, each of which has cost a real session:
#
#  1. **Build both sides identically.** Both go through `make release-brood`, so
#     profile (`release-fast`) and features (your ./configure) cannot drift
#     between them. A hand-run `cargo build --release` against a `make`-built
#     binary compares two different things.
#  2. **Never `-p brood`.** That builds the lib and does NOT relink the binary,
#     so you benchmark a stale executable. (2026-06-18: this produced a fully
#     bogus go/no-go and a phantom JIT regression.) `release-brood` uses `-p cli`.
#  3. **Prove the binaries differ.** If a build silently no-ops, both sides are
#     the same file and every delta reads 0.0%. This aborts on identical hashes
#     unless you passed --allow-same.
#  4. **Warm the boot cache.** The expanded-prelude cache (ADR-138) is keyed by
#     build id, so the FIRST run of a freshly built binary pays ~30ms extra. Each
#     binary gets a discarded warmup per row; without it a new binary looks slower
#     on every short row.
#  5. **Pin, and use best-of-N.** Wall time on this class of machine drifts
#     10-20% between runs. Single-core rows are pinned to one CPU; the process
#     rows (which are supposed to use the pool) get all of them. Best-of-N, never
#     a mean: the minimum is the least noise-contaminated sample.
#
# A caveat it CANNOT encode: interleaving A and B in one sweep can still show a
# few percent of thermal/cache drift on a row (2026-07-27: persistent-map read
# +3.7% in a sweep and 82 vs 81 ms when measured directly). If a row moves by
# only a few percent and you care, re-run that row alone before believing it.
# ---------------------------------------------------------------------------
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

bench_dir="${BROOD_BENCH_DIR:-$root/../brood-benchmarks}"
pin_cpu="${AB_PIN_CPU:-2}"
base_ref="HEAD"
reps=7
prebuilt_base=""
allow_same=0
rows=()

# Rows that are supposed to use every core (the scheduler/process benchmarks);
# everything else is pinned to one CPU so a co-scheduled thread can't skew it.
parallel_rows=" pfib spawn pipeline ring pingpong "

die() { printf 'ab-bench: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    -b|--base)    base_ref="${2:?-b needs a git ref}"; shift 2 ;;
    -n|--reps)    reps="${2:?-n needs a count}"; shift 2 ;;
    -a|--against) prebuilt_base="${2:?-a needs a path to a brood binary}"; shift 2 ;;
    --allow-same) allow_same=1; shift ;;
    --all)        rows=(ALL); shift ;;
    --list)       ls "$bench_dir/bench/brood/" | sed 's/\.blsp$//' | tr '\n' ' '; echo; exit 0 ;;
    -h|--help)    sed -n '2,30p' "$0"; exit 0 ;;
    -*)           die "unknown flag: $1" ;;
    *)            rows+=("$1"); shift ;;
  esac
done

[ -d "$bench_dir/bench/brood" ] || die "no benchmark programs at $bench_dir/bench/brood (set BROOD_BENCH_DIR)"

# Default row set: the ones that move under compute/JIT work, cheap enough to run
# on every iteration. Use --all when you need the regression sweep.
if [ ${#rows[@]} -eq 0 ]; then
  rows=(fib pfib bintree nqueens nbody sieve loop collatz primes json sort)
elif [ "${rows[0]}" = "ALL" ]; then
  mapfile -t rows < <(ls "$bench_dir/bench/brood/" | sed 's/\.blsp$//')
fi

for r in "${rows[@]}"; do
  [ -f "$bench_dir/bench/brood/$r.blsp" ] || die "no such row: $r (try --list)"
done

# ---- build side A (baseline) ----------------------------------------------
if [ -n "$prebuilt_base" ]; then
  [ -x "$prebuilt_base" ] || die "not an executable: $prebuilt_base"
  base_bin="$prebuilt_base"
  base_desc="$prebuilt_base"
else
  base_sha="$(git rev-parse --short "$base_ref")" || die "bad ref: $base_ref"
  wt="$root/target/ab/$base_sha"
  # A worktree, never a checkout/stash of your tree: the working tree is where
  # your uncommitted change lives and this script must not touch it.
  if [ ! -d "$wt" ]; then
    echo "ab-bench: creating baseline worktree $base_sha -> target/ab/$base_sha" >&2
    git worktree add --detach "$wt" "$base_sha" >/dev/null
  fi
  # config.mk is generated by ./configure and gitignored, so a fresh worktree has
  # none — copy ours in, or the baseline builds with different features (footgun 1).
  [ -f "$root/config.mk" ] && cp "$root/config.mk" "$wt/config.mk"
  echo "ab-bench: building baseline ($base_sha) ..." >&2
  # Build the baseline with THIS tree's Makefile (`-f`), not the one the old commit
  # shipped: identical flags on both sides is the whole point (footgun 1), and any
  # ref older than the `release-brood` target would otherwise fail outright — which
  # is exactly what happened the first time this script was run.
  make -f "$root/Makefile" -C "$wt" release-brood >/dev/null || die "baseline build failed"
  base_bin="$wt/target/release-fast/brood"
  base_desc="$base_sha"
fi

# ---- build side B (working tree) ------------------------------------------
echo "ab-bench: building working tree ..." >&2
make -C "$root" release-brood >/dev/null || die "working-tree build failed"
new_bin="$root/target/release-fast/brood"

[ -x "$base_bin" ] || die "baseline binary missing: $base_bin"
[ -x "$new_bin" ]  || die "working-tree binary missing: $new_bin"

# Footgun 3: identical binaries make every delta read 0.0%.
if [ "$(sha256sum <"$base_bin" | cut -d' ' -f1)" = "$(sha256sum <"$new_bin" | cut -d' ' -f1)" ]; then
  if [ "$allow_same" -eq 0 ]; then
    die "both sides are byte-identical — nothing to compare (did the build no-op? pass --allow-same to force)"
  fi
  echo "ab-bench: WARNING both binaries are identical" >&2
fi

# ---- measure ---------------------------------------------------------------
all_cpus="0-$(( $(nproc) - 1 ))"

best_of() { # $1 binary  $2 program  $3 cpu spec -> best wall ms
  local best=99999999 i t0 t1 ms
  for i in $(seq "$reps"); do
    t0=$(date +%s%N)
    taskset -c "$3" "$1" "$2" >/dev/null 2>&1 || true
    t1=$(date +%s%N)
    ms=$(( (t1 - t0) / 1000000 ))
    [ "$ms" -lt "$best" ] && best=$ms
  done
  echo "$best"
}

printf 'ab-bench: base=%s  new=working tree  reps=best-of-%s  pin=cpu%s\n\n' \
  "$base_desc" "$reps" "$pin_cpu" >&2
printf '%-16s %9s %9s %9s\n' row base new delta
printf '%-16s %9s %9s %9s\n' ---------------- --------- --------- ---------

regressed=0
for r in "${rows[@]}"; do
  prog="$bench_dir/bench/brood/$r.blsp"
  case "$parallel_rows" in *" $r "*) cpus="$all_cpus" ;; *) cpus="$pin_cpu" ;; esac
  # Footgun 4: discard one run per binary so the build-id-keyed boot cache is warm.
  taskset -c "$cpus" "$base_bin" "$prog" >/dev/null 2>&1 || true
  taskset -c "$cpus" "$new_bin"  "$prog" >/dev/null 2>&1 || true
  b=$(best_of "$base_bin" "$prog" "$cpus")
  n=$(best_of "$new_bin"  "$prog" "$cpus")
  d=$(awk -v a="$b" -v c="$n" 'BEGIN{ if (a==0) print "n/a"; else printf "%+.1f%%", (c-a)*100.0/a }')
  case "$d" in
    +[5-9].*%|+[1-9][0-9]*.*%) regressed=1 ;;
  esac
  printf '%-16s %7sms %7sms %9s\n' "$r" "$b" "$n" "$d"
done

echo
if [ "$regressed" -eq 1 ]; then
  echo "ab-bench: a row is >=5% slower. Re-run that row ALONE before believing it" >&2
  echo "          (sweep interleaving shows a few percent of drift), then bisect with -b." >&2
fi
echo "ab-bench: baseline worktrees live in target/ab/ — 'make ab-clean' removes them." >&2
