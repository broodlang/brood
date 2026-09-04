#!/usr/bin/env bash
# Does the project's own checker gate stay green with the PRELUDE IMAGE on?
#
# Why this exists. On 2026-09-04 the prelude image (ADR-314) was made the default, and every
# test written FOR the image passed — the boot differential, the stale-directory guard, all
# 1377 suite cases. `nest check` over `std/ + tests/ + examples/` went red on the first run:
# with the image on, a multi-file check lost every DERIVED multimethod mirror (KI-106). The
# feature's tests were shaped by what its authors thought could break; the project's gates
# were not, and they found it at once. So this runs the checker gate UNDER the flag.
#
# Two things it must do or it proves nothing:
#   1. Warm `nest`'s OWN prelude image first — images are per-binary (build-id keyed), and a
#      cold nest takes the source path where the bug does not exist. Proven the hard way: a
#      first sabotage run read 0 warnings because nest had never booted under the flag.
#   2. ASSERT the boot actually took the image path (BROOD_BOOT_TRACE says `(prelude image)`).
#      A stale or absent artifact falls back to the text cache silently, and a gate that ran
#      the source path and printed "clean" has answered a question nobody asked.
#
# A private XDG_CACHE_HOME so the machine's cache state cannot decide the answer.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
. "$ROOT/scripts/lib/gate-binary.sh"
NEST="${NEST:-$(gate_pick nest)}"
[ -x "$NEST" ] || { echo "check-imaged: no nest at $NEST — run \`make release\`"; exit 2; }

WORK="$(mktemp -d)" || exit 2
trap 'rm -rf "$WORK"' EXIT
export XDG_CACHE_HOME="$WORK/cache"
export BROOD_PRELUDE_IMAGE=1
export BROOD_NO_CHECK_CACHE=1
mkdir -p "$XDG_CACHE_HOME"

# The stdlib image, so `require` takes the imaged path too (the configuration users get).
"$NEST" stdimage >/dev/null 2>&1 || true
# Boot nest once under the flag: the cold source boot that WRITES nest's prelude image.
"$NEST" complete -- >/dev/null 2>&1 || true
# Prove the next boot is the imaged one.
trace="$(BROOD_BOOT_TRACE=1 "$NEST" complete -- 2>&1 >/dev/null | grep '^\[boot\]' || true)"
case "$trace" in
  *"(prelude image)"*) ;;
  *) echo "check-imaged: nest did NOT boot from the prelude image, so this gate would test the source path. Boot trace:"; echo "$trace" | sed 's/^/  /'; exit 2 ;;
esac

cd "$ROOT" || exit 2
shopt -s globstar nullglob
out="$("$NEST" check std/**/*.blsp tests/**/*.blsp examples/**/*.blsp 2>&1)"
n="$(printf '%s\n' "$out" | grep -c 'warning:' || true)"
if [ "${n:-0}" -ne 0 ]; then
  echo "check-imaged: $n warning(s) from nest check WITH the prelude image on (KI-106's class):"
  printf '%s\n' "$out" | grep 'warning:' | head -10 | sed 's/^/  /'
  exit 1
fi
echo "check-imaged: nest check is clean with the prelude image on (imaged boot confirmed)"
