# csv-spectrum

Max Ogden's tricky-CSV corpus — small on purpose, covering the places RFC 4180
parsers actually break. Each `data/csvs/X.csv` pairs with `data/json/X.json` holding
the rows it must parse to.

| document | what it pins |
|---|---|
| `comma_in_quotes` | the delimiter inside a quoted field |
| `escaped_quotes` | `""` as one literal quote |
| `quotes_and_newlines` | both at once, multi-line |
| `newlines` / `newlines_crlf` | a raw newline inside quotes, LF and CRLF |
| `simple` / `simple_crlf` | the baseline, both line endings |
| `empty` / `empty_crlf` | empty and trailing-empty fields |
| `utf8` | non-ASCII payloads |
| `json` | JSON embedded in a cell (quotes, braces, colons) |
| `location_coordinates` | **excluded — broken upstream**, see below |

- **Upstream**: <https://github.com/maxogden/csv-spectrum>
- **Pinned commit**: `d30e80f8b99d2eecb3778f1d7b9ed1cb425502ec`
- **Licence**: BSD-2-Clause
- **Runner**: `tests/conformance_csv_test.blsp`
- **Refresh**: `scripts/fetch-corpus.sh csv`

`location_coordinates` is excluded: its expectation file is a bare JSON *object*
where every other one is an array of row objects, and the phone number in it
(`1234567890`) is not the one in its own CSV (`2095257564`). There is nothing
coherent to test against.

## Findings

This is the first corpus whose subject is **pure Brood** — `std/csv` is a
hand-written state-machine parser, not a wrapper over a Rust crate — and it found a
bug immediately (2026-07-25):

**A CRLF inside a quoted field was being rewritten to LF.** RFC 4180 §2.6 says a
field enclosed in quotes may contain CRLF and that it is *content*; the parser
swallowed the `\r` in its `:quoted` state along with the ones that really are line
endings. So any CSV with a multi-line quoted cell — the `newlines_crlf` case, and
anything exported from Excel on Windows — silently lost its carriage returns and did
not round-trip through `csv-parse` → `csv-emit`. Line-ending normalisation belongs in
the `:unquoted` and `:quote-seen` states, which is where it now happens exclusively.
