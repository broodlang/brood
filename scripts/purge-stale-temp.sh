#!/bin/sh
# Sweep the PREVIOUS run's temp fixtures before the suite fans out (KI-126).
#
# KI-30 established the convention and `std/tool/test.blsp`'s `purge-stale-temp`
# implements it for the `.blsp` suite: name fixtures with a unique prefix, and drop
# the previous run's leftovers at file load. That bounds /tmp to one run's worth and
# recovers from a crashed run. A source-scan gate
# (`tests/temp_purge_coverage_test.blsp`) keeps every `.blsp` test honest about it.
#
# The RUST half of the suite has no such convention and no gate that can see it, and
# it leaked: 1109 entries / 340 MB on this box, oldest 2026-08-30. The mechanism is
# the one thing worth remembering — every site is written as
#
#     let dir = temp_dir().join(format!("brood-startup-image-{tag}-{}", process::id()));
#     let _ = fs::remove_dir_all(&dir);            // purge, at the start
#     … test …
#     let _ = fs::remove_dir_all(&dir);            // and again at the end
#
# which LOOKS like the convention and cannot be it: the name carries the test
# process's pid, so the opening purge matches a path no previous run ever used. The
# only cleanup that ever fires is the closing one, and that is skipped by every early
# return, every failed assertion and every panic. Nothing accumulated fast — one dir
# per test per run — which is why it took six weeks to become 340 MB.
#
# Fixing it site by site would mean ~40 call sites in 15 files and a new gate to keep
# them right. This is the same convention applied ONCE, where the run begins: nextest
# runs it as a setup script before the fan-out, so every Rust test gets the purge the
# `.blsp` tests get at load, whatever new fixtures arrive later. The pid in the names
# can stay — it is what keeps concurrent cases apart, and that is its job.
#
# Safety, since this removes files outside the repo:
#   * `-maxdepth 1` — only fixture ROOTS in the temp dir, never a path inside one.
#   * `-user` — never another account's entries on a shared /tmp.
#   * `-mmin +60` — a live run's fixtures are minutes old (the per-case cap is 2 min,
#     a full suite ~10). An hour untouched cannot belong to a running suite, so a
#     CONCURRENT run keeps its own. The `.blsp` convention needs no such guard: it
#     runs at file load, before that file's fixtures exist. A setup script runs once
#     for the whole process tree, so it asks about age instead.
#
# Like the other setup scripts this is housekeeping and must never redden a run:
# every failure path exits 0. Off switch `BROOD_NO_TEMP_PURGE=1`, named after
# `BROOD_NO_WARM_BOOT_CACHE`; `BROOD_TEMP_PURGE_AGE_MIN` moves the age guard (0
# sweeps everything, which is what a `make doctor` finding wants).
set -u

if [ -n "${BROOD_NO_TEMP_PURGE:-}" ]; then
    echo "purge-stale-temp: skipped (BROOD_NO_TEMP_PURGE set)"
    exit 0
fi

TMP="${TMPDIR:-/tmp}"
AGE="${BROOD_TEMP_PURGE_AGE_MIN:-60}"
UID_NOW="$(id -u 2>/dev/null)" || exit 0

# The fixture namespace of brood's own tests: `brood-…` and `brood_…` (the LSP's
# in-src test modules use the underscore spelling) and `nest-…` (the nest crate's
# integration tests — `nest-blsp-*` alone was 404 of the 1109).
removed=0
for pat in 'brood-*' 'brood_*' 'nest-*'; do
    hits=$(find "$TMP" -maxdepth 1 -name "$pat" -user "$UID_NOW" -mmin "+$AGE" -print 2>/dev/null | wc -l | tr -d ' ')
    [ "${hits:-0}" -eq 0 ] && continue
    find "$TMP" -maxdepth 1 -name "$pat" -user "$UID_NOW" -mmin "+$AGE" -exec rm -rf {} + 2>/dev/null
    removed=$((removed + hits))
done

[ "$removed" -gt 0 ] && echo "purge-stale-temp: removed $removed stale fixture(s) from $TMP"
exit 0
