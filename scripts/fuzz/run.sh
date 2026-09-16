#!/bin/bash
# Differential + crash fuzz runner. Generates N programs from a generator and runs
# each under the tree-walker (reference), VM-no-JIT, VM+JIT, and GC-stress; flags
# any output divergence, any "BAD" (oracle generators), or any crash (rc>128).
#
# Usage: scripts/fuzz/run.sh <generator> [count] [base-seed]
#   e.g. scripts/fuzz/run.sh metamorphic 300
# Build the armed binary first:
#   RUSTFLAGS="-C debug-assertions=on" cargo build --release --features jit --bin brood
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BROOD:-$ROOT/target/release/brood}"
GEN="$1"; N="${2:-200}"; BASE="${3:-0}"
GENPY="$ROOT/scripts/fuzz/generators/${GEN}.py"
[ -f "$GENPY" ] || { echo "no generator: $GENPY (have: $(ls $ROOT/scripts/fuzz/generators | sed 's/.py//' | tr '\n' ' '))"; exit 2; }
[ -x "$BIN" ] || { echo "no brood binary at $BIN — build it first"; exit 2; }
WORK="$(mktemp -d "${TMPDIR:-/tmp}/brood-fuzz-${GEN}.XXXX")"
trap 'rm -rf "$WORK"' EXIT
python3 "$GENPY" "$N" "$BASE" "$WORK" >/dev/null
echo "running $GEN: $N programs x 4 engine configs ..."
# A differential is only evidence when the programs RUN. Every generator emitted stale
# names for weeks after the ADR-302/ADR-330 rename waves (`println`, `rem`, `concat`,
# `rope-insert`, …): each program died on its first form, all four engines agreed on an
# empty stdout, and this runner reported "0 divergences" on 1850 programs (2026-09-15) —
# the same failure `bench/smoke.py` and `stress/fuzz_programs.py` had before it. So a
# program is STALE, and the run fails, when the reference engine reports an unbound symbol
# or prints nothing — for every generator but the two whose programs are MEANT to be
# rejected: `checker` (a malformed `sig`; its preamble is real code, so the unbound rule
# still applies) and `syntax` (random tokens — an unbound name there is the point, so
# neither rule applies). Both are still held to "no crash" below.
expect_output=1; expect_bound=1
case "$GEN" in checker) expect_output=0;; syntax) expect_output=0; expect_bound=0;; esac
bad=0; div=0; crash=0; checked=0; stale=0
for f in "$WORK"/*.blsp; do
  tw=$(timeout 60 env BROOD_VM=0 "$BIN" "$f" 2>"$WORK/tw.err")
  jt=$(timeout 60 "$BIN" "$f" 2>/dev/null)
  nj=$(timeout 60 env BROOD_NO_JIT=1 "$BIN" "$f" 2>/dev/null)
  gs=$(timeout 90 env BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1 "$BIN" "$f" 2>/dev/null)
  checked=$((checked+1))
  if [ "$expect_bound" = 1 ] && grep -q "unbound symbol" "$WORK/tw.err"; then
    stale=$((stale+1)); [ $stale -le 3 ] && echo "STALE GENERATOR $(basename "$f"): $(grep -m1 -o 'unbound symbol: [^ ]*' "$WORK/tw.err")"
  elif [ "$expect_output" = 1 ] && [ -z "$tw" ]; then
    stale=$((stale+1)); [ $stale -le 3 ] && echo "STALE GENERATOR $(basename "$f"): printed nothing — $(grep -m1 'error' "$WORK/tw.err" | cut -c1-120)"
  fi
  echo "$jt" | grep -q "BAD" && { bad=$((bad+1)); echo "BAD $(basename "$f"): $(echo "$jt"|grep BAD|head -1)"; }
  if [ "$tw" != "$jt" ] || [ "$tw" != "$nj" ] || [ "$tw" != "$gs" ]; then
    div=$((div+1)); echo "DIVERGE $(basename "$f"): tw=${tw:0:50} jit=${jt:0:50} nj=${nj:0:50} gs=${gs:0:50}"
  fi
  "$BIN" "$f" >/dev/null 2>&1; [ $? -gt 128 ] && { crash=$((crash+1)); echo "CRASH $(basename "$f") (kept: $f)"; cp "$f" "$ROOT/scripts/fuzz/" 2>/dev/null; }
done
echo "=== $GEN: checked=$checked bad=$bad divergences=$div crashes=$crash stale=$stale ==="
[ $stale -eq 0 ] && [ $bad -eq 0 ] && [ $div -eq 0 ] && [ $crash -eq 0 ]
