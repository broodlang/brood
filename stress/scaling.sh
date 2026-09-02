#!/usr/bin/env bash
# Parallel-scaling assertion: independent CPU-bound processes must go faster on a
# bigger worker pool. Run from `stress/run.sh` (not CI — it is timing-sensitive and
# wants a quiet machine).
#
# Why this exists: on 2026-07-28 a scaling regression was "found" that did not exist.
# The measurement compared BROOD_J=1 against BROOD_J=2 — but `worker_count()` floors
# the real pool at 2 (the spare that drains a dirty-blocked worker), so those are the
# SAME configuration, and the resulting 1.0x looked like total serialization. The
# baseline here is therefore 2 workers, the smallest pool that actually exists, and
# the script prints what the runtime *reports* rather than what was requested.
#
# It is deliberately a weak bound. Real speedup on this workload is ~2.5x of a ~3.0x
# hardware ceiling (2 -> 12 workers, measured against 12 independent OS processes),
# and the BEAM gets 2.4x on the same box — so Brood is at ~83% of available. The
# assertion catches the *collapse* case (a lock that serialises the pool), not a few
# percent of drift, because a tighter bound on a shared CI box is a flaky test.
set -uo pipefail

BROOD=${BROOD:-target/release/brood}
CORES=$(nproc 2>/dev/null || echo 1)
MIN_SPEEDUP=${MIN_SPEEDUP:-140}   # percent; 140 = 1.4x going 2 -> $CORES workers

# Below this there is no headroom to measure — two workers is already the floor.
if [ "$CORES" -lt 8 ]; then
  echo "skip  scaling ($CORES cores — needs >= 8 to have headroom over the 2-worker floor)"
  exit 0
fi

# The probe is a REAL FILE, not a heredoc. Embedded in this script it was invisible to
# `check-corpora`, which statically checks `stress/**.blsp` for names that no longer
# resolve — so a rename wave killed it silently (2026-09-02). As a `.blsp` it is checked
# before anyone runs it.
prog="$(cd "$(dirname "$0")" && pwd)/scaling_probe.blsp"
[ -f "$prog" ] || { echo "FAIL  scaling — missing $prog"; exit 1; }

# Best-of-3 each: this is a minimum-of-samples measurement, not an average — a
# co-scheduled neighbour can only ever make a sample slower.
# Milliseconds, best of 3. The sentinel is absurd for a MILLISECOND reading and ordinary for
# a nanosecond one, which is how this gate died once: the probe moved to `os/now-ns`, every
# reading was silently larger than the seed, `best` never moved, and the caller reported "no
# timing" — indistinguishable from a probe that printed nothing. `saw_reading` separates the
# two, because they need opposite fixes.
SENTINEL=999999
# Emits "<best> <last-raw-reading>" on one line. Both values have to come back through
# stdout: `best_of` is called in a command substitution, which is a SUBSHELL, so a global
# assigned inside it never reaches the caller — the first cut of this diagnostic set one and
# always reported "printed nothing", including for the nanosecond case it exists to name.
best_of() { # $1 = BROOD_J
  local best=$SENTINEL i t saw=""
  for i in 1 2 3; do
    t=$(BROOD_J="$1" "$BROOD" "$prog" 2>/dev/null | tail -1 | tr -dc '0-9')
    [ -n "$t" ] || continue
    saw="$t"
    [ "$t" -lt "$best" ] && best=$t
  done
  echo "$best $saw"
}

read -r small small_saw <<<"$(best_of 2)"
read -r large large_saw <<<"$(best_of "$CORES")"
saw_reading="${large_saw:-$small_saw}"

if [ "$small" = "$SENTINEL" ] || [ "$large" = "$SENTINEL" ] || [ "$large" -eq 0 ]; then
  if [ -n "$saw_reading" ]; then
    echo "FAIL  scaling — the probe printed $saw_reading, which never beats the $SENTINEL"
    echo "      sentinel. That sentinel assumes MILLISECONDS; check what the probe prints:"
    echo "      $prog"
  else
    echo "FAIL  scaling — the probe printed nothing at all (brood=$BROOD)."
    echo "      Run it directly to see why: $BROOD $prog"
  fi
  exit 1
fi

pct=$(( small * 100 / large ))
if [ "$pct" -lt "$MIN_SPEEDUP" ]; then
  printf 'FAIL  scaling: 2 workers %sms -> %s workers %sms = %d.%02dx\n' \
    "$small" "$CORES" "$large" "$(( pct / 100 ))" "$(( pct % 100 ))"
  echo "      below the ${MIN_SPEEDUP}% floor. Independent CPU-bound processes are not"
  echo "      using the pool — suspect a lock taken on a per-operation path, and check"
  echo "      the baseline really is 2 workers ((sched-stats) :workers) before digging."
  exit 1
fi

printf 'pass  scaling (2 workers %sms -> %s workers %sms = %d.%02dx)\n' \
  "$small" "$CORES" "$large" "$(( pct / 100 ))" "$(( pct % 100 ))"
