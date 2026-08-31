#!/usr/bin/env bash
# `make smoke-bedit` — the DOWNSTREAM smoke: run bedit's gates against THIS tree's `nest`.
#
# bedit (github.com/broodlang/bedit, a sibling checkout at ../bedit) is where a brood
# regression surfaces first — it is the largest program written in the language that is
# not in this repo. On 2026-08-30 a half-landed rename wave (ADR-302) shipped in an
# installed brood while bedit still called the old names, and nothing in brood's own
# gates could see it: `std/` + `tests/` were green, because the callers that broke were
# in another repository. This script is that missing gate, and the CI job
# `downstream-bedit` in .github/workflows/ci.yml runs exactly this script, so the local
# and the CI verdict are the same three commands:
#
#   1. nest check                                  — zero warnings (exit 0)
#   2. nest run --check-boot  (BROOD_GUI_HEADLESS=1) — every module loads, :main resolves,
#                                                    nothing runs (KI-66)
#   3. nest test                                   — the suite (~28 s wall locally)
#
# Every command runs under an address-space cap (CLAUDE.md, KI-87: a diverging process
# is indistinguishable from a heavy one until it has eaten the machine), and they run
# one at a time — each is a single OS process, which is what `-j1` buys the nextest
# runs. The cap is 16 GB rather than the 4 GB the Rust suite uses because `nest test`
# on bedit peaks at ~230 MB per run but its 1300+ cases fan out over the worker pool.
#
# Environment:
#   BEDIT_DIR   where bedit is (default: ../bedit, beside this repo)
#   NEST        which `nest` to use (default: the NEWEST of target/debug/nest,
#               target/release/nest, target/release-fast/nest — a stale binary fails
#               by agreeing with the baseline, so the choice and its build sha are printed)
#   SMOKE_ULIMIT_KB  the `ulimit -v` cap in KB (default 16000000)
#
# `--require` makes a missing BEDIT_DIR a failure (CI); by default it is a note and
# exit 0, so `make green-all` still works on a machine without the sibling checkout.
set -u

root=$(cd "$(dirname "$0")/.." && pwd)
require=0
for arg in "$@"; do
  case "$arg" in
    --require) require=1 ;;
    *) echo "usage: $0 [--require]" >&2; exit 2 ;;
  esac
done

BEDIT_DIR=${BEDIT_DIR:-$root/../bedit}
SMOKE_ULIMIT_KB=${SMOKE_ULIMIT_KB:-16000000}

fail=0
red()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=$((fail+1)); }
ok()   { printf '  \033[32mok\033[0m   %s\n' "$1"; }
note() { printf '  \033[33m!\033[0m    %s\n' "$1"; }

echo "== downstream smoke: bedit against this tree's nest =="

if [ ! -f "$BEDIT_DIR/project.blsp" ]; then
  if [ "$require" = 1 ]; then
    red "no bedit checkout at $BEDIT_DIR (set BEDIT_DIR)"
    exit 1
  fi
  note "no bedit checkout at $BEDIT_DIR — skipping (set BEDIT_DIR, or clone github.com/broodlang/bedit beside this repo)"
  exit 0
fi
BEDIT_DIR=$(cd "$BEDIT_DIR" && pwd)

# Pick the newest nest binary unless told which. Newest by mtime, because the question
# is "does the binary reflect the tree?" and the debug build is usually the most recent.
if [ -z "${NEST:-}" ]; then
  NEST=$(ls -t "$root"/target/debug/nest "$root"/target/release/nest "$root"/target/release-fast/nest 2>/dev/null | head -1 || true)
fi
if [ -z "${NEST:-}" ] || [ ! -x "$NEST" ]; then
  red "no nest binary found (build one: cargo build -p nest, or set NEST=...)"
  exit 1
fi

nest_version=$("$NEST" --version 2>/dev/null || echo "?")
tree_sha=$(git -C "$root" rev-parse --short HEAD 2>/dev/null || echo "?")
bedit_sha=$(git -C "$BEDIT_DIR" rev-parse --short HEAD 2>/dev/null || echo "?")
bedit_dirty=$(git -C "$BEDIT_DIR" status --porcelain 2>/dev/null | wc -l)
echo "  nest:  $NEST ($nest_version, built $(date -r "$NEST" '+%Y-%m-%d %H:%M'))"
echo "  brood: HEAD $tree_sha"
echo "  bedit: $BEDIT_DIR @ $bedit_sha ($bedit_dirty modified files)"
case "$nest_version" in
  *"$tree_sha"*) ;;
  *) note "nest was built from a different commit than HEAD — rebuild (cargo build -p nest) if the tree moved" ;;
esac
echo

# One command, capped, timed, with its output shown; records ok/FAIL.
run_step() {
  local name=$1; shift
  local start end
  start=$(date +%s)
  echo "-- $name"
  ( ulimit -v "$SMOKE_ULIMIT_KB"; cd "$BEDIT_DIR" && "$@" )
  local status=$?
  end=$(date +%s)
  if [ "$status" = 0 ]; then
    ok "$name ($((end - start)) s)"
  else
    red "$name (exit $status, $((end - start)) s)"
  fi
  echo
}

# The order is fast-and-precise first: a rename that `check` can see fails in seconds and
# names the site; the suite would fail on the same thing minutes later and less clearly.
run_step "nest check (zero warnings)" "$NEST" check
run_step "nest run --check-boot (headless)" env BROOD_GUI_HEADLESS=1 "$NEST" run --check-boot
run_step "nest test" env BROOD_GUI_HEADLESS=1 "$NEST" test

if [ "$fail" = 0 ]; then
  echo "smoke-bedit: green — bedit @ $bedit_sha checks, boots and tests against $nest_version"
  exit 0
fi
echo "smoke-bedit: $fail of 3 gates FAILED (bedit @ $bedit_sha, $nest_version)"
exit 1
