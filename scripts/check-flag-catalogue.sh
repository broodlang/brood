#!/usr/bin/env bash
# Every `"BROOD_X"` literal in the runtime's source is a flag the catalogue in
# crates/lisp/src/diagnostics/debug_flags.rs lists — or names in NON_FLAGS, when it is a
# build-time constant rather than an env flag. The static half of the Rust test
# `every_runtime_flag_is_catalogued`, in under a second.
#
#   scripts/check-flag-catalogue.sh              # every tracked file
#   scripts/check-flag-catalogue.sh FILE…        # these files (the pre-push hook's changed ones)
#
# The Rust test's scope exactly: `crates/` and `std/`, minus the test/bench/example trees,
# whose `"BROOD_…"` literals are fixtures (an env var a test sets), not runtime flags.
set -euo pipefail

catalogue=crates/lisp/src/diagnostics/debug_flags.rs
if [ "$#" -eq 0 ]; then
  set -- $(git ls-files 'crates/*.rs' 'std/*.blsp')
fi

missing=""
for f in $(printf '%s\n' "$@" | grep -E '^(crates|std)/.*\.(rs|blsp)$' \
                              | grep -vE '(^|/)(target|tests|benches|examples)/' || true); do
  [ -f "$f" ] || continue
  [ "$f" = "$catalogue" ] && continue
  for flag in $(grep -oE '"BROOD_[A-Z0-9_]+"' "$f" 2>/dev/null | sort -u); do
    grep -qF -- "$flag" "$catalogue" || missing="$missing $flag ($f)"
  done
done

if [ -n "$missing" ]; then
  echo "BROOD_* flag read by the runtime but absent from the catalogue:$missing" >&2
  echo "  add an f(NAME, GROUP, \"…\") entry — or NON_FLAGS if it is not an env flag — in $catalogue" >&2
  exit 1
fi
