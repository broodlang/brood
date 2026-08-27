#!/usr/bin/env bash
# `make green` — answer ONE question honestly: is this tree green?
#
# It exists because that question was answered wrongly for two days (KI-68/KI-69, 2026-08-27),
# in two different ways, and neither was a lie anyone told — both were the obvious reading of
# what the tools printed.
#
#   1. THE RUN LIST DOES NOT SHOW RED. Every CI run is cancelled by the next push to the same
#      ref (the workflow's `concurrency` group), so a day with several pushes shows a wall of
#      `cancelled` with one `in_progress` on top. `gh run list` looked fine while the last
#      three COMPLETED runs had all failed. A cancelled run is not evidence of anything, so
#      this script filters to completed runs and reports only those.
#
#   2. `make check` IS NOT WHAT CI RUNS. It is clippy + tests. CI additionally runs
#      `nest format --check`, the zero-warning checker gate over std/ + tests/, the examples
#      gate and the stress gate — four gates a developer can be entirely green without. v0.14.0
#      was tagged and pushed with three .blsp files failing `nest format --check` for exactly
#      this reason.
#
# So: the remote half (what CI actually concluded) and the local half (the gates CI runs that
# `make check` does not), in one place, with no reading between the lines.
#
# This does NOT replace `make test` — the suite is the slow half and is left to CI or to an
# explicit run. `--local` skips the remote half (offline, or no `gh`); `--remote` skips the
# local half. Exit status is 0 only if everything checked passed.
#
# See also `make doctor`, which reports the things that make a gate LIE (a stale binary agrees
# with its baseline; a cold boot cache reads as a regression). Run that when a result surprises
# you; run this to find out whether there is a result at all.
set -u

do_local=1
do_remote=1
do_clippy=0
for arg in "$@"; do
  case "$arg" in
    --local)  do_remote=0 ;;
    --remote) do_local=0 ;;
    # Off by default because it is a full --all-features build (minutes, not seconds) and
    # every other check here is seconds. On in `make green-all`.
    --clippy) do_clippy=1 ;;
    *) echo "usage: $0 [--local | --remote] [--clippy]" >&2; exit 2 ;;
  esac
done

# The workflow that actually gates the tree. `Release` also runs here and only builds
# binaries; it succeeds on commits whose CI failed, so it must never be read as green.
WORKFLOW=${WORKFLOW:-CI}

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root" || exit 2

fail=0
no_verdict=0   # set when CI has concluded nothing recent — green locally is then not green
red()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; fail=$((fail+1)); }
ok()   { printf '  \033[32mok\033[0m   %s\n' "$1"; }
note() { printf '  \033[33m!\033[0m    %s\n' "$1"; }

# ── remote: what CI actually concluded ───────────────────────────────────────────────────
# Only `success` and `failure` count. `cancelled` means a newer push superseded the run before
# it finished — it says nothing about the tree, and treating it as "not red" is the mistake.
if [ "$do_remote" = 1 ]; then
  echo "== $WORKFLOW workflow, completed runs only — cancelled runs are not evidence =="
  if ! command -v gh >/dev/null 2>&1; then
    note "gh not installed — skipping the remote half (use --local to silence this)"
  elif ! gh auth status >/dev/null 2>&1; then
    note "gh not authenticated (\`gh auth login\`) — skipping the remote half"
  else
    # Scoped to ONE workflow on purpose. `gh run list` mixes them, and this repo also runs a
    # `Release` workflow that only builds binaries — it goes green on a commit whose CI run
    # failed, so an unscoped list shows a reassuring `ok` that is about something else
    # entirely. That is the same reading error the whole script exists to prevent.
    rows=$(gh run list --workflow "$WORKFLOW" --limit 40 \
             --json conclusion,headSha,displayTitle,createdAt,databaseId \
             -q '.[] | select(.conclusion=="success" or .conclusion=="failure")
                 | "\(.conclusion)\t\(.headSha[0:8])\t\(.databaseId)\t\(.createdAt[0:16])\t\(.displayTitle[0:52])"' \
           2>/dev/null | head -5)
    if [ -z "$rows" ]; then
      no_verdict=1
      note "no completed $WORKFLOW run in the last 40 — every one was cancelled or is still going."
      note "that is the KI-68/KI-69 shape: stop pushing and let one finish before believing green."
    else
      newest=$(printf '%s\n' "$rows" | head -1 | cut -f1)
      printf '%s\n' "$rows" | while IFS=$'\t' read -r concl sha id when title; do
        if [ "$concl" = "success" ]; then
          printf '  \033[32mok\033[0m   %s  %s  %s\n' "$sha" "$when" "$title"
        else
          printf '  \033[31mFAIL\033[0m %s  %s  %s\n' "$sha" "$when" "$title"
          printf '       gh run view %s --log-failed\n' "$id"
        fi
      done
      [ "$newest" = "failure" ] && fail=$((fail+1))
    fi

    # A green run on a commit that is not the tip proves nothing about the tip.
    if git rev-parse --verify -q origin/main >/dev/null 2>&1; then
      tip=$(git rev-parse --short=8 origin/main)
      newest_sha=$(printf '%s\n' "$rows" | head -1 | cut -f2)
      if [ -n "$newest_sha" ] && [ "$newest_sha" != "$tip" ]; then
        note "newest completed run is $newest_sha, but origin/main is at $tip — that run does not cover the tip"
      fi
    fi
    local_ahead=$(git rev-list --count origin/main..HEAD 2>/dev/null || echo 0)
    [ "${local_ahead:-0}" -gt 0 ] && note "HEAD is $local_ahead commit(s) ahead of origin/main — CI has not seen them"
  fi
  echo
fi

# ── local: the gates CI runs that `make check` does not ──────────────────────────────────
if [ "$do_local" = 1 ]; then
  echo "== local gates (the ones \`make check\` skips) =="
  nest="$root/target/release/nest"
  if [ ! -x "$nest" ]; then
    note "target/release/nest missing — run \`make release\` first; skipping the .blsp gates"
  else
    # Built from the same tree? A stale binary passes by agreeing with itself (see `make doctor`).
    if [ -n "$(git status --porcelain -- std crates 2>/dev/null)" ] &&
       [ "$nest" -ot "$(git diff --name-only -- std crates 2>/dev/null | head -1)" ] 2>/dev/null; then
      note "std/ or crates/ changed after target/release/nest was built — rebuild before believing these"
    fi

    if "$nest" format --check >/dev/null 2>&1; then
      ok "nest format --check"
    else
      red "nest format --check  →  run \`$nest format\`"
      "$nest" format --check 2>&1 | grep "needs formatting" | head -5 | sed 's/^/       /'
    fi

    # The checker's one hard reject, batch-only by design (ADR-123/124/125/126). Held at zero
    # since 2026-07-31; a warning here is a real finding or a missing justified `check-allow`.
    if (shopt -s globstar nullglob; "$nest" check std/**/*.blsp tests/**/*.blsp >/dev/null 2>&1); then
      ok "nest check std/ + tests/ (zero warnings)"
    else
      red "nest check std/ + tests/"
      (shopt -s globstar nullglob; "$nest" check std/**/*.blsp tests/**/*.blsp 2>&1) |
        grep "warning:" | head -8 | sed 's/^/       /'
    fi
  fi

  if cargo fmt --all --check >/dev/null 2>&1; then ok "cargo fmt --all --check"
  else red "cargo fmt --all --check  →  run \`make fmt\`"; fi

  # `--all-features` is the load-bearing half, not a thoroughness flourish: without it the
  # lint set is smaller, and a plain `cargo clippy --all-targets` passes on code CI rejects.
  # That is not hypothetical — it happened twice on 2026-08-27, once to a locally-verified
  # commit and once to the boot-cache prune (`unnecessary_sort_by`), where the Clippy step
  # failing SKIPPED every step behind it, so the tests and the checker gate never ran at all.
  if [ "$do_clippy" = 1 ]; then
    if cargo clippy --all-targets --all-features -- -D warnings >/dev/null 2>&1; then
      ok "cargo clippy --all-targets --all-features -- -D warnings"
    else
      red "cargo clippy (CI flags)"
      cargo clippy --all-targets --all-features -- -D warnings 2>&1 |
        grep -E "^error" | head -5 | sed 's/^/       /'
    fi
  fi

  # Cheap and catches a whole class: a KI/ADR cited with no entry, or two entries claiming one
  # number (which happened on 2026-08-27 and was invisible because `defined()` used a set).
  if cargo test -q -p brood --test doc_refs >/dev/null 2>&1; then ok "doc_refs (KI/ADR references + no duplicate numbers)"
  else red "doc_refs  →  cargo test -p brood --test doc_refs"; fi

  echo
  if [ "$do_clippy" = 0 ]; then
    note "clippy NOT run (add --clippy, or use \`make green-all\`). CI's exact invocation is"
    note "  cargo clippy --all-targets --all-features -- -D warnings"
    note "and a plain \`cargo clippy --all-targets\` does NOT catch what it catches."
  fi
  note "not run here (slow — CI or an explicit run): make test, the breakage suite, and the"
  note "tree-walker differential. \`make green-all\` adds clippy, examples and stress."
  echo
fi

if [ "$fail" -eq 0 ]; then
  if [ "$no_verdict" = 1 ]; then
    # Deliberately not the word "green". Local gates passing while CI has concluded nothing is
    # precisely the state that read as green for two days.
    printf '\033[33mno verdict\033[0m — the local gates passed, but CI has not finished a run.\n'
    printf '            Let one complete before calling this green.\n'
    exit 0
  fi
  printf '\033[32mgreen\033[0m — everything checked here passed. Note what was NOT checked, above.\n'
  exit 0
fi
printf '\033[31mNOT green\033[0m — %d check(s) failed.\n' "$fail"
exit 1
