#!/bin/bash
# How a project's LOAD scales with module count, and what the startup image costs (ADR-218).
#
#   scripts/bench/gen-project.py 4000 /tmp/brood-n4000     # generate a fixture first
#   scripts/bench/image-scale.sh 500 1000 2000 4000 8000   # then sweep it
#
# Prints, per N: the cold load alone, the cold load + image write, and the image's size —
# so the write is attributable rather than lumped into "startup". Read the SLOPE (per-module
# marginal cost), never the level: the level folds in the runtime's ~180 MB base.
#
# Measured 2026-08-07 on this shape (idle box, release build): ~130 KB and ~1.6 ms per
# module, flat from 500 to 8 000, with image size exactly linear. There is no per-module
# memory defect in the loader — if a number says otherwise, suspect the measurement.
#
# TRAPS, each of which produced a wrong number here first:
#   * The box must be IDLE. A sweep run beside `make test` read 12.8 s for N=500 against
#     1.67 s for N=1000. This script waits for load < 2 before each row.
#   * Discard the first run after a fresh build (cold boot cache). The warm-up below does.
#   * `nest` and `brood` have different build-ids, and the fingerprint includes it — an image
#     written by one is always a miss for the other. Measure one binary at a time.
#   * `nest run` is NOT the loader: on a cold run it also does the advisory pre-flight, which
#     is most of the cost on a large project. Use `brood` when the question is the loader.
set -u
B=${BROOD:-$(cd "$(dirname "$0")/../.." && pwd)/target/release/brood}
[ -x "$B" ] || { echo "no brood at $B (cargo build --release --bin brood)"; exit 1; }

sizes=${@:-500 1000 2000 4000 8000}
until [ "$(cut -d' ' -f1 /proc/loadavg | cut -d. -f1)" -lt 2 ]; do sleep 20; done

for n in $sizes; do
  [ -d "/tmp/brood-n$n/src" ] || "$(dirname "$0")/gen-project.py" "$n" "/tmp/brood-n$n" >/dev/null
done
# Warm the boot cache; this result is discarded.
(cd "/tmp/brood-n$(echo $sizes | cut -d' ' -f1)" && "$B" -e '(+ 1 1)' >/dev/null 2>&1) || true

printf '%-7s %-24s %-24s %s\n' "N" "load only (s / MB)" "load+write (s / MB)" "image MB"
for n in $sizes; do
  R=/tmp/brood-n$n
  cat > /tmp/img-lo-$n.blsp <<EOF
(require 'project)
(project/project-setup "$R")
(project/project-load-sources "$R")
EOF
  cat > /tmp/img-lw-$n.blsp <<EOF
(require 'project)
(project/project-setup "$R")
(project/project-load-sources-cached "$R")
EOF
  rm -rf "$R/.brood"
  lo=$(cd "$R" && /usr/bin/time -f "%e %M" "$B" /tmp/img-lo-$n.blsp 2>&1 >/dev/null | tail -1)
  rm -rf "$R/.brood"
  lw=$(cd "$R" && /usr/bin/time -f "%e %M" "$B" /tmp/img-lw-$n.blsp 2>&1 >/dev/null | tail -1)
  img=$(ls -l "$R/.brood/image.bin" 2>/dev/null | awk '{printf "%.0f", $5/1048576}')
  printf '%-7s %-24s %-24s %s\n' "$n" \
    "$(echo $lo | cut -d' ' -f1) / $(( $(echo $lo | cut -d' ' -f2) / 1024 ))" \
    "$(echo $lw | cut -d' ' -f1) / $(( $(echo $lw | cut -d' ' -f2) / 1024 ))" \
    "${img:-0}"
done
