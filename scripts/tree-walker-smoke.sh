#!/usr/bin/env bash
# The per-push tree-walker smoke: a few test files through `BROOD_TIER=0`.
#
# The whole suite through the tree-walker (ADR-076) takes ~25 minutes on CI, so it runs in
# full.yml, weekly and on dispatch. That left the tree-walker unseen on every push: KI-399
# (a contract shim's helpers indistinguishable from user code, tree-walker only) passed
# per-push CI and surfaced days later in `make test-both`. This runs the files where
# tree-walker-only failures have shown up before, plus the core forms, in a couple of
# minutes. When a failure turns up only under `BROOD_TIER=0`, add the file that caught it.
#
# Passes only on a zero exit AND a summary line reporting no failure: a runner that died
# before printing one is not a pass.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
. "$ROOT/scripts/lib/run-cap.sh"
NEST="${NEST:-$ROOT/target/release/nest}"
[ -x "$NEST" ] || { echo "tree-walker-smoke: no nest at $NEST — build with 'cargo build --release -p cli -p nest' (or set NEST=)"; exit 2; }

files=(
  tests/contract_dispatch_test.blsp      # KI-399
  tests/concurrency_test.blsp
  tests/registry_test.blsp
  tests/linmap_soundness_test.blsp
  tests/isolate_load_test.blsp
  tests/jit_pair_predicate_test.blsp
  tests/vm_prim_error_pos_test.blsp
  tests/dynamic_test.blsp
  tests/fn_heads_test.blsp
  tests/let_capture_shadow_test.blsp
  tests/multimethod_test.blsp
  tests/prelude_capture_test.blsp
  tests/quasiquote_dotted_tail_test.blsp
  tests/try_catch_test.blsp
  tests/macroexpand_test.blsp
)

cd "$ROOT"
# Not vacuous: if `BROOD_TIER=0` stopped reaching the engine, this would quietly be a second
# VM run of files the suite already covers.
engine="$( ( ulimit -v "$RUN_CAP_KB"; BROOD_TIER=0 "$NEST" eval '(%tree-walker?)' ) 2>/dev/null | tail -1)"
[ "$engine" = "true" ] || { echo "tree-walker-smoke: BROOD_TIER=0 did not select the tree-walker (read: ${engine:-nothing})"; exit 1; }

log="$(mktemp -t brood-tree-walker-smoke.XXXXXX)"
( ulimit -v "$RUN_CAP_KB"; BROOD_TIER=0 "$NEST" test "${files[@]}" ) >"$log" 2>&1
status=$?
summary="$(grep -E '^[0-9]+ tests, [0-9]+ passed, [0-9]+ failed' "$log" | tail -1)"
if [ $status -eq 0 ] && [ -n "$summary" ] && printf '%s\n' "$summary" | grep -q ', 0 failed'; then
  echo "tree-walker-smoke: $summary (BROOD_TIER=0, ${#files[@]} files)"
  rm -f "$log"
  exit 0
fi
echo "tree-walker-smoke: FAILED (exit $status) — ${summary:-no summary line: the runner died}"
records="$(sed 's/\x1b\[[0-9;]*m//g' "$log" | awk '/test failed: /{on=1} on{print} on&&/^[[:space:]]*$/{on=0}' | head -60)"
if [ -n "$records" ]; then
  printf '%s\n' "$records" | sed 's/^/    /'
else
  grep -E 'error:' "$log" | grep -v ' died: ' | head -20 | sed 's/^/    /'
fi
rm -f "$log"
exit 1
