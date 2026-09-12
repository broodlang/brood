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
#
# THE PIN. CI runs bedit at `BEDIT_REF` (.github/workflows/ci.yml), not at whatever
# ../bedit is checked out at — so a green run here proves nothing about CI unless the two
# agree. The script says so up front, and `--bump` closes the gap: after a green run it
# rewrites `BEDIT_REF` to bedit's HEAD, refusing a HEAD that is not on bedit's origin
# (CI cannot fetch an unpushed commit) or a dirty checkout (the commit is not what ran).
# The rule the pin's comment states — "bump it in the SAME brood commit that lands the
# change" — became a step somebody had to remember after ADR-325's rename wave; now it
# is `make smoke-bedit ARGS=--bump`.
set -u

root=$(cd "$(dirname "$0")/.." && pwd)
require=0
bump=0
for arg in "$@"; do
  case "$arg" in
    --require) require=1 ;;
    --bump) bump=1 ;;
    *) echo "usage: $0 [--require] [--bump]" >&2; exit 2 ;;
  esac
done
ci_yml=$root/.github/workflows/ci.yml

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

# bedit's end-to-end tests (apprun/testrun/procstream) spawn `nest` by NAME, so the
# resolved binary must be what a bare `nest` finds — on CI nothing is installed and
# without this the fixtures die with "proc-spawn nest: No such file or directory".
export PATH="$(cd "$(dirname "$NEST")" && pwd):$PATH"

nest_version=$("$NEST" --version 2>/dev/null || echo "?")
tree_sha=$(git -C "$root" rev-parse --short HEAD 2>/dev/null || echo "?")
bedit_sha=$(git -C "$BEDIT_DIR" rev-parse --short HEAD 2>/dev/null || echo "?")
bedit_dirty=$(git -C "$BEDIT_DIR" status --porcelain 2>/dev/null | wc -l)
echo "  nest:  $NEST ($nest_version, built $(date -r "$NEST" '+%Y-%m-%d %H:%M'))"
echo "  brood: HEAD $tree_sha"
echo "  bedit: $BEDIT_DIR @ $bedit_sha ($bedit_dirty modified files)"
# Is this the bedit CI will run? A green smoke against a different commit is a fact about
# the wrong bedit.
pinned=$(sed -n 's/^  BEDIT_REF: \([0-9a-f]*\).*/\1/p' "$ci_yml" | head -1)
bedit_full=$(git -C "$BEDIT_DIR" rev-parse HEAD 2>/dev/null || echo "?")
if [ -n "$pinned" ] && [ "$pinned" != "$bedit_full" ]; then
  note "CI pins bedit @ ${pinned:0:8} (BEDIT_REF), not the $bedit_sha checked out here — pass --bump after a green run to move the pin"
fi
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
  if [ "$bump" = 1 ]; then
    if [ "$bedit_dirty" != 0 ]; then
      red "--bump refused: bedit has $bedit_dirty modified file(s); the commit is not what just ran"
      exit 1
    fi
    if ! git -C "$BEDIT_DIR" fetch -q origin 2>/dev/null || ! git -C "$BEDIT_DIR" merge-base --is-ancestor "$bedit_full" origin/main 2>/dev/null; then
      red "--bump refused: bedit $bedit_sha is not on origin/main — push it first, CI cannot fetch an unpushed commit"
      exit 1
    fi
    if [ "$pinned" = "$bedit_full" ]; then
      ok "BEDIT_REF already pins $bedit_sha"
    else
      sed -i "s/^  BEDIT_REF: [0-9a-f]*/  BEDIT_REF: $bedit_full/" "$ci_yml"
      ok "BEDIT_REF ${pinned:0:8} -> $bedit_sha in $(realpath --relative-to="$root" "$ci_yml") — commit it with the change bedit adopted"
    fi
  fi
  exit 0
fi
echo "smoke-bedit: $fail of 3 gates FAILED (bedit @ $bedit_sha, $nest_version)"
exit 1
