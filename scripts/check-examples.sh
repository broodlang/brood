#!/usr/bin/env bash
# Does every program in `examples/` still RUN against this build?
#
# Why this exists. `examples/` is outside `make test`, `nest check` and the breakage suite,
# so nothing ever executed it — and it rotted three separate ways without anyone noticing:
#
#   * `examples/editor` has called `eval-command/eval-last-sexp` since 2026-05-31, two and a
#     half months after that module moved to the sibling `brood-edit` project (KI-45 — the
#     example was deleted rather than repaired, `brood-edit` being the real one).
#   * ADR-227 moved `sqrt`/`frequencies` out of the prelude; `examples/life.blsp` needed a
#     `(:use …)` and would have died on a bare name.
#   * ADR-229 removed `require`; `examples/{webserver,hot-reload,editor}` all opened with a
#     `(require '…)` line and stopped loading entirely.
#
# That is the same pattern as KI-42 (breakage), KI-43 (a suite outside the gate) and KI-44
# (the benchmarks repo): a migration sweep covers what the gates cover, and everything else
# rots quietly. This is the cheap counter for `examples/`.
#
# What it checks, and why that and not "exit 0". Several examples cannot COMPLETE in a
# sandbox — `webserver`/`node_server` are servers that run until killed, `node_client` needs
# a peer, `font-zoom` needs `--features gui`. Their exit codes are environment noise. But
# **an `unbound symbol` diagnostic is never environment noise**: it means a name the example
# uses no longer exists, which is precisely what every rot above looked like. So each
# example is run briefly and its diagnostics are scanned; a program that reaches its own
# graceful "server not running" error has passed.
#
#   scripts/check-examples.sh              # every example
#   scripts/check-examples.sh life tour    # a subset
#   BROOD=path/to/brood scripts/check-examples.sh
#
# Exits non-zero if any example names something unbound, so it can gate.
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# WHICH binary, and which modules it carries — see `scripts/lib/gate-binary.sh` for both
# questions and why each one had a wrong answer here (KI-76's class).
. "$ROOT/scripts/lib/gate-binary.sh"
BROOD="${BROOD:-$(gate_pick brood)}"
NEST="${NEST:-$(gate_pick nest)}"
RUN_SECS="${RUN_SECS:-8}"

# Example PROJECTS whose suite is knowingly red — named here so the skip prints on every
# run rather than hiding, exactly as `BREAKAGE_SKIP` does for the breakage suite. Empty
# now that `examples/editor` (the one known-red project, KI-45) was deleted — brood-edit is
# the real editor project, so the in-repo duplicate was removed rather than kept limping.
SKIP_PROJECTS="${SKIP_PROJECTS:-}"

gate_require_fresh "$BROOD"
gate_load_modules "$BROOD"

want=("$@")
matches() {
  [ ${#want[@]} -eq 0 ] && return 0
  local n="$1" w
  for w in "${want[@]}"; do [ "$w" = "$n" ] && return 0; done
  return 1
}

fail=0
echo ">>> checking examples with $BROOD (${RUN_SECS}s each)"

# --- single-file examples --------------------------------------------------------------
for f in "$ROOT"/examples/*.blsp; do
  name="$(basename "$f" .blsp)"
  matches "$name" || continue
  out="$(cd "$ROOT" && timeout "$RUN_SECS" "$BROOD" "$f" 2>&1)"
  gate_classify "$out"
  case "$GATE_VERDICT" in
    ok)   echo "  ok      $name" ;;
    skip) echo "  skip    $name ($GATE_DETAIL)" ;;
    *)    fail=1; echo "  FAIL    $name"; printf '%s\n' "$GATE_DETAIL" | sed 's/^/            /' ;;
  esac
done

# --- examples that live in a SUBDIRECTORY without a project.blsp ------------------------
# `examples/hot-reload/main.blsp` is neither `examples/*.blsp` nor an example project, so the
# first version of this script skipped it — and that is exactly the file that was broken
# (a bare `reload-on-change` after ADR-229, and a bare `greet` that module namespacing had
# turned into `greeter/greet`). A gap in the checker is indistinguishable from a passing check.
for f in "$ROOT"/examples/*/*.blsp; do
  dir="$(dirname "$f")"
  [ -f "$dir/project.blsp" ] && continue          # handled as a project below
  name="$(basename "$dir")/$(basename "$f" .blsp)"
  matches "$(basename "$dir")" || matches "$(basename "$f" .blsp)" || [ ${#want[@]} -eq 0 ] || continue
  out="$(cd "$ROOT" && timeout "$RUN_SECS" "$BROOD" "$f" 2>&1)"
  gate_classify "$out"
  case "$GATE_VERDICT" in
    ok)   echo "  ok      $name" ;;
    skip) echo "  skip    $name ($GATE_DETAIL)" ;;
    *)    fail=1; echo "  FAIL    $name"; printf '%s\n' "$GATE_DETAIL" | sed 's/^/            /' ;;
  esac
done

# --- example PROJECTS (a project.blsp + tests/) -----------------------------------------
for p in "$ROOT"/examples/*/project.blsp; do
  [ -f "$p" ] || continue
  dir="$(dirname "$p")"; name="$(basename "$dir")"
  matches "$name" || continue
  case " $SKIP_PROJECTS " in *" $name "*)
    echo "  skip    $name/ (named in SKIP_PROJECTS)"; continue ;;
  esac
  if [ ! -x "$NEST" ]; then
    echo "  skip    $name/ (no nest binary at $NEST)"; continue
  fi
  out="$(cd "$dir" && timeout 120 "$NEST" test 2>&1)"
  if printf '%s\n' "$out" | grep -qE '0 failed'; then
    echo "  ok      $name/ ($(printf '%s\n' "$out" | grep -oE '[0-9]+ tests, [0-9]+ passed' | head -1))"
  else
    fail=1
    echo "  FAIL    $name/"
    printf '%s\n' "$out" | grep -E 'test failed|unbound|error' | head -3 | sed 's/^/            /'
  fi
done

echo
if [ $fail -ne 0 ]; then echo ">>> examples: FAILURES above"; exit 1; fi
echo ">>> examples: all clean"
