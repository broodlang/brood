#!/usr/bin/env bash
# `make tier-audit` — does every hot arm on the benchmark rows STAY native?
#
# Every gate in this repo is a value gate, and the KI-132 class produces the RIGHT
# answer slowly: an arm goes native, deopts on every activation, and after sixteen is
# latched onto the interpreter for the rest of the process (`deopt-thrash-latched`). Four
# helpers of the syntax highlighter ran that way for weeks, and `second` on `http`; a
# prepass/emit depth disagreement kept `json/num-end` and `json/object-acc` off the native
# tier for as long as they existed, reported only as a Cranelift verifier error nobody
# printed. `BROOD_JIT_BAIL_TRACE=1` names all of these now; this script runs every
# benchmark row under it and fails on any line that means "this arm lowered and then
# could not stay native" or "the lowering refused something it should not have":
#
#   deopt-thrash-latched     the arm deopted 16 times in a row — a per-activation deopt
#   suspend-latched          a native arm hosted a parking receive
#   join-depth-mismatch      prepass and emit loop disagree on a join's depth (a bug)
#   prepass-unmodelled-inst  the subset admitted an opcode the depth model lacks (a bug)
#   cranelift-define-function  the verifier rejected the IR (a bug)
#
# Refusals BY DESIGN (`call-mediated-boxed`, `chunk-outside-jit-subset`,
# `call-spill-exhausted`, …) are not failures: they are the profitability gate and the
# subset rule doing their job. Rows needing a server (`http`) or measuring boot
# (`startup`) are skipped; `http` is covered by hand with `bench/httpserver.py` up.
#
# Uses the benchmark checkout scripts/bench-dir.sh finds (or BENCH_DIR) and the newest
# release brood under target/ (or BROOD=); a missing checkout is a note, not a failure.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BENCH="${BENCH_DIR:-$("$ROOT/scripts/bench-dir.sh" "$ROOT")}"
BIN="${BROOD:-$ROOT/target/release-fast/brood}"
if [ ! -d "$BENCH/bench/brood" ]; then
  echo "tier-audit: no benchmark checkout at $BENCH — skipped (set BENCH_DIR)"; exit 0
fi
[ -x "$BIN" ] || { echo "tier-audit: no brood at $BIN — build with 'make release-brood' (or set BROOD=)"; exit 2; }
bad=0; rows=0
for f in "$BENCH"/bench/brood/*.blsp; do
  row="$(basename "$f" .blsp)"
  case "$row" in http|startup) continue;; esac
  rows=$((rows+1))
  hits="$( ( ulimit -v 16000000; BROOD_JIT_BAIL_TRACE=1 timeout 180 "$BIN" "$f" 2>&1 >/dev/null ) \
    | grep -E 'deopt-thrash-latched|suspend-latched|join-depth-mismatch|prepass-unmodelled-inst|cranelift-define-function' \
    | sed 's/ inlined=.*//; s/\[jit-bail\] //' | sort | uniq -c )"
  if [ -n "$hits" ]; then
    bad=$((bad+1)); echo "tier-audit: $row"; echo "$hits" | sed 's/^/    /'
  fi
done
if [ $bad -eq 0 ]; then
  echo "tier-audit: $rows rows, every hot arm stayed native (no latch, no lowering bug)"
else
  echo "tier-audit: $bad of $rows rows have an arm latched off the native tier or a lowering bug — see above"
  exit 1
fi
