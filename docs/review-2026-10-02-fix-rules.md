# Shared rules for the fix agents (read fully before starting)

Repo: /home/wilhelm/src/broodlang/brood (branch main, HEAD 3f3e7732). Read its CLAUDE.md
sections "Green tree first", "Build uncapped, run capped", "When you add a feature" (esp.
step 3, SABOTAGE every guard), "Conventions & invariants", and the global rules:
never abbreviate variable names; never use Python; no git history/state commands.

## You share ONE working tree and ONE target/ with up to three other agents

Disk is tight (~24 GB free), so there are no worktrees. Consequences:

1. **Edit only the files you own** (listed in your prompt). If a fix genuinely needs a
   change in a file you do not own, do NOT edit it — put the exact change you need in
   your final report under "Needs from other owners" and work around it or skip that item.
2. **Never run `cargo fmt`, `make fmt`, or `nest format` without file arguments** — they
   rewrite other agents' files mid-edit. Format only your own files:
   `rustfmt --edition 2021 <file.rs>…` and `target/debug/nest format <file.blsp>…`
   (check what `nest format --help` accepts; if it only formats a whole project, run
   `nest format --check` and fix your own files by hand).
3. **Keep the tree compiling between your edits.** Make each change self-consistent
   quickly. Builds serialize on cargo's lock. If a build fails with errors in a file you
   do NOT own, another agent is mid-edit: wait ~60 s and retry (up to ~10 min), never
   "fix" their file.
4. **Build commands** (uncapped — the linker dies under the cap):
   `cargo build --bin brood --bin nest` and, for Rust unit tests,
   `cargo nextest run -p brood --no-run`. Then RUN capped:
   `( ulimit -v 16000000; cargo nextest run -p brood -j1 -E 'test(<filter>)' )`,
   `( ulimit -v 16000000; target/debug/brood --check f.blsp )`,
   `( ulimit -v 16000000; target/debug/brood --test tests/foo_test.blsp )`.
   std/*.blsp is baked into the binary: after editing std you MUST rebuild before
   `nest check`/`brood --check` (they refuse a stale binary, exit 2).
5. **Do not edit shared narrative docs**: docs/devlog.md, docs/known-issues.md,
   docs/decisions.md, docs/type-system-status.md, docs/handoff.md, ROADMAP.md, CLAUDE.md.
   Instead, put in your final report a ready-to-paste devlog paragraph (dated
   2026-10-02) and any ADR/status-doc correction text. The coordinator integrates them.
6. **No commits, no pushes, no git stash/reset/checkout/restore/clean.**
7. Scratch files go under
   /tmp/claude-1000/-home-wilhelm-src-broodlang-brood/92491eff-0fcd-4dd4-a1f2-077e1b25885f/scratchpad/
   (tmpfs — never put a cargo target dir there).

## Definition of done for each item

- The repro (paths given in your prompt, all under the scratchpad) now gives the right
  verdict, and you ran it.
- A regression test exists at the entry point a caller reaches (a `check/tests/*.rs`
  case, a `types/tests.rs` case, or a `tests/*_test.blsp` case), and you SABOTAGED it:
  reverted the fix, watched the test go red, restored. Say so in the report.
- No new warning in the zero-warning gates. Run before you report (globstar matters —
  plain bash does not expand `**` without it):
  ```
  shopt -s globstar nullglob
  ( ulimit -v 16000000; target/debug/nest check std/**/*.blsp tests/**/*.blsp examples/**/*.blsp )
  ( ulimit -v 16000000; target/debug/nest check --strict std/**/*.blsp tests/**/*.blsp )
  ```
  The baseline output of both at HEAD was ONLY `note: checker gave up` lines (advisory)
  and exit 0. Any `warning:` you introduce is yours to resolve (a checker false positive
  you created, or a real finding in std/tests — fix the code, don't silence it, unless the
  test is deliberately exercising a failure, then `check-allow`).
- The type-system unit suite stays green:
  `( ulimit -v 16000000; cargo nextest run -p brood -j1 -E 'test(types::)' )`.
- If an item turns out NOT to be a defect after all (e.g. the documented design says
  otherwise), do not force it: report why, with evidence.

## Final report format

Per item: status (fixed / partially / not-a-bug / blocked), the files changed, the test
that guards it and the sabotage result, one line on the approach. Then: "Needs from other
owners", devlog paragraph, doc corrections, and the exact gate commands you ran with their
final lines.
