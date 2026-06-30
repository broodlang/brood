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
bad=0; div=0; crash=0; checked=0
for f in "$WORK"/*.blsp; do
  tw=$(timeout 60 env BROOD_VM=0 "$BIN" "$f" 2>/dev/null)
  jt=$(timeout 60 "$BIN" "$f" 2>/dev/null)
  nj=$(timeout 60 env BROOD_NO_JIT=1 "$BIN" "$f" 2>/dev/null)
  gs=$(timeout 90 env BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1 "$BIN" "$f" 2>/dev/null)
  checked=$((checked+1))
  echo "$jt" | grep -q "BAD" && { bad=$((bad+1)); echo "BAD $(basename "$f"): $(echo "$jt"|grep BAD|head -1)"; }
  if [ "$tw" != "$jt" ] || [ "$tw" != "$nj" ] || [ "$tw" != "$gs" ]; then
    div=$((div+1)); echo "DIVERGE $(basename "$f"): tw=${tw:0:50} jit=${jt:0:50} nj=${nj:0:50} gs=${gs:0:50}"
  fi
  "$BIN" "$f" >/dev/null 2>&1; [ $? -gt 128 ] && { crash=$((crash+1)); echo "CRASH $(basename "$f") (kept: $f)"; cp "$f" "$ROOT/scripts/fuzz/" 2>/dev/null; }
done
echo "=== $GEN: checked=$checked bad=$bad divergences=$div crashes=$crash ==="
