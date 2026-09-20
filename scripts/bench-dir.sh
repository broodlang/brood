#!/usr/bin/env sh
# Where is the benchmark corpus checked out?
#
# Every tool that reads a benchmark row — `ab-bench.sh`, `tier-audit.sh`,
# `jit-lower-witness.sh` — hard-coded `../brood-benchmarks`, the upstream repository's own
# name. A clone does not have to be named after its remote, and on at least one machine it
# is `../brood-benchmark` (singular). Each tool treats a missing checkout as a skip rather
# than a failure, by design, so the whole class went quiet: `make tier-audit` reported "no
# benchmark checkout — skipped", the handoff recorded it as "the one `make green-all`
# component with no local verdict", and a perf task was deferred off-box for a directory
# name. A tool that is allowed to skip must be sure about what it is skipping.
#
# So: resolve by looking, not by assuming, and print the resolved path (empty when there is
# genuinely nothing). Callers keep their own env override — this is only the default.
#
# Usage:  bench_dir="$(scripts/bench-dir.sh [<repo-root>])"
set -u

root="${1:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"

for cand in "$root/../brood-benchmarks" "$root/../brood-benchmark"; do
  if [ -d "$cand/bench/brood" ]; then
    # Canonicalise, so a caller printing this path names something a reader can `cd` to.
    (cd "$cand" && pwd)
    exit 0
  fi
done

# Nothing found: name the canonical location, so the caller's "no checkout at …" message
# tells the reader what to clone and where.
echo "$root/../brood-benchmarks"
