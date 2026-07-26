# General Decimal Arithmetic Testcases (dectest)

Mike Cowlishaw / IBM's reference suite for decimal arithmetic — the corpus Python's
`decimal` module is validated against, and the one shipped with `decNumber`. Each
line is one case:

```
addx001 add 1       1       ->  2
addx003 add '5.75'  '3.3'   ->  9.05
addx011 add '0.4444444444'  '0.5555555555' -> '1.00000000' Inexact Rounded
```

`id operation operands… -> result [conditions…]`. Operands may be quoted; `--`
starts a comment; `precision:` / `rounding:` / `maxExponent:` lines set the context.

- **Upstream**: <https://speleotrove.com/decimal/dectest.html>
- **Fetched from**: <https://github.com/python/cpython> `Lib/test/decimaltestdata`,
  which vendors it verbatim
- **Pinned commit**: `8a5465339e639e1527f874e377b3aa9c4eeea860`
- **Licence**: ICU licence (as distributed with decNumber and ICU); © IBM 1981–2008
- **Runner**: `tests/conformance_dectest_test.blsp`
- **Refresh**: `scripts/fetch-corpus.sh dectest`

## What is vendored, and what applies

Only the operations Brood's `Decimal` has: `add`, `subtract`, `multiply`, `minus`,
`plus`, `abs`, `compare` — 5,616 lines, of which ~1,900 apply.

dectest specifies **IEEE 754 decimal**: a context with a precision, a rounding mode
and an exponent range. Brood's `Decimal` is arbitrary-precision and **exact**, with
no context at all. So the runner keeps only the cases where the two models must
agree — finite operands, and a reference result carrying no condition flags, meaning
the context did not round, clamp, overflow or underflow and dectest's answer *is* the
exact answer. Everything else (`Inexact`, `Rounded`, `Subnormal`, NaN, sNaN,
Infinity, and `divide` throughout) is skipped explicitly. The runner's header
documents each exclusion and why.

What survives is the part worth having: the **ideal-exponent** rules. dectest says
`1.50 × 2.25` is `3.3750`, not `3.375`, and `1 + 0.0` is `1.0`, not `1` — decades of
exponent-alignment edge cases nobody would write by hand.

## Findings

Wiring this up (2026-07-25) caught two scale bugs in Brood's decimal arithmetic,
both inherited from the `bigdecimal` crate's identity short-circuits:

- `Sub` returns the other operand untouched when one side is zero, so `1 - 0.0` gave
  `1` instead of `1.0`. `Add` does not short-circuit, so `+` and `-` disagreed.
- `Mul` returns the other operand untouched when one side is one-valued, so
  `1.00 × -1` gave `-1` instead of `-1.00`.

`num_bin` now pins every exact decimal result to the standard's ideal exponent.
