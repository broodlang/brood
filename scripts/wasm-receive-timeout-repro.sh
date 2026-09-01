#!/usr/bin/env bash
# Reproduce (and check the fix for) the wasm receive-timeout clock mismatch.
#
# WHAT IT PINS
#   On wasm32 there is no timer thread, so `process::timer::sched_now()` is a FROZEN
#   LOGICAL clock that only `fire_next_timer` advances. A `receive` deadline is minted
#   as `sched_now() + ms`, so every gate that asks "has it elapsed?" must read the same
#   clock. When `park_on_receive` read real time instead, a snippet that computed for
#   longer than `ms` before reaching its `receive` got: park gate says "passed" →
#   re-queue → re-scan gate (`sched_now() >= d`) says "not yet" → suspend → park →
#   forever. `pump_until_quiescent` saw `ran_any = true` every sweep, so it never fell
#   through to `fire_next_timer` — the one thing that could advance the logical clock.
#   100% CPU, frozen browser tab.
#
#   NOTHING IN `make test` CAN CATCH THIS: off wasm, `sched_now()` *is* `Instant::now()`,
#   so both gates agree and a native run is green either way. The permanent guard is the
#   source-text test `crates/lisp/tests/sched_clock_domain.rs`; this script is the
#   behavioural check, run by hand when the scheduler's park/timeout path changes.
#
# REQUIREMENTS
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.100   # must match crates/playground
#   node
#
# USAGE
#   scripts/wasm-receive-timeout-repro.sh            # build + run, expect PASS
#   BURN=100000000 scripts/wasm-receive-timeout-repro.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${OUT:-$ROOT/target/wasm-repro}"
# Iterations of the burn loop. It must take longer in REAL time than the 1000 ms
# receive timeout below, or the bug does not arm (the whole point is real time
# outrunning frozen logical time). ~50M is ~2 s on a 2026-era laptop.
BURN="${BURN:-50000000}"
# Seconds to allow. The fixed build finishes in about the burn time; the broken one
# never finishes, so anything comfortably above the burn is a decisive verdict.
LIMIT="${LIMIT:-90}"

command -v node >/dev/null || { echo "node not found"; exit 2; }
command -v wasm-bindgen >/dev/null || { echo "wasm-bindgen CLI not found (cargo install wasm-bindgen-cli --version 0.2.100)"; exit 2; }
rustup target list --installed | grep -qx wasm32-unknown-unknown \
  || { echo "wasm32-unknown-unknown target not installed (rustup target add wasm32-unknown-unknown)"; exit 2; }

echo "== building crates/playground for wasm32 =="
# `--profile release-wasm` (workspace root): the size-optimized profile the shipped wasm
# uses. Building `--release` here would exercise a different binary than the site serves.
cargo build --profile release-wasm -p brood-playground --target wasm32-unknown-unknown

mkdir -p "$OUT"
wasm-bindgen --target nodejs --out-dir "$OUT/pkg" \
  "$ROOT/target/wasm32-unknown-unknown/release-wasm/brood_playground.wasm"

cat > "$OUT/repro.js" <<'JS'
const p = require("./pkg/brood_playground.js");
const burn = process.env.BURN;
// 1. A first receive-timeout freezes the logical clock and then advances it to that
//    deadline (fire_next_timer). 2. The burn spends REAL time without spending logical
//    time. 3. The second receive's deadline is in the logical near-future but already
//    in the real past — the exact window where the two clocks disagree.
const src = `
(defn burn (n acc) (if (= n 0) acc (burn (- n 1) (+ acc 7))))
(let (a (receive (after 1 :first))
      b (burn ${burn} 0)
      c (receive (after 1000 :second)))
  (list a c))
`;
const t0 = Date.now();
const out = p.run(src);
console.log(`result=${JSON.stringify(out)} wall=${Date.now() - t0}ms`);
process.exit(out.includes(":first") && out.includes(":second") ? 0 : 1);
JS

echo "== running (burn=$BURN, limit=${LIMIT}s) =="
set +e
BURN="$BURN" timeout "$LIMIT" node "$OUT/repro.js"
rc=$?
set -e

case "$rc" in
  0) echo "PASS — the (after 1000 …) clause fired; both gates read the same clock." ;;
  124) echo "FAIL — timed out after ${LIMIT}s. This is the clock mismatch: the pump is"
       echo "       spinning (check ~100% CPU) because park_on_receive and the receive"
       echo "       re-scan disagree about whether the deadline has passed."
       exit 1 ;;
  *) echo "FAIL — the snippet did not produce (:first :second) (exit $rc)."; exit 1 ;;
esac

# Control: with a burn SHORTER than the timeout, real and logical time still agree, so
# even a broken build terminates. A run where the main case fails and this one passes is
# what identifies the clock mismatch rather than the build or the harness.
echo "== control: short burn (real elapsed < the timeout) =="
BURN=2000000 timeout 60 node "$OUT/repro.js"
