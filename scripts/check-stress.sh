#!/usr/bin/env bash
# Do the stress harnesses in `stress/` and `scripts/fuzz/stress/` still LOAD against this
# build?
#
# Why this exists. These two directories are outside `make test`, `nest check`, the breakage
# suite AND `check-examples`, so nothing ever ran them — and on 2026-08-20 eight of their
# files could not load at all, rotted by two separate rename waves that nobody noticed:
#
#   * ADR-230's `string/*` wave left `string-length`, `string-split`, `string-repeat`,
#     `string-span` and `string-span-until` behind in five files.
#   * ADR-236's prefix rollout left `format-source`, `gen-call`, `tcp-read-n` and
#     `tcp-read-until` in three more — some DOUBLE-prefixed (`stream/stream-to-list`),
#     where the sweep qualified a name the module had also shortened.
#
# That is the same pattern as KI-42 (breakage), KI-45 (`examples/`) and the `parse_prelude`
# bench that named `std/prelude.blsp` for a week after the prelude was split into nine files.
# A migration sweep covers what the gates cover; everything else rots quietly, and CI finds
# out one red build later — or, here, never. This is the cheap counter for the stress dirs.
#
# What it checks, and why that and not "exit 0". These harnesses are soaks, storms and
# scaling sweeps: `soak_selfcheck` runs until told to stop, the `*_storm` files are meant to
# be abusive, and several want a peer or a long budget. Their exit codes and durations under
# a gate are environment noise. But **an `unbound symbol` diagnostic is never environment
# noise** — it means a name the harness calls no longer exists, which is exactly what every
# rot above looked like. So each file is run briefly with small parameters and its
# diagnostics are scanned; a harness that reaches its own timeout has passed.
#
# `stress/*_test.blsp` are real test files and cheap (78 cases, ~7 s all told), so those are
# held to the stronger bar of actually passing — the same way `check-examples.sh` asserts
# `0 failed` for example projects rather than merely loading them.
#
#   scripts/check-stress.sh                    # every harness
#   scripts/check-stress.sh scale_sweep        # a subset
#   BROOD=path/to/brood scripts/check-stress.sh
#
# Exits non-zero if any harness names something unbound (or a stress test fails), so it can
# gate.
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# WHICH binary, and which modules it carries — see `scripts/lib/gate-binary.sh` for both
# questions and why each one had a wrong answer here (KI-76's class).
. "$ROOT/scripts/lib/gate-binary.sh"
BROOD="${BROOD:-$(gate_pick brood)}"
RUN_SECS="${RUN_SECS:-12}"
TEST_SECS="${TEST_SECS:-180}"

# Harnesses knowingly excluded — named here so the skip PRINTS on every run rather than
# hiding, exactly as `BREAKAGE_SKIP` does for the breakage suite and `SKIP_PROJECTS` for the
# examples. Empty: all of them load today, and a gate that starts out with exemptions is a
# gate nobody trusts.
STRESS_SKIP="${STRESS_SKIP:-}"

# Small parameters so a harness gets through more of its own body inside RUN_SECS. Each file
# defaults these itself, so the names it does not read are simply ignored.
STRESS_ENV="N=5 CHUNKS=5 SIZE=8 ITER=2 ROUNDS=2 SECS=1 PROCS=4"

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
echo ">>> checking stress harnesses with $BROOD (${RUN_SECS}s each, tests ${TEST_SECS}s)"

# --- stress/*_test.blsp: real tests, held to actually passing ---------------------------
# `--test` needs the `test` module, which a lean (`--no-default-features`) brood compiles
# away — so on that build this whole block cannot run, and saying so once beats 12 identical
# failures that look like rot.
have_test=1
gate_absent_module test && have_test=0
for f in "$ROOT"/stress/*_test.blsp; do
  [ -f "$f" ] || continue
  name="stress/$(basename "$f" .blsp)"
  short="$(basename "$f" .blsp)"
  matches "$short" || continue
  case " $STRESS_SKIP " in *" $short "*)
    echo "  skip    $name (named in STRESS_SKIP)"; continue ;;
  esac
  if [ "$have_test" = 0 ]; then
    echo "  skip    $name (needs the \`test\` module, absent from this lean build)"; continue
  fi
  out="$(cd "$ROOT" && timeout "$TEST_SECS" "$BROOD" --test "$f" 2>&1)"
  if printf '%s\n' "$out" | grep -qE '0 failed'; then
    echo "  ok      $name ($(printf '%s\n' "$out" | grep -oE '[0-9]+ tests, [0-9]+ passed' | head -1))"
  else
    fail=1
    echo "  FAIL    $name"
    printf '%s\n' "$out" | grep -E 'unbound|test failed|error' | head -3 | sed 's/^/            /'
  fi
done

# --- every other harness: it must LOAD, i.e. name nothing unbound -----------------------
# `stress/*.blsp` that are not tests (helpers like `table_digest`) plus the whole of
# `scripts/fuzz/stress/`.
for f in "$ROOT"/stress/*.blsp "$ROOT"/scripts/fuzz/stress/*.blsp; do
  [ -f "$f" ] || continue
  case "$f" in *_test.blsp) continue ;; esac
  short="$(basename "$f" .blsp)"
  case "$f" in
    "$ROOT"/stress/*) name="stress/$short" ;;
    *)               name="fuzz/$short" ;;
  esac
  matches "$short" || continue
  case " $STRESS_SKIP " in *" $short "*)
    echo "  skip    $name (named in STRESS_SKIP)"; continue ;;
  esac
  out="$(cd "$ROOT" && env $STRESS_ENV timeout "$RUN_SECS" "$BROOD" "$f" 2>&1)"
  gate_classify "$out"
  case "$GATE_VERDICT" in
    ok)   echo "  ok      $name" ;;
    skip) echo "  skip    $name ($GATE_DETAIL)" ;;
    *)    fail=1; echo "  FAIL    $name"; printf '%s\n' "$GATE_DETAIL" | sed 's/^/            /' ;;
  esac
done

echo
if [ $fail -ne 0 ]; then echo ">>> stress: FAILURES above"; exit 1; fi
echo ">>> stress: all clean"
