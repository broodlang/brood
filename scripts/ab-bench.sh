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
#   scripts/ab-bench.sh --floor fib nbody      # + each row's own noise floor + a verdict
#   scripts/ab-bench.sh --json /tmp/ab.json    # + machine-readable rows
#   scripts/ab-bench.sh --tier 1 pfib          # measure on the VM (see `ab_tier` below)
#
# Env: BROOD_BENCH_DIR (default ../brood-benchmarks), AB_PIN_CPU (default 2),
#      AB_TIER (default: the binary's own default ceiling).
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
#
# `--floor` is that caveat made measurable: it runs the BASELINE binary twice per row and
# reports the base-vs-base spread, so a delta is read against that row's own noise rather
# than against a fixed 5%. It exists because a +5.3% "confirmed" regression (2026-07-29)
# was a baseline that had wandered ~10% across the day — the same change measured +0.9%
# against a +0.5% floor, i.e. neutral. A `verdict` column reports regressed/improved/noise
# on that basis. Costs one extra timed pass per row, hence opt-in.
#
# What this script deliberately does NOT do is compare against a *stored* baseline of
# absolute times. Absolute ms drift 10-20% between runs on this box and do not compare
# across machines at all (see the note above and docs/benchmarking.md §1), so a committed
# number would be a false reference. The comparison has to be against a binary built here,
# now, from the same target — which is what the worktree build exists for.
# ---------------------------------------------------------------------------
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

bench_dir="${BROOD_BENCH_DIR:-$root/../brood-benchmarks}"
pin_cpu="${AB_PIN_CPU:-2}"
# Execution-tier ceiling to measure at (ADR-222), or empty for the default. `--tier 1` is
# how a VM-path regression becomes visible: at the default ceiling a hot arm lowers to
# native and simply does not execute the interpreter, so a cost on the VM's call path reads
# as flat here while being a 3x on any workload whose arms don't lower (KI-40 read +1.3% at
# the default ceiling against 3.19x at ceiling 1).
ab_tier="${AB_TIER:-}"
base_ref="HEAD"
reps=7
prebuilt_base=""
allow_same=0
rows=()

# Rows that are supposed to use every core (the scheduler/process benchmarks);
# everything else is pinned to one CPU so a co-scheduled thread can't skew it.
#
# `spawn-live` and `supervisor` were MISSING here until 2026-08-13, so the two rows whose
# whole point is holding 300 000 / 20 000 live processes were being A/B'd on a single CPU —
# the scheduler behaviour they exist to measure cannot appear on one core, and a change that
# only shows up across cores read as flat. Found while measuring KI-40, whose entire class
# (per-call contention between worker threads) is invisible pinned. If you add a row that
# spawns, add it here too.
parallel_rows=" pfib spawn spawn-live supervisor pipeline ring pingpong "

# Rows this script CANNOT run: they need scaffolding the benchmark harness provides and
# this one deliberately does not. `http` needs a local server (`bench/harness.py` starts
# one per run, see its SERVER_FOR) — without it the program blocks forever on connect,
# and because each row is timed rather than bounded, that is an unkillable-looking hang
# rather than a failure. It cost a 4-hour zombie and a stuck `--all` sweep before this
# guard existed. Use `bench/harness.py --only http` for that row.
unsupported_rows=" http "

die() { printf 'ab-bench: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    -b|--base)    base_ref="${2:?-b needs a git ref}"; shift 2 ;;
    -n|--reps)    reps="${2:?-n needs a count}"; shift 2 ;;
    -a|--against) prebuilt_base="${2:?-a needs a path to a brood binary}"; shift 2 ;;
    --allow-same) allow_same=1; shift ;;
    --json)       json_out="${2:?--json needs a path}"; shift 2 ;;
    --floor)      measure_floor=1; shift ;;
    --tier)       ab_tier="$2"; shift 2 ;;
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

# Drop the rows that need harness scaffolding, loudly — silence here is what turns an
# unsupported row into a hang.
kept=()
for r in "${rows[@]}"; do
  case "$unsupported_rows" in
    *" $r "*) echo "ab-bench: skipping '$r' — needs the harness's local server; use \`bench/harness.py --only $r\`" >&2 ;;
    *) kept+=("$r") ;;
  esac
done
rows=("${kept[@]}")
[ ${#rows[@]} -gt 0 ] || die "every requested row is unsupported here"

# A row that blocks forever would otherwise be timed rather than failed, so bound each
# run. Generous: the slowest row (`ring`) is ~0.8s and a cold JIT adds to the first.
timeout_s="${AB_TIMEOUT:-120}"

# `--json <path>`: also write the sweep as JSON, so a tool (or an LLM) reads rows instead of
# re-parsing the human table. `--floor`: measure each row's own NOISE FLOOR by running the
# baseline binary TWICE and taking the base-vs-base spread — the method CLAUDE.md prescribes
# after a +5.3% "confirmed regression" turned out to be a baseline that had wandered ~10%
# across the day. Without a floor, a delta is a number with no scale; with one, the verdict
# below only calls a regression when the delta clears a couple of multiples of it. Opt-in
# because it costs one extra timed pass per row.
json_out="${json_out:-}"
measure_floor="${measure_floor:-0}"

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
  #
  # ...but a cargo FEATURE this tree's Makefile forwards that the baseline's crate does not
  # declare fails at dependency RESOLUTION, before any of that matters. `stdimage` (ADR-281,
  # 2026-08-28) was the first: every ref older than it died with
  #   package `cli` depends on `brood` with feature `stdimage` but `brood` does not have that
  # which broke `make ab` for exactly the refs it exists to compare against. So drop a feature
  # the baseline cannot know about — the alternative is a tool that only A/Bs against today.
  # Add a line here when a new optional feature appears; the check is one grep against the
  # baseline's own Cargo.toml, so it cannot go stale silently the way a hardcoded date would.
  base_feature_overrides=""
  grep -q '^stdimage *=' "$wt/crates/lisp/Cargo.toml" 2>/dev/null \
    || base_feature_overrides="$base_feature_overrides WITH_STDIMAGE=0"
  [ -n "$base_feature_overrides" ] &&
    echo "ab-bench: baseline predates$(echo "$base_feature_overrides" | sed 's/ WITH_/ /g;s/=0//g') — building it without" >&2
  # shellcheck disable=SC2086  # word splitting is intended for the VAR=VAL overrides
  make -f "$root/Makefile" -C "$wt" $base_feature_overrides release-brood >/dev/null \
    || die "baseline build failed"
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

# Footgun 6: the stdimage. Its id embeds the git sha, so a commit silently strands one
# side booting from source while the other boots `:live` — a fixed ~10 ms/run bias that
# read as "startup +32% on a comment-only change" (2026-08-30) and contaminated a
# rejection table's magnitudes the same night. The discipline line ("image :live on BOTH
# arms, verified per run") lives in three documents and was still skipped mid-session, so
# the harness now verifies it instead of trusting the operator: ASYMMETRY is fatal,
# symmetric non-live is a warning (both sides boot from source — comparable, but not the
# imaged path users get).
stdimage_state() { # $1 binary -> live|stale|absent|unreadable|unknown
  local probe out
  probe="$(mktemp --suffix=.blsp)"
  printf '(%%print (stdimage/status))\n' >"$probe"
  out="$(timeout 30 "$1" "$probe" 2>/dev/null || true)"
  rm -f "$probe"
  case "$out" in
    *":state :live"*)       echo live ;;
    *":state :stale"*)      echo stale ;;
    *":state :absent"*)     echo absent ;;
    *":state :unreadable"*) echo unreadable ;;
    *)                      echo unknown ;;  # predates stdimage/status (or errored)
  esac
}
base_img="$(stdimage_state "$base_bin")"
new_img="$(stdimage_state "$new_bin")"
if [ "$base_img" = "$new_img" ]; then
  if [ "$base_img" = "live" ]; then
    echo "ab-bench: stdimage :live on both arms" >&2
  else
    echo "ab-bench: WARNING stdimage is '$base_img' on BOTH arms — symmetric, so deltas are fair, but neither side measures the imaged boot users get" >&2
  fi
elif [ "$base_img" = "unknown" ] || [ "$new_img" = "unknown" ]; then
  # A side that predates `stdimage/status` (or was built WITH_STDIMAGE=0, the documented
  # override for old baselines) has no imaged path to be fair to — inherent to comparing
  # across the feature boundary, so warn rather than refuse.
  echo "ab-bench: WARNING stdimage state base=$base_img new=$new_img — one side predates the stdimage; boot-heavy rows compare across that feature boundary" >&2
else
  echo "ab-bench: stdimage MISMATCH — base=$base_img new=$new_img. One side boots from source: a fixed per-run bias, worst on short rows (startup/spawn)." >&2
  echo "ab-bench: to fix the working tree: cargo build -p nest && scripts/build-std-image.sh" >&2
  echo "ab-bench: to fix a worktree baseline: (cd <worktree> && cargo build -p nest && scripts/build-std-image.sh)" >&2
  echo "ab-bench: or make it symmetric-by-construction: BROOD_NO_STDIMAGE=1 on the whole run (loses the imaged path)." >&2
  die "stdimage state differs between the arms — fix it or set BROOD_NO_STDIMAGE=1 (see above)"
fi

# ---- measure ---------------------------------------------------------------
all_cpus="0-$(( $(nproc) - 1 ))"

best_of() { # $1 binary  $2 program  $3 cpu spec -> best wall ms
  local best=99999999 i t0 t1 ms
  for i in $(seq "$reps"); do
    t0=$(date +%s%N)
    # NOT wrapped in `timeout`: GNU timeout inflates a ~155 ms run to a dead-constant
    # 205 ms (measured 2026-07-28), so putting it inside the timed region silently
    # destroys fidelity — every row read the same value and every delta read +0.0%.
    # The hang guard lives on the warmup below, outside the measurement.
    env ${ab_tier:+BROOD_TIER=$ab_tier} taskset -c "$3" "$1" "$2" >/dev/null 2>&1 || true
    t1=$(date +%s%N)
    ms=$(( (t1 - t0) / 1000000 ))
    [ "$ms" -lt "$best" ] && best=$ms
  done
  echo "$best"
}

printf 'ab-bench: base=%s  new=working tree  reps=best-of-%s  pin=cpu%s (concurrency rows: all cores)  tier=%s\n\n' \
  "$base_desc" "$reps" "$pin_cpu" "${ab_tier:-default}" >&2
if [ "$measure_floor" -eq 1 ]; then
  printf '%-16s %9s %9s %9s %9s %-10s\n' row base new delta floor verdict
  printf '%-16s %9s %9s %9s %9s %-10s\n' ---------------- --------- --------- --------- --------- ----------
else
  printf '%-16s %9s %9s %9s\n' row base new delta
  printf '%-16s %9s %9s %9s\n' ---------------- --------- --------- ---------
fi

regressed=0
json_rows=()
for r in "${rows[@]}"; do
  prog="$bench_dir/bench/brood/$r.blsp"
  case "$parallel_rows" in *" $r "*) cpus="$all_cpus" ;; *) cpus="$pin_cpu" ;; esac
  # Footgun 4: discard one run per binary so the build-id-keyed boot cache is warm.
  # The warmup doubles as the hang guard: it is untimed, so `timeout` here costs
  # nothing, and a row that blocks forever fails loudly instead of pinning a core.
  timeout "$timeout_s" env ${ab_tier:+BROOD_TIER=$ab_tier} taskset -c "$cpus" "$base_bin" "$prog" >/dev/null 2>&1 \
    || die "row '$r' did not finish within ${timeout_s}s on the baseline — needs harness scaffolding?"
  timeout "$timeout_s" env ${ab_tier:+BROOD_TIER=$ab_tier} taskset -c "$cpus" "$new_bin"  "$prog" >/dev/null 2>&1 \
    || die "row '$r' did not finish within ${timeout_s}s on the working tree" 
  b=$(best_of "$base_bin" "$prog" "$cpus")
  # The floor pass runs the SAME binary again, between the two sides, so it samples the
  # same thermal/cache window the comparison does.
  if [ "$measure_floor" -eq 1 ]; then
    b2=$(best_of "$base_bin" "$prog" "$cpus")
    floor=$(awk -v a="$b" -v c="$b2" 'BEGIN{ if (a==0) print 0; else { d=(c-a)*100.0/a; if (d<0) d=-d; printf "%.1f", d } }')
  else
    floor=""
  fi
  n=$(best_of "$new_bin"  "$prog" "$cpus")
  d_num=$(awk -v a="$b" -v c="$n" 'BEGIN{ if (a==0) print "nan"; else printf "%.1f", (c-a)*100.0/a }')
  d=$(awk -v x="$d_num" 'BEGIN{ if (x=="nan") print "n/a"; else printf "%+.1f%%", x }')
  # Verdict: a regression must clear 5% AND — when a floor was measured — twice that row's
  # own noise. "Don't report a regression whose size is within a couple of multiples of the
  # floor you haven't measured" (CLAUDE.md).
  verdict=$(awk -v x="$d_num" -v f="${floor:--1}" 'BEGIN{
    if (x=="nan") { print "unknown"; exit }
    lim = 5.0; if (f >= 0 && 2*f > lim) lim = 2*f;
    if (x >= lim) print "regressed"; else if (x <= -lim) print "improved"; else print "noise";
  }')
  [ "$verdict" = "regressed" ] && regressed=1
  if [ -n "$floor" ]; then
    printf '%-16s %7sms %7sms %9s %8s%% %-10s\n' "$r" "$b" "$n" "$d" "$floor" "$verdict"
  else
    printf '%-16s %7sms %7sms %9s\n' "$r" "$b" "$n" "$d"
  fi
  json_rows+=("$(printf '{"row":"%s","base_ms":%s,"new_ms":%s,"delta_pct":%s,"floor_pct":%s,"verdict":"%s"}' \
    "$r" "$b" "$n" "${d_num/nan/null}" "${floor:-null}" "$verdict")")
done

if [ -n "$json_out" ]; then
  { printf '{"base":"%s","reps":%s,"pinned_cpu":"%s","rows":[' "$base_desc" "$reps" "$pin_cpu"
    for i in "${!json_rows[@]}"; do
      [ "$i" -gt 0 ] && printf ','
      printf '%s' "${json_rows[$i]}"
    done
    printf ']}\n'
  } > "$json_out"
  echo "ab-bench: wrote $json_out" >&2
fi

echo
if [ "$regressed" -eq 1 ]; then
  echo "ab-bench: a row is >=5% slower. Re-run that row ALONE before believing it" >&2
  echo "          (sweep interleaving shows a few percent of drift), then bisect with -b." >&2
fi
echo "ab-bench: baseline worktrees live in target/ab/ — 'make ab-clean' removes them." >&2
