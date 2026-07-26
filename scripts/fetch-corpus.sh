#!/usr/bin/env bash
# scripts/fetch-corpus.sh — (re)fetch the external conformance corpora.
#
# Brood's own tests are hand-written; these are the corpora other implementers
# already paid for in production bugs (see ROADMAP "External conformance
# corpora"). Each suite lands in tests/corpus/<suite>/ with a README.md pinning
# the upstream URL, commit and licence.
#
#   scripts/fetch-corpus.sh                 # refresh every vendored suite
#   scripts/fetch-corpus.sh parse-number    # refresh one
#   scripts/fetch-corpus.sh --full parse-number
#
# The committed subset stays small enough to read and to keep in git. `--full`
# additionally pulls the multi-hundred-megabyte upstream files into
# tests/corpus/<suite>/full/ (gitignored) for an exhaustive local pass; the
# runners pick those up automatically when present.
#
# Subsampling is "every Nth line", never random — re-running must reproduce the
# committed bytes exactly.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CORPUS="$ROOT/tests/corpus"
FULL=0

fetch() { # fetch <url> <dest>
  printf '  %s\n' "${1##*/}"
  curl -fsSL --retry 3 "$1" -o "$2"
}

# every Nth line, offset 1 — deterministic, so the committed file is reproducible
sample() { # sample <src> <n> <dest>
  awk -v n="$2" 'NR % n == 1' "$1" >"$3"
}

# ---------------------------------------------------------------- parse-number
# nigeltao/parse-number-fxx-test-data — decimal->f{16,32,64} parsing.
# Apache-2.0. ~270 MB upstream; we vendor the curated small files whole and
# subsample the two mid-size ones.
PARSE_NUMBER_COMMIT=55d79b184b7d8fac2e143e89dc19b766ec4e54b8
PARSE_NUMBER_BASE="https://raw.githubusercontent.com/nigeltao/parse-number-fxx-test-data/$PARSE_NUMBER_COMMIT"

suite_parse_number() {
  local d="$CORPUS/parse-number" tmp
  mkdir -p "$d/data"
  tmp="$(mktemp -d)"

  # Vendored whole — small, and each is a distinct real-world extraction.
  local whole=(
    more-test-cases.txt       # the hand-picked pathological strings
    lemire-fast-float.txt
    freetype-2-7.txt
    tencent-rapidjson.txt
    google-wuffs.txt
  )
  for f in "${whole[@]}"; do
    fetch "$PARSE_NUMBER_BASE/data/$f" "$d/data/$f"
  done

  # Subsampled — every 16th line keeps the shape and the exponent spread.
  local sampled=(
    ibm-fpgen.txt
    lemire-fast-double-parser.txt
  )
  for f in "${sampled[@]}"; do
    fetch "$PARSE_NUMBER_BASE/data/$f" "$tmp/$f"
    sample "$tmp/$f" 16 "$d/data/${f%.txt}-sampled.txt"
  done

  rm -rf "$tmp"
  fetch "$PARSE_NUMBER_BASE/LICENSE" "$d/LICENSE"

  if [ "$FULL" = 1 ]; then
    mkdir -p "$d/full"
    for f in exhaustive-float16.txt google-double-conversion.txt ulfjack-ryu.txt \
             remyoudompheng-fptest-0.txt remyoudompheng-fptest-1.txt \
             remyoudompheng-fptest-2.txt remyoudompheng-fptest-3.txt \
             ibm-fpgen.txt lemire-fast-double-parser.txt; do
      fetch "$PARSE_NUMBER_BASE/data/$f" "$d/full/$f"
    done
  fi
}

# --------------------------------------------------------------------- dectest
# Mike Cowlishaw / IBM's "General Decimal Arithmetic Testcases" — the definitive
# decimal arithmetic suite, distributed with decNumber under the ICU licence.
# Fetched from CPython, which vendors it verbatim as Lib/test/decimaltestdata.
#
# We take the base (precision-9, "extended") files for the operations Brood's
# arbitrary-precision Decimal actually implements. The runner filters further —
# see tests/conformance_dectest_test.blsp for exactly which cases apply and why.
DECTEST_COMMIT=8a5465339e639e1527f874e377b3aa9c4eeea860
DECTEST_BASE="https://raw.githubusercontent.com/python/cpython/$DECTEST_COMMIT/Lib/test/decimaltestdata"

suite_dectest() {
  local d="$CORPUS/dectest"
  mkdir -p "$d/data"
  for f in add subtract multiply minus plus abs compare; do
    fetch "$DECTEST_BASE/$f.decTest" "$d/data/$f.decTest"
  done
}

# ------------------------------------------------------------------------- json
# nst/JSONTestSuite — the corpus behind "Parsing JSON is a Minefield". MIT.
# `y_` must parse, `n_` must be rejected, `i_` is implementation-defined. The
# upstream repo is ~60 MB of parser binaries; we take only test_parsing/ (1.6 MB).
JSON_COMMIT=1ef36fa01286573e846ac449e8683f8833c5b26a
JSON_TARBALL="https://github.com/nst/JSONTestSuite/archive/$JSON_COMMIT.tar.gz"

suite_json() {
  local d="$CORPUS/json" tmp
  mkdir -p "$d"
  tmp="$(mktemp -d)"
  printf '  %s\n' "test_parsing/ (from the repo tarball)"
  curl -fsSL --retry 3 "$JSON_TARBALL" -o "$tmp/jts.tgz"
  rm -rf "$d/data"
  mkdir -p "$d/data"
  tar xzf "$tmp/jts.tgz" -C "$d/data" --strip-components=2 --wildcards "*/test_parsing/*"
  tar xzf "$tmp/jts.tgz" -C "$tmp" --strip-components=1 --wildcards "*/LICENSE"
  cp "$tmp/LICENSE" "$d/LICENSE"
  rm -rf "$tmp"
}

# -------------------------------------------------------------------------- ucd
# The Unicode Character Database conformance files. Unicode licence.
#
# UCD_VERSION must track the Unicode version the `unicode-segmentation` and
# `unicode-normalization` crates were generated from (both are on 16.0.0 as of
# 2026-07-25) — testing against a NEWER UCD than the crates implement produces
# failures that are version skew, not bugs. Check `Cargo.lock` before bumping.
UCD_VERSION=16.0.0
UCD_BASE="https://www.unicode.org/Public/$UCD_VERSION/ucd"

suite_ucd() {
  local d="$CORPUS/ucd"
  mkdir -p "$d/data"
  fetch "$UCD_BASE/auxiliary/GraphemeBreakTest.txt" "$d/data/GraphemeBreakTest.txt"
  fetch "$UCD_BASE/NormalizationTest.txt" "$d/data/NormalizationTest.txt"
  echo "$UCD_VERSION" >"$d/data/VERSION"
}

# -------------------------------------------------------------------------- csv
# maxogden/csv-spectrum — the tricky-CSV corpus. BSD-2-Clause. Each csvs/X.csv
# pairs with json/X.json holding the rows it must parse to.
CSV_COMMIT=d30e80f8b99d2eecb3778f1d7b9ed1cb425502ec
CSV_TARBALL="https://github.com/maxogden/csv-spectrum/archive/$CSV_COMMIT.tar.gz"

suite_csv() {
  local d="$CORPUS/csv" tmp
  mkdir -p "$d"
  tmp="$(mktemp -d)"
  printf '  %s\n' "csvs/ + json/ (from the repo tarball)"
  curl -fsSL --retry 3 "$CSV_TARBALL" -o "$tmp/cs.tgz"
  rm -rf "$d/data"
  mkdir -p "$d/data"
  tar xzf "$tmp/cs.tgz" -C "$d/data" --strip-components=1 --wildcards "*/csvs/*" "*/json/*"
  rm -rf "$tmp"
}

# ------------------------------------------------------------------------- utf8
# Markus Kuhn's "UTF-8 decoder capability and stress test" — CC BY 4.0. A single
# document that is deliberately NOT valid UTF-8: it embeds overlongs, lone
# surrogates, truncated sequences and impossible bytes in its own prose.
UTF8_URL="https://www.cl.cam.ac.uk/~mgk25/ucs/examples/UTF-8-test.txt"

suite_utf8() {
  local d="$CORPUS/utf8"
  mkdir -p "$d/data"
  fetch "$UTF8_URL" "$d/data/UTF-8-test.txt"
}

# ------------------------------------------------------------------------ crypto
# NIST CAVP test vectors — the byte-oriented SHA vectors and the HMAC vectors. US
# government work, public domain. Machine-readable `.rsp` files: `Len`/`Msg`/`MD`
# records for the hashes, `Klen`/`Tlen`/`Key`/`Msg`/`Mac` for HMAC.
#
# Only the algorithms Brood's `%hmac` / std/hash implement (SHA-1/256/384/512 —
# SHA-224 and the SHA-512/t truncations are not exposed). ShortMsg covers bit
# lengths 0..255, i.e. every block/padding boundary, which is where hash bugs live;
# LongMsg is multi-block and gets record-sampled to keep the tree small.
CRYPTO_SHA_ZIP="https://csrc.nist.gov/CSRC/media/Projects/Cryptographic-Algorithm-Validation-Program/documents/shs/shabytetestvectors.zip"
CRYPTO_HMAC_ZIP="https://csrc.nist.gov/CSRC/media/Projects/Cryptographic-Algorithm-Validation-Program/documents/mac/hmactestvectors.zip"

# Every Nth blank-line-separated RECORD (awk paragraph mode), not every Nth line —
# a .rsp case is a 3-line record, so line sampling would shred it.
sample_records() { # sample_records <src> <n> <dest>
  awk -v n="$2" 'BEGIN{RS="";ORS="\n\n"} NR % n == 1' "$1" >"$3"
}

suite_crypto() {
  local d="$CORPUS/crypto" tmp
  mkdir -p "$d/data"
  tmp="$(mktemp -d)"

  printf '  %s\n' "shabytetestvectors.zip"
  curl -fsSL --retry 3 "$CRYPTO_SHA_ZIP" -o "$tmp/sha.zip"
  unzip -oq "$tmp/sha.zip" -d "$tmp/sha"
  for a in SHA1 SHA256 SHA384 SHA512; do
    # CRLF -> LF so the runner's line splitting doesn't carry \r into a hex field.
    tr -d '\r' <"$tmp/sha/shabytetestvectors/${a}ShortMsg.rsp" >"$d/data/${a}ShortMsg.rsp"
  done
  tr -d '\r' <"$tmp/sha/shabytetestvectors/SHA256LongMsg.rsp" >"$tmp/long.rsp"
  sample_records "$tmp/long.rsp" 8 "$d/data/SHA256LongMsg-sampled.rsp"

  printf '  %s\n' "hmactestvectors.zip"
  curl -fsSL --retry 3 "$CRYPTO_HMAC_ZIP" -o "$tmp/hmac.zip"
  unzip -oq "$tmp/hmac.zip" -d "$tmp/hm"
  tr -d '\r' <"$tmp/hm/HMAC.rsp" >"$d/data/HMAC.rsp"

  rm -rf "$tmp"
}

# ----------------------------------------------------------------------- driver
SUITES=(parse-number dectest json ucd csv utf8 crypto)

main() {
  local want=()
  for a in "$@"; do
    case "$a" in
      --full) FULL=1 ;;
      -h|--help) sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \?//'; exit 0 ;;
      *) want+=("$a") ;;
    esac
  done
  [ ${#want[@]} -eq 0 ] && want=("${SUITES[@]}")

  for s in "${want[@]}"; do
    local fn="suite_${s//-/_}"
    if ! declare -F "$fn" >/dev/null; then
      echo "fetch-corpus: unknown suite '$s' (have: ${SUITES[*]})" >&2
      exit 1
    fi
    echo "$s:"
    "$fn"
  done
  echo "done — $(du -sh "$CORPUS" | cut -f1) in tests/corpus/"
}

main "$@"
