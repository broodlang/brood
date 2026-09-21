#!/bin/bash
# How a project's LOAD scales with module count, and what the startup image costs (ADR-218) —
# and, since ADR-380, what a WARM start costs, which is the row that used to hide an
# O(source bytes) re-parse of every file behind the image hit.
#
#   scripts/bench/gen-project.py 4000 /tmp/brood-n4000     # generate a fixture first (or let this do it)
#   scripts/bench/image-scale.sh 500 1000 2000 4000 8000   # then sweep it — the ~180-line shape
#   FNS=340 scripts/bench/image-scale.sh 250 500 1000      # the ~3 000-line shape (docs/large-project-scaling.md)
#
# Prints, per N: the cold load alone, the cold load + image write, the image's size, and two
# WARM starts with the image and the module index both current — `warm all` materialises the
# whole project (what `nest test`/`nest check` pay, O(project) by design) and `warm lazy`
# installs the image and materialises nothing (what `nest run` pays before it requires its
# entry: O(files) stats, and since ADR-380 no source read). The write is thus attributable
# rather than lumped into "startup", and a warm start that grows with file SIZE rather than
# with what the entry reaches is visible as a row, not a doc. Read the SLOPE (per-module
# marginal cost), never the level: the level folds in the runtime's ~180 MB base.
#
# Measured 2026-08-07 on the 180-line shape (idle box, release build): ~130 KB and ~1.6 ms per
# module, flat from 500 to 8 000, with image size exactly linear. There is no per-module
# memory defect in the loader — if a number says otherwise, suspect the measurement.
# Measured 2026-09-21 on the 3k-line shape at N=1000: warm 3.0 s before ADR-380 (the module
# index re-parsed every file, twice), independent of file size after it.
#
# TRAPS, each of which produced a wrong number here first:
#   * The box must be IDLE. A sweep run beside `make test` read 12.8 s for N=500 against
#     1.67 s for N=1000. This script waits for load < 2 before each row.
#   * Discard the first run after a fresh build (cold boot cache). The warm-up below does.
#   * `nest` and `brood` have different build-ids, and the fingerprint includes it — an image
#     written by one is always a miss for the other. Measure one binary at a time.
#   * `nest run` is NOT the loader: on a cold run it also does the advisory pre-flight, which
#     is most of the cost on a large project. Use `brood` when the question is the loader.
#   * This script drove `project/project-setup` and `project/project-load-sources` — names
#     that ADR-325 renamed — for a month with nothing running it (the "generator no gate can
#     see" trap in CLAUDE.md). The names below are the current ones; if a row prints an
#     error instead of a time, suspect a rename before a regression.
set -u
B=$(readlink -f "${BROOD:-$(cd "$(dirname "$0")/../.." && pwd)/target/release/brood}")
[ -x "$B" ] || { echo "no brood at $B (cargo build --release --bin brood)"; exit 1; }
FNS=${FNS:-20}
suffix=""; [ "$FNS" != 20 ] && suffix="-f$FNS"

sizes=${@:-500 1000 2000 4000 8000}
until [ "$(cut -d' ' -f1 /proc/loadavg | cut -d. -f1)" -lt 2 ]; do sleep 20; done

for n in $sizes; do
  [ -d "/tmp/brood-n$n$suffix/src" ] || "$(dirname "$0")/gen-project.py" "$n" "/tmp/brood-n$n$suffix" --fns "$FNS" >/dev/null
done
# Warm the boot cache; this result is discarded.
(cd "/tmp/brood-n$(echo $sizes | cut -d' ' -f1)$suffix" && "$B" -e '(+ 1 1)' >/dev/null 2>&1) || true

printf '%-7s %-24s %-24s %-9s %-18s %s\n' "N" "load only (s / MB)" "load+write (s / MB)" "image MB" "warm all (s / MB)" "warm lazy (s / MB)"
for n in $sizes; do
  R=/tmp/brood-n$n$suffix
  cat > /tmp/img-lo-$n.blsp <<EOS
(project/setup "$R")
(project/load-sources "$R")
EOS
  cat > /tmp/img-lw-$n.blsp <<EOS
(project/setup "$R")
(project-image/load-sources-cached "$R")
EOS
  cat > /tmp/img-lz-$n.blsp <<EOS
(project/setup "$R")
(project-image/setup-lazy-image "$R")
EOS
  rm -rf "$R/.brood"
  lo=$(cd "$R" && /usr/bin/time -f "%e %M" "$B" /tmp/img-lo-$n.blsp 2>&1 >/dev/null | tail -1)
  rm -rf "$R/.brood"
  lw=$(cd "$R" && /usr/bin/time -f "%e %M" "$B" /tmp/img-lw-$n.blsp 2>&1 >/dev/null | tail -1)
  img=$(ls -l "$R/.brood/image.bin" 2>/dev/null | awk '{printf "%.0f", $5/1048576}')
  # The same program again: the image AND the module index are current, so this is what every
  # later invocation pays. Before ADR-380 it re-read every source file to root the modules.
  wm=$(cd "$R" && /usr/bin/time -f "%e %M" "$B" /tmp/img-lw-$n.blsp 2>&1 >/dev/null | tail -1)
  lz=$(cd "$R" && /usr/bin/time -f "%e %M" "$B" /tmp/img-lz-$n.blsp 2>&1 >/dev/null | tail -1)
  printf '%-7s %-24s %-24s %-9s %-18s %s\n' "$n" \
    "$(echo $lo | cut -d' ' -f1) / $(( $(echo $lo | cut -d' ' -f2) / 1024 ))" \
    "$(echo $lw | cut -d' ' -f1) / $(( $(echo $lw | cut -d' ' -f2) / 1024 ))" \
    "${img:-0}" \
    "$(echo $wm | cut -d' ' -f1) / $(( $(echo $wm | cut -d' ' -f2) / 1024 ))" \
    "$(echo $lz | cut -d' ' -f1) / $(( $(echo $lz | cut -d' ' -f2) / 1024 ))"
done
