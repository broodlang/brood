#!/usr/bin/env bash
# bench-ratio.sh — the load-robust VM benchmark report.
#
# On a loaded / low-powered machine, absolute benchmark times drift ±10-20%
# between separate process runs, so comparing two builds (or two `quickbench`
# invocations) is noise. The trustworthy signal is the **VM ÷ tree-walker ratio**
# measured as *adjacent rows in one `divan` process*: the tree-walker is a stable
# in-process reference that drifts *with* the VM under load, so the ratio survives
# load that wrecks the absolutes. (See docs/benchmarking.md for why.)
#
# This runs the `eval` engine-grid benches (which pin Vm vs Tw per row via
# `set_forced_engine`) and prints, per workload size, the VM median, the TW
# median, and VM/TW. A ratio < 1.0 means the VM beats the tree-walker. Track the
# *ratio* across changes, not the absolute ms.
#
# Usage:
#   scripts/bench-ratio.sh [divan-filter] [-- extra divan args]
#     scripts/bench-ratio.sh                 # the whole eval grid
#     scripts/bench-ratio.sh letrec_loop     # just one bench
#     scripts/bench-ratio.sh fib -- --sample-count 20
#
# Note: a normal build (no --features perf-stats) — we want clean timing, not
# counters. For *where* the time goes, build --features perf-stats and read
# `(vm-stats)` instead (a counting tool, not a timing one).
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

filter=""
extra=()
seen_dashdash=0
for a in "$@"; do
  if [[ "$a" == "--" ]]; then seen_dashdash=1; continue; fi
  if [[ $seen_dashdash -eq 1 ]]; then extra+=("$a"); else filter="$a"; fi
done

# A few more samples than quickbench (3) — enough to stabilise the median while
# staying quick. Override with `-- --sample-count N`.
default_args=(--sample-count 12 --max-time 3)

# Run the (heavy) bench to a file FIRST, then parse that file as a separate cheap
# step — never one pipeline. `cargo bench --bench eval` compiles + runs the whole
# engine grid; on a loaded box it can spike memory hard enough to be OOM-killed, and
# with `set -o pipefail` a death mid-pipe would take the parser (and an interactive
# session driving this) down with it. Decoupling means a bench blowup loses only the
# bench step, and the parser reads a file rather than a live pipe that could hang.
out="$(mktemp -t bench-ratio.XXXXXX.txt)"
trap 'rm -f "$out"' EXIT

if ! NO_COLOR=1 cargo bench --quiet --bench eval -- \
    "${default_args[@]}" "${extra[@]}" "$filter" 2>&1 \
    | sed -E 's/\x1b\[[0-9;]*m//g' > "$out"; then
  echo "bench-ratio: \`cargo bench --bench eval\` failed or was killed (see $out)." >&2
  echo "  partial output was captured — inspect it, then re-run with a narrower filter" >&2
  echo "  or fewer samples (e.g. \`$0 fib -- --sample-count 8\`) if the box ran out of memory." >&2
  trap - EXIT  # keep the file for inspection on failure
  exit 1
fi

python3 "$root/scripts/bench_ratio.py" "$out"
