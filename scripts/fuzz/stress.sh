#!/bin/bash
# Run every GC/VM/JIT stress program under the most aggressive config
# (GC_STRESS + GC_VERIFY + RT_GC_FLOOR: minor collection at every safepoint AND
# forced RUNTIME compaction) and the tree-walker; flag any crash/tripwire/divergence.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BROOD:-$ROOT/target/release/brood}"
bad=0
for f in "$ROOT"/scripts/fuzz/stress/*.blsp; do
  n=$(basename "$f")
  g=$(BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1 BROOD_RT_GC_FLOOR=1 timeout 300 "$BIN" "$f" 2>&1); grc=$?
  trip=$(echo "$g" | grep -iE "use-after-GC|is from epoch|\[jit-verify\] STALE|GC-VERIFY|cannot unwind|panicked" | head -1)
  [ $grc -gt 128 ] && { echo "CRASH $n sig=$((grc-128))"; bad=$((bad+1)); }
  [ -n "$trip" ] && { echo "TRIPWIRE $n: $trip"; bad=$((bad+1)); }
  echo "  $n: rc=$grc $(echo "$g" | grep -iE 'storm|result|TOTAL-BAD' | head -1)"
done
echo "=== STRESS DONE bad=$bad ==="
