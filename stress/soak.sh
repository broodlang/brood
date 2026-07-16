#!/usr/bin/env bash
# Continuous differential-fuzz SOAK — runs successive seed batches back-to-back
# until a REAL divergence (a batch exits nonzero after the fuzzer's 3x
# re-confirmation) or the process is killed. For long/overnight runs: one
# background job that keeps finding bugs without needing relaunch, and parks on
# the first genuine finding so it's still reproducible when you come back.
#
# Usage: stress/soak.sh [START_SEED] [BATCH_SIZE]
set -u
cd "$(dirname "$0")/.."
BROOD=${BROOD:-target/release/brood}
start=${1:-500000}
batch=${2:-5000}
i=0
while true; do
  s=$((start + i * batch))
  echo "=== batch $i: seeds ${s}..$((s + batch - 1))  $(date -u +%Y-%m-%dT%H:%M:%SZ) ==="
  if ! BROOD="$BROOD" python3 -u stress/fuzz_programs.py --seeds "$batch" --start "$s"; then
    echo "!!!! DIVERGENCE in batch $i (seeds from ${s}) — parking soak for triage"
    exit 1
  fi
  echo "---- batch $i clean"
  i=$((i + 1))
done
