#!/usr/bin/env bash
# The JIT lowering witness (docs/backend-seams.md §7).
#
# A *restructuring* of the JIT — moving decisions between modules, introducing a backend
# trait — must not change WHICH arms lower or WHAT they lower to. No test asserts that
# directly: `tests/jit.rs` proves each warmed program stays bit-identical to the
# interpreter, which stays true even if an arm silently stopped lowering and fell back to
# the (correct, slower) VM. This script is that missing witness.
#
# It runs a fixed set of benchmark rows under `BROOD_JIT_DUMP_IR=1` and prints the sorted,
# de-duplicated set of arm fingerprints — `(name, ckpt_slot, opcode sequence)`. Compare
# before/after with `diff`; the sets must be identical.
#
# Why the SET and not the count: installation is asynchronous (the background compiler
# thread races the program's exit), so a marginal arm may or may not land before a given
# run ends — measured ±2 on a 78-lowering sweep. Which arms are *eligible*, and what each
# lowers to, is deterministic. So the count is noise and the set is signal.
#
# The scalar-register path reports too, as of 2026-08-11 (`scalar-register: i64|f64` in place
# of `ckpt_slot:`). It previously emitted no `[jit-ir]` line at all, so `fib`/`pfib` — the arms
# it wins biggest on — were invisible both here and in the CLAUDE.md "did the arm ever lower?"
# check, where absence is the documented signal. That blind spot is what makes an ordering
# mistake in `jit_plan::plan_general_lowering` survive every other gate: the arm still computes
# the right answer on the VM, so only a benchmark moves. See docs/backend-seams.md §3.
#
# Usage:
#   scripts/jit-lower-witness.sh > /tmp/before.txt     # on the baseline
#   scripts/jit-lower-witness.sh > /tmp/after.txt      # after the change
#   diff /tmp/before.txt /tmp/after.txt                # must be empty
#
# Env: BROOD (binary under test), ROWS_DIR (brood-benchmarks rows).
set -u

BROOD=${BROOD:-target/release/brood}
ROWS_DIR=${ROWS_DIR:-../brood-benchmarks/bench/brood}

# row:BENCH_N — sized so every hot arm clears the tiering threshold and the background
# compiler has time to install, while the whole sweep stays ~15s. Rows chosen to span the
# distinct lowering strategies and the decisions that gate them: register-carried self-tail
# loops (loop, collatz), inline small-vector reads (bintree, matmul), the call-mediated
# profitability gate (nbody), float unboxing (mandelbrot, nbody), the JIT-lowered table ops
# (sieve), HOF/closure arms (reduce, sort), and non-tail recursion (nqueens).
ROWS="collatz:2000000 loop:20000000 bintree:15 nqueens:9 matmul:250 sieve:5000000 mandelbrot:600 nbody:200000 reduce:3000000 sort:1000000 primes:1000000 fib:32 pfib:30"

if [ ! -x "$BROOD" ]; then
  echo "no brood binary at $BROOD — build it with: cargo build --release --bin brood" >&2
  exit 1
fi

for spec in $ROWS; do
  row=${spec%%:*}
  n=${spec##*:}
  [ -f "$ROWS_DIR/$row.blsp" ] || { echo "missing row: $ROWS_DIR/$row.blsp" >&2; exit 1; }
  # Drop the leading `arm: <n>` (the chunk's instruction count) — it is redundant with the
  # `insts:` list that follows, which carries the opcodes themselves.
  BENCH_N="$n" BROOD_JIT_DUMP_IR=1 "$BROOD" "$ROWS_DIR/$row.blsp" 2>&1 >/dev/null \
    | sed -n 's/^\[jit-ir\] ===== arm: [0-9]* /'"$row"'\t/p' \
    | sort -u
done
