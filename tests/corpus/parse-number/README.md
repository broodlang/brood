# parse-number-fxx test data

Decimal-string → IEEE 754 binary64 parsing. Every line is one case: the expected
`f16`, `f32` and `f64` bit patterns in hex, then the input string.

```
3C00 3F800000 3FF0000000000000 1
57B7 42F6E979 405EDD2F1A9FBE77 123.456
7C00 7F800000 7FF0000000000000 123.456e789
```

Columns are fixed: `[0..4]` f16, `[5..13]` f32, `[14..30]` f64, `[31..]` the input.
Brood has no f16/f32, so the runner reads the f64 column only.

- **Upstream**: <https://github.com/nigeltao/parse-number-fxx-test-data>
- **Pinned commit**: `55d79b184b7d8fac2e143e89dc19b766ec4e54b8`
- **Licence**: Apache-2.0 (see `LICENSE`)
- **Runner**: `tests/conformance_parse_number_test.blsp`
- **Refresh**: `scripts/fetch-corpus.sh parse-number`

## What is vendored

Upstream is ~270 MB. `data/` holds 33,552 cases (~1.5 MB): the curated small
files whole, and every 16th line of the two mid-size ones.

| File | Cases | Origin |
|------|-------|--------|
| `more-test-cases.txt` | 60 | hand-picked pathological strings — huge/tiny exponents, the `2.2250738585072011e-308` family |
| `lemire-fast-float.txt` | 3,299 | Lemire's fast_float |
| `freetype-2-7.txt` | 3,566 | FreeType 2.7 |
| `tencent-rapidjson.txt` | 3,563 | RapidJSON |
| `google-wuffs.txt` | 10,744 | Wuffs |
| `ibm-fpgen-sampled.txt` | 6,425 | IBM's fpgen, every 16th line |
| `lemire-fast-double-parser-sampled.txt` | 5,895 | Lemire's fast_double_parser, every 16th line |

`scripts/fetch-corpus.sh --full parse-number` additionally pulls the complete
upstream (including the exhaustive float16 list and the 4×50 MB
`remyoudompheng-fptest` files) into `full/`, which is gitignored. The runner
prefers `full/` when it is present, so an exhaustive local pass needs no code
change.
