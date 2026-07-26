# NIST CAVP test vectors

The vectors an implementation has to pass to be FIPS-validated. Machine-readable `.rsp`
records; CRLF is stripped at fetch time so the runner's line splitting never carries a
`\r` into a hex field.

| file | records | format |
|---|---|---|
| `SHA1ShortMsg.rsp` | 65 | `Len` / `Msg` / `MD` |
| `SHA256ShortMsg.rsp` | 65 | ″ |
| `SHA384ShortMsg.rsp` | 129 | ″ |
| `SHA512ShortMsg.rsp` | 129 | ″ |
| `SHA256LongMsg-sampled.rsp` | 8 | ″ (every 8th record of 64) |
| `HMAC.rsp` | 1,575 | `Klen` / `Tlen` / `Key` / `Msg` / `Mac`, in `[L=n]` sections |

- **Upstream**: <https://csrc.nist.gov/projects/cryptographic-algorithm-validation-program>
  (`shabytetestvectors.zip`, `hmactestvectors.zip`)
- **Licence**: US Government work — public domain
- **Runner**: `tests/conformance_crypto_test.blsp`
- **Refresh**: `scripts/fetch-corpus.sh crypto`

## What is vendored, and what applies

Only the algorithms Brood exposes: **SHA-1, SHA-256, SHA-384, SHA-512** (plus MD5, which
CAVP no longer publishes vectors for — the canonical RFC 1321 digests are asserted inline
in the runner instead). SHA-224 and the SHA-512/t truncations have no target — `%hmac` and
`std/hash` don't expose them — so the `[L=28]` HMAC section is counted as skipped rather
than silently dropped, and the runner asserts the skip count is non-zero so a parser
change that stops running cases is caught instead of reading as a pass.

**ShortMsg is the valuable half.** It walks message bit lengths 0..255 (0..1023 for the
512-bit digests) — every block and padding boundary, which is where hash implementations
actually break. LongMsg is multi-block and gets record-sampled; `Monte` (iterated hashing)
is skipped as too expensive to drive from Brood for what it adds.

Two format traps the runner handles explicitly, both of which silently corrupt results if
missed:

- **`Len = 0` is spelled with a placeholder `Msg = 00`.** The message is *empty*, not the
  byte `0x00`. Getting this wrong fails every empty-message vector.
- **`Tlen` truncates the MAC**, in bytes — so the comparison is against the first
  `2 × Tlen` hex characters, not the whole digest. This is the part a thin wrapper gets
  wrong, and it is why the HMAC file is worth running rather than trusting the crate.

**Project Wycheproof is deliberately not vendored.** Its value is in ECDSA, AES-GCM, RSA
and key-agreement edge cases, none of which Brood implements; for the primitives Brood
does have, CAVP is the authoritative set. Revisit if `std/crypto` grows public-key or AEAD
surface beyond its current secretbox-style API.

## Findings

None. The digests come from Rust crates that are themselves CAVP-validated, so the real
exposure here was never the compression function — it was the wiring: the algorithm
keyword reaching `%hmac`, the hex output casing, the UTF-8/bytes boundary, and MAC
truncation. All correct. The value is that this is the one corpus in the set whose
failures would be *security* bugs, so it is worth having the gate.
