#!/usr/bin/env bash
# Fast feedback variant of `scripts/bench.sh` — runs the benchmark suite with
# divan's `--sample-count 3 --max-time 0.3` so the whole thing finishes in
# under ~10s. Use this between changes when you only want a directional read
# ("did that get faster?"). It deliberately does NOT archive results — the
# numbers from such few samples are not reproducible enough to be worth
# keeping; commit the headline number via `scripts/bench.sh` instead.
#
# Usage: scripts/quickbench.sh [extra divan args / filters]
#   e.g. scripts/quickbench.sh maps     # only the `maps` group
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

# The two named bench targets, run one at a time so divan's CLI flags don't
# leak into the lib's unit-test harness (cargo bench --benches passes the
# filters to *every* target). Same scope as `bench.sh` modulo archiving.
for bench in eval library; do
  NO_COLOR=1 cargo bench --quiet --bench "$bench" -- \
    --sample-count 3 --max-time 0.3 "$@" 2>&1 \
    | sed -E 's/\x1b\[[0-9;]*m//g'
done
