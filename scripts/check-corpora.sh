#!/usr/bin/env bash
# `make check-corpora` — STATICALLY check every Brood tree outside :source-paths for names
# that do not resolve.
#
# The trees here (examples/, stress/, scripts/fuzz/stress/, breakage/) already have gates —
# `make check-examples`, `make check-stress`, `make breakagetests` — but all three work by
# RUNNING the programs and failing on `unbound symbol`. That only ever catches a dead name on
# an EXECUTED path, and these corpora are full of branches a given run does not take: an
# `(if ...)` arm, a `catch` body, a case behind a `when`, a helper the harness only calls for
# one size of input.
#
# The difference is not theoretical. On 2026-08-27, with all three runtime gates green, a
# static pass over the same files found **74** unresolvable names: 31 `rem`, 11 `quot`,
# `min`/`max`/`floor`/`->fixed`, `whereis`, `read-string`, and the whole `table-*` family in
# `stress/check_corpus/` — every one a rename wave nobody had followed here, sitting on a path
# no run had taken. That is the KI-68 shape again (a gate that passes because it never looked)
# and this closes it.
#
# The bar is **zero `unbound symbol`**, not zero warnings — deliberately. These corpora exist
# to be adversarial: `breakage/` tries to break the runtime, `stress/check_corpus/` feeds the
# checker programs that must run clean, and style lints on generated or deliberately-awful
# code are noise. An unbound name is never noise: the file names something that does not
# exist, so it is either rot or it is dynamic. If it is genuinely dynamic (a global created by
# `eval` or `system/reload-defs` — see `chaos_eval_wormhole.blsp`), say so in the file with
# `(check-allow :unbound …)` rather than leaving the gate to guess.
set -u

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root" || exit 2

# WHICH binary — see `scripts/lib/gate-binary.sh`. This gated on `target/release/nest`
# while `make release` writes `target/release-fast`, so the command its error named could
# not fix it (KI-76's class).
ROOT="$root"
. "$root/scripts/lib/gate-binary.sh"
NEST=${NEST:-$(gate_pick nest)}
gate_require_fresh "$NEST"

shopt -s globstar nullglob

# Each tree checked separately: a name resolves against the files loaded WITH it, so a
# per-tree pass keeps one corpus from accidentally satisfying another's references — and it
# makes the output say which tree is rotten.
trees="examples stress scripts/fuzz/stress breakage"
fail=0

for tree in $trees; do
  files=("$tree"/**/*.blsp)
  if [ "${#files[@]}" -eq 0 ]; then
    echo "  -- $tree (no .blsp files — did the tree move?)"
    continue
  fi
  out=$("$NEST" check "${files[@]}" 2>&1 | grep "unbound symbol" || true)
  if [ -z "$out" ]; then
    printf '  \033[32mok\033[0m   %-22s %d files\n' "$tree" "${#files[@]}"
  else
    n=$(printf '%s\n' "$out" | wc -l | tr -d ' ')
    printf '  \033[31mFAIL\033[0m %-22s %d unresolved name(s)\n' "$tree" "$n"
    printf '%s\n' "$out" | sed "s#$root/##" | head -20 | sed 's/^/       /'
    [ "$n" -gt 20 ] && printf '       … and %d more\n' "$((n - 20))"
    fail=$((fail + 1))
  fi
done

echo
if [ "$fail" -eq 0 ]; then
  printf '\033[32mcheck-corpora: every name resolves\033[0m\n'
  exit 0
fi
cat <<'MSG'
check-corpora: a name above does not resolve. Either it is rot from a rename wave — fix the
call site — or the global is created at runtime by `eval` / `system/reload-defs`, in which case
wrap the use in `(check-allow :unbound …)` with a comment saying why, as
`breakage/chaos_eval_wormhole.blsp` and `scripts/fuzz/stress/eval_forward_ref.blsp` do.

Note the runtime gates (`make check-examples`, `make check-stress`, `make breakagetests`) can
all be green while this is red: they only see names on paths a run actually takes.
MSG
exit 1
