#!/usr/bin/env bash
# The wasm32 BEHAVIOURAL suite — run the cooperative scheduler, don't just compile it.
#
# WHY THIS EXISTS
#   CI's wasm job is "build only, deliberately: there is no wasm test runner here", so an
#   entire alternate scheduler — `pump_until_quiescent`, the frozen logical clock
#   (`timer::sched_now`), `fire_next_timer`, and the non-blocking park — is compiled on
#   every run and executed on none. The native suite cannot cover it by construction: off
#   wasm there are OS worker threads and `sched_now()` IS `Instant::now()`, so every gate
#   agrees and a native run is green whether the wasm path works or not. That is the same
#   shape as the hole `%gui-compiled?` shipped through — a host build cannot see a hole in
#   a cfg surface it never compiles.
#
#   `sched_clock_domain.rs` guards the clock as SOURCE TEXT (it greps the gate), and
#   `wasm-receive-timeout-repro.sh` checks one behaviour BY HAND. This runs the behaviour,
#   automatically, over the mechanisms the cooperative scheduler is made of.
#
# REQUIREMENTS   rustup target add wasm32-unknown-unknown; cargo install wasm-bindgen-cli
#                --version 0.2.100 (must match crates/playground); node
# USAGE          scripts/wasm-suite.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${OUT:-$ROOT/target/wasm-suite}"
LIMIT="${LIMIT:-120}"

# `wasm-bindgen` is installed into ~/.cargo/bin, which a non-login shell may not have.
export PATH="$HOME/.cargo/bin:$PATH"
command -v node >/dev/null || { echo "wasm-suite: node not found"; exit 2; }
command -v wasm-bindgen >/dev/null || {
  echo "wasm-suite: wasm-bindgen CLI not found (cargo install wasm-bindgen-cli --version 0.2.100)"; exit 2; }
rustup target list --installed | grep -qx wasm32-unknown-unknown \
  || { echo "wasm-suite: wasm32-unknown-unknown target not installed"; exit 2; }

echo "== building crates/playground for wasm32 (profile release-wasm) =="
# The size-optimized profile the site actually serves — building `--release` here would
# exercise a different binary than the one that ships.
cargo build --profile release-wasm -p brood-playground --target wasm32-unknown-unknown

mkdir -p "$OUT"
wasm-bindgen --target nodejs --out-dir "$OUT/pkg" \
  "$ROOT/target/wasm32-unknown-unknown/release-wasm/brood_playground.wasm"

cp "$ROOT/scripts/wasm-suite.cjs" "$OUT/suite.cjs"
echo "== running the behavioural suite (limit ${LIMIT}s) =="
# A HANG is the signature failure of this scheduler (a park/timeout mismatch spins the
# pump forever — 100% CPU, frozen tab), so it must read as a distinct verdict rather than
# as a bare non-zero exit. Silence is not success.
set +e
timeout "$LIMIT" node "$OUT/suite.cjs"
rc=$?
set -e
case "$rc" in
  0) ;;
  124) echo
       echo "TIMED OUT after ${LIMIT}s — this is the cooperative scheduler HANGING, not a"
       echo "slow machine. The usual cause is a park/receive gate reading a different clock"
       echo "than the deadline was minted on, so the pump never idles and never reaches"
       echo "fire_next_timer. See scripts/wasm-receive-timeout-repro.sh and"
       echo "crates/lisp/tests/sched_clock_domain.rs."
       exit 1 ;;
  *) exit "$rc" ;;
esac
