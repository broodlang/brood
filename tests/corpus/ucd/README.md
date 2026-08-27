# Unicode Character Database conformance files

Two machine-generated corpora from the standard's own tables.

**`GraphemeBreakTest.txt`** — UAX #29 extended grapheme cluster boundaries. Each line
spells a string with break markers between code points: `÷` is a cluster boundary,
`×` is not.

```
÷ 0020 × 0308 ÷ 0020 ÷	#  ÷ [0.2] SPACE (Other) × [9.0] COMBINING DIAERESIS ÷ [999.0] SPACE ÷ [0.3]
```

**`NormalizationTest.txt`** — UAX #15. Five `;`-separated columns of the same text —
source, NFC, NFD, NFKC, NFKD — as space-separated hex code points, with `@PartN`
section headers.

```
1E0A 0323;1E0C 0307;0044 0323 0307;1E0C 0307;0044 0323 0307; # …
```

The conformance requirement is a *closure*, not a single check: normalising **any** of
the five columns into a given form must yield that form's column. Checking only
`NFC(source)` would miss idempotence, which is where a normaliser usually breaks. The
runner checks the full closure — 6 assertions × ~19,000 lines.

- **Upstream**: <https://www.unicode.org/Public/16.0.0/ucd/>
- **Version**: 16.0.0 (recorded in `data/VERSION`)
- **Licence**: [Unicode licence](https://www.unicode.org/license.txt) (permissive)
- **Runner**: `tests/conformance_ucd_test.blsp`
- **Refresh**: `scripts/fetch-corpus.sh ucd`

## Version skew is the failure mode to suspect first

The vendored files must match the Unicode version the `unicode-segmentation` and
`unicode-normalization` crates were generated from — both are on **16.0.0** as of
2026-07-25. Testing against a newer UCD than the crates implement produces failures
that are skew, not bugs. Check `Cargo.lock` before bumping `UCD_VERSION` in
`scripts/fetch-corpus.sh`, and update the pinned assertion in the runner's first test.

## Findings

Wiring this up (2026-07-25) required two new primitives — `string->graphemes` and
`string-normalize` — since Brood previously exposed neither (only `string/display-width`,
which segments internally). Results:

- **NormalizationTest: ~19,000 cases, all pass**, full conformance closure.
- **GraphemeBreakTest: 602 cases, one failure**, and it is upstream, not ours.
  `÷ 2701 × 200D × 2701 ÷` — UAX #29 rule GB11 joins a ZWJ sequence when both sides
  are Extended_Pictographic, and Unicode 16 gives U+2701 UPPER BLADE SCISSORS that
  property, so it is one cluster. `unicode-segmentation` 1.13.3 (the current release)
  omits U+2701 from its Extended_Pictographic table and returns two. The rule is
  correct everywhere else — U+270A, U+2764, U+1F468 and U+1F3F3 ZWJ sequences all
  join — so it is a table gap around the U+2700 dingbats. Excluded from the sweep and
  pinned by a test asserting the *current* behaviour, so the exclusion fails loudly
  when the crate is fixed.
