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
  # WHICH binary: `make release` builds RELEASE_DIR=target/release-fast; only a plain
  # `cargo build --release` writes target/release. This gated on target/release while
  # telling you to run `make release`, which never touches it — so it ran a binary no
  # documented command refreshes. On 2026-08-28 that binary was 9 commits behind and
  # reported two phantom `unbound symbol` failures (a pre-rename std/ baked into it).
  # Prefer whichever candidate reports HEAD's sha.
  head_sha=$(git rev-parse --short HEAD 2>/dev/null || echo '?')
  binary_sha() { "$1" --version 2>/dev/null | sed -n 's/.*(\([0-9a-f]\{7,\}\)).*/\1/p'; }
  nest=""
  for cand in "$root/target/release-fast/nest" "$root/target/release/nest"; do
    [ -x "$cand" ] || continue
    [ -n "$nest" ] || nest="$cand"                                   # first that exists
    if [ "$(binary_sha "$cand")" = "$head_sha" ]; then nest="$cand"; break; fi
  done

  # Is the binary's std/ the tree's std/? A binary from a DIFFERENT commit is still valid if
  # nothing it bakes in changed since — std/ is include_str!'d and crates/ *is* the binary — so
  # when `git diff` over those two paths is empty between the binary's commit and HEAD, its
  # verdict is current. Without this exemption every docs-only commit refuses the gate, and the
  # reflex becomes to stop believing it: the same trap the pre-push hook records for a stale
  # formatter ("a stale gate that refuses correct work is worse than no gate").
  bin_sha=$(binary_sha "$nest" 2>/dev/null)
  stale=0
  equiv=""
  if [ -n "$nest" ] && [ "$bin_sha" != "$head_sha" ]; then
    if [ -n "$bin_sha" ] && git cat-file -e "${bin_sha}^{commit}" 2>/dev/null &&
       [ -z "$(git diff --name-only "$bin_sha" HEAD -- std crates 2>/dev/null)" ]; then
      equiv="built at $bin_sha, not HEAD ($head_sha) — but std/ and crates/ are byte-identical between them"
    else
      stale=1
    fi
  fi

  # A stale binary's verdict is meaningless in BOTH directions, so a stale one is a FAILURE,
  # not a note: it must not be possible to read a green — or a red — off the wrong std/. The
  # old guard only fired when std/ or crates/ had UNCOMMITTED changes, i.e. never on the clean
  # tree you have right before a push, which is exactly when this gate gets consulted.
  if [ -z "$nest" ]; then
    note "no nest in target/release-fast or target/release — run \`make release\`; skipping the .blsp gates"
  elif [ "$stale" = 1 ]; then
    red "the .blsp gates DID NOT RUN — $nest is built from ${bin_sha:-an unknown commit}, HEAD is $head_sha"
    note "  rebuild with \`make release\`, then re-run. A stale binary bakes in a stale std/."
  elif newer=$(find std crates \( -name '*.blsp' -o -name '*.rs' \) -newer "$nest" -print -quit 2>/dev/null);
       [ -n "$newer" ]; then
    red "the .blsp gates DID NOT RUN — $newer is newer than $nest (uncommitted work)"
    note "  rebuild with \`make release\`, then re-run."
  else
    [ -n "$equiv" ] && note "$nest is $equiv, so the two gates below are current"


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
