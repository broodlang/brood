#!/usr/bin/env bash
# rr-hunt.sh RUNS BINARY [ARGS…] — run BINARY under `rr record --chaos` up to RUNS times and stop
# at the first failing run, keeping its recording; every passing recording is deleted.
# Replay the kept one with `rr replay <dir>` (reverse-continue, reverse-step and watchpoints
# work there). For a one-in-N race that a loop of plain runs reproduces too rarely to debug.
#
#   make rr-hunt RR_TEST="target/debug/deps/brood-HASH eval::compile::tests::some_case --exact"
set -u
runs=${1:?usage: rr-hunt.sh RUNS BINARY [ARGS...]}
shift
[ $# -ge 1 ] || { echo "usage: rr-hunt.sh RUNS BINARY [ARGS...]" >&2; exit 2; }
command -v rr >/dev/null || { echo "rr-hunt: rr is not installed (apt install rr)" >&2; exit 2; }
paranoid=$(cat /proc/sys/kernel/perf_event_paranoid 2>/dev/null || echo 2)
if [ "$paranoid" -gt 1 ]; then
  echo "rr-hunt: kernel.perf_event_paranoid is $paranoid; rr needs <= 1" \
       "(sudo sysctl kernel.perf_event_paranoid=1)" >&2
  exit 2
fi
traces=${RR_TRACE_DIR:-target/rr-hunt}
mkdir -p "$traces"
for i in $(seq 1 "$runs"); do
  dir="$traces/run-$i"
  rm -rf "$dir"
  if rr record --chaos -o "$dir" "$@" >"$dir.log" 2>&1; then
    rm -rf "$dir" "$dir.log"
    printf '.'
  else
    echo
    echo "rr-hunt: run $i failed — recording kept in $dir (log: $dir.log)"
    echo "replay it with: rr replay $dir"
    exit 1
  fi
done
echo
echo "rr-hunt: $runs runs passed under rr chaos mode; no recording kept."
