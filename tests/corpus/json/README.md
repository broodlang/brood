# JSONTestSuite

Nicolas Seriot's corpus from *"Parsing JSON is a Minefield"* — the survey that found
essentially every shipped JSON parser disagreeing with every other one. Each file in
`data/` is one document, and its name is the verdict RFC 8259 requires:

| prefix | meaning | count |
|--------|---------|-------|
| `y_` | MUST parse | 95 |
| `n_` | MUST be rejected | 188 |
| `i_` | implementation-defined — either verdict conforms | 35 |

- **Upstream**: <https://github.com/nst/JSONTestSuite>
- **Pinned commit**: `1ef36fa01286573e846ac449e8683f8833c5b26a`
- **Licence**: MIT (see `LICENSE`)
- **Runner**: `tests/conformance_json_test.blsp`
- **Refresh**: `scripts/fetch-corpus.sh json`

Upstream is ~60 MB, almost all of it parser binaries for the survey. Only
`test_parsing/` is vendored (1.6 MB).

Several `n_` documents are deliberately not valid UTF-8. `slurp` raises on those
before `json-parse` is reached, which is the right verdict — a parser must not accept
mis-encoded bytes — so the runner counts an unreadable file as a rejection.

## Findings

Wiring this up (2026-07-25) turned up two bugs:

- **`std/json` accepted unescaped control characters inside strings**, which RFC 8259
  §7 forbids (U+0000–U+001F must be escaped). A raw tab or newline in a string body
  parsed as content. Fixed in `json--string--acc`. Caught by
  `n_string_unescaped_{ctrl_char,newline,tab}`.
- **KI-11**: `n_structure_100000_opening_arrays.json` and
  `n_structure_open_array_object.json` **abort the OS process** — deep non-tail
  recursion on the JIT path overflows the native stack, where the bytecode VM and the
  tree-walker both handle the same input correctly. Not a JSON bug; a JIT call-path
  bug this corpus happened to find. Both files are excluded by name in the runner
  until it is fixed. See [`docs/known-issues.md`](../../../docs/known-issues.md).
