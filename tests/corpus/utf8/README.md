# UTF-8 decoder capability and stress test

Markus Kuhn's single-document torture test for UTF-8 decoders. The file is deliberately
**not** valid UTF-8: it embeds overlong forms, lone surrogates, truncated sequences and
impossible bytes inside its own prose, so a decoder that silently substitutes U+FFFD
produces a readable-looking document while a correct one refuses the file.

- **Upstream**: <https://www.cl.cam.ac.uk/~mgk25/ucs/examples/UTF-8-test.txt>
- **Revision**: 2015-08-28 (22,781 bytes — the runner asserts the length, so a silent
  upstream revision shows up as a failure rather than as drift)
- **Licence**: CC BY 4.0
- **Runner**: `tests/conformance_utf8_test.blsp`
- **Refresh**: `scripts/fetch-corpus.sh utf8`

## How it is used

Two ways. The vendored file gives the assertion that matters most in practice — `slurp`
promises a Brood string, and Brood strings are UTF-8, so the only correct answer for this
file is to **raise**, while `slurp-bytes` must return all 22,781 bytes untouched.

Kuhn's catalogue then supplies the case table, transcribed into the runner as hex
sequences with his section numbers kept so any case can be looked up against the
document: §2 boundary conditions, §3.1 unexpected continuation bytes, §3.2 lonely start
characters, §3.3–3.4 truncation, §3.5 impossible bytes, §4.1–4.3 overlong forms, §5.1–5.2
surrogates, §5.3 noncharacters.

Two classes are worth knowing about because they are easy to get backwards:

- **Overlong forms must be rejected**, and this is security-critical rather than
  pedantic: an overlong `/` (`C0 AF`) slips a path separator past a filter that only
  looks for the one-byte `2F`.
- **Noncharacters must be accepted.** U+FFFE, U+FFFF and friends are legal code points
  that merely must not be interchanged as text; rejecting them is as wrong as accepting a
  surrogate. `EF BF BF` decodes.

## Findings

None — Brood's UTF-8 handling delegates to Rust's `String::from_utf8`, which gets all of
this right, and `slurp` correctly raises rather than substituting. The value here is the
regression gate plus the explicit record of which classes are accept-vs-reject, since
that is the part a future hand-rolled decoder or a `slurp` "convenience" change would get
wrong.
