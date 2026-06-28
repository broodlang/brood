# `store` (Postgres driver) findings — 2026-06-28

*Field notes from building **`store`** (a sibling repo, `../store`) — a native PostgreSQL
driver and data-mapping layer written in pure Brood: the wire protocol over `net/tcp`'s
binary mode (no libpq), SCRAM-SHA-256 auth, the simple + extended query protocols, type
codecs, and a supervised connection pool. It connects to and queries a real Postgres 18.4
end to end.*

> **Path convention.** Bare references like `wire/bytes.blsp` are files in the **`store`**
> repo (`../store/src/`). `crates/…`, `std/…`, and bare `docname.md` are in **this** (`brood`)
> repo.

The headline is positive: **the foundation was sufficient to build the whole driver in pure
Brood, no kernel patch.** `tcp-set-binary` (the Latin-1 byte carrier, as used by
`http/websocket`), `crypto`/`hash`, `uuid`, and `gen`/`supervisor` covered everything from
the wire framing to the pool. The 2026-06-03 audit's note that the missing bytevector type
*blocks* binary protocols (`net.rs` `from_utf8_lossy`) is now **stale** — binary mode is
byte-faithful and sufficient.

The gaps that remain are a tight, coherent cluster: **the crypto/encoding stdlib is
string/hex/UTF-8-oriented, not raw-byte-oriented.** SCRAM is all raw bytes (HMAC over a
binary key, a base64-decoded binary salt, XOR of digests), so every primitive it needs had
to be reimplemented in pure Brood — ~150 lines in `wire/bytes.blsp` — and the pure-Brood
PBKDF2 costs ~2s per connection. One spawn footgun was also hit and **already fixed** in
this repo.

---

## 0. Triage summary

| # | Severity | Area | One-liner | Status |
|---|----------|------|-----------|--------|
| 1 | HIGH | prelude | `(spawn (fn () body))` silently no-ops | **Fixed** (`1a63eb7`) |
| 2 | MED | std/hash | no raw-byte HMAC (string-only) | open |
| 3 | MED | std/hash | `sha256-bytes` returns hex, not bytes | open |
| 4 | MED | std/crypto | `pbkdf2` can't take a binary salt | open |
| 5 | MED | perf | pure-Brood PBKDF2 ≈ 2s/connection | open (subsumed by #4) |
| 6 | MED | std/encoding | base64/hex are UTF-8-bound, not byte-vector | open |
| 7 | LOW | ergonomics | binary I/O rides a Latin-1 string carrier | open |
| 8 | LOW | types | no decimal/bignum → `numeric` decodes to string | open |

**Highest-leverage item:** findings 2, 3, 4, and 6 are one change — *a raw-byte crypto/encoding
layer*. It removes the ~150 lines of `wire/bytes.blsp` reimplementation, fixes correctness for
any binary-protocol auth (Postgres SCRAM, Redis, AMQP…), and erases the ~2s/connection cost.

---

## 1. [HIGH · prelude footgun] `(spawn (fn () body))` is a silent no-op — *fixed*

**Symptom.** A spawned process never runs its body; it exits `:normal` immediately, so
monitors fire `[:down … :normal]` and any reply it was meant to send never arrives. Cost us
a long debugging session — the process *looks* like it started.

**Minimal repro** (deterministic, release and debug):
```clojure
(def parent (self))
(spawn (fn () (send parent [:hi 99])))
(receive ([:hi v] v) (after 2000 :timeout))   ;=> :timeout (body never ran)
```

**Root cause.** `spawn` is a body-taking macro: `(spawn expr)` → `(%spawn (fn () expr))`. So
`(spawn (fn () body))` expands to `(%spawn (fn () (fn () body)))` — the process evaluates the
inner `(fn () body)` to a closure, discards it, and exits. The Erlang
`spawn(fun() -> … end)` habit produces exactly this, and it fails silently.

**Fix (already landed, `1a63eb7`).** `spawn`/`spawn-link` now detect a literal `(fn () …)`
body and pass it through unwrapped, so `(spawn body)` and `(spawn (fn () body))` mean the
same. Non-lambda bodies and `(spawn name expr)` are unchanged; `gen`/`agent` (which spawn
`(f state)`, not a lambda) are unaffected. Verified against `gen_test`, `agent_test`.

**Follow-up worth considering.** The macro can only see a *literal* lambda; `(spawn my-thunk)`
where `my-thunk` is a 0-arg fn value is the same no-op and the macro can't catch it. The
advisory type checker could warn when a `spawn` body statically has function type (i.e. the
process would evaluate to an uncalled function and do nothing).

---

## 2. [MED · std/hash] No raw-byte HMAC

`hmac-sha256` (`std/hash.blsp:85`) takes a **string** key and message and returns a **hex
string**. SCRAM's `ClientKey = HMAC(SaltedPassword, "Client Key")` needs HMAC over a
**raw-byte key** (the PBKDF2 output) with **raw-byte output** (it gets XORed and re-hashed).
A Brood string can't faithfully carry an arbitrary-byte key (its UTF-8 encoding ≠ the bytes).

**Worked around** by implementing HMAC-SHA-256 over byte vectors in pure Brood
(`wire/bytes.blsp` `hmac256`), built on `sha256-bytes`; validated against `hmac-sha256` and
the RFC 4231 vector.

**Suggested fix.** `%hmac-sha256-bytes (key-bv msg-bv) -> bv` (and `…-sha1/512`). The `hmac`
crate is already a dependency (`std/hash.blsp` header).

---

## 3. [MED · std/hash] `sha256-bytes` returns hex, not bytes

`sha256-bytes` (`std/hash.blsp:55`) accepts a byte vector but returns a **hex string**.
Chaining digests over raw bytes (SCRAM `StoredKey = SHA256(ClientKey)`, then HMAC over
`StoredKey`) forces a hex→bytes decode on every step.

**Worked around** with `sha256v` = `hex->bytes ∘ sha256-bytes` (`wire/bytes.blsp`).

**Suggested fix.** A raw-bytes-returning digest — either make `sha256-bytes` return a byte
vector, or add `sha256-raw`/`%sha256-bytes->bytes`.

---

## 4. [MED · std/crypto] `pbkdf2` cannot take a binary salt

`crypto/pbkdf2` (`std/crypto.blsp:91`) does `(if (string? salt) salt (bytes->str salt))` —
i.e. it UTF-8-decodes a byte-vector salt — and the underlying `%pbkdf2-sha256` takes a
**String** salt. SCRAM's salt is **base64-decoded binary**, so `(bytes->str salt)` throws
`invalid UTF-8`, and even a string salt would be re-encoded as UTF-8 rather than used as raw
bytes.

**Worked around** by implementing PBKDF2-HMAC-SHA256 over byte vectors in pure Brood
(`wire/bytes.blsp` `pbkdf2-sha256`), on top of `hmac256`. Correctness validated end to end:
the SCRAM client proof matches the RFC 7677 §3 worked example.

**Suggested fix.** `%pbkdf2-sha256-bytes (pw-bv salt-bv iterations key-len) -> bv`.

---

## 5. [MED · perf] Pure-Brood PBKDF2 ≈ 2s per connection

A consequence of #4: at Postgres's default `iterations = 4096`, the Brood-level PBKDF2 runs
~4096 HMACs (~12k SHA-256 calls), each through a hex-string round-trip and byte-vector
rebuilds — ~2s wall per connection on this machine. It dominates connection establishment;
`store`'s pool only hides it by opening its connections **in parallel** (startup ≈ one
handshake, not N). A native byte-PBKDF2 (#4) removes both the correctness gap and the latency
(the `pbkdf2`/`hmac`/`sha2` crates do this in microseconds).

---

## 6. [MED · std/encoding] base64 and hex are UTF-8-bound

`base64-encode` (`std/encoding.blsp:93`) encodes a string's **UTF-8 bytes**
(`string->utf8-bytes`), so it can't base64 an arbitrary byte vector — a codepoint ≥ 0x80
becomes two bytes. `hex-decode` builds a string and throws `invalid UTF-8` on any non-UTF-8
byte. SCRAM needs base64 of the client proof (raw HMAC bytes), base64-decode of the salt, and
hex over digests.

**Worked around** with byte-vector base64/hex in `wire/bytes.blsp`
(`b64-encode`/`b64-decode`/`hex->bytes`/`bytes->hex`).

**Suggested fix.** Byte-vector variants: `base64-encode-bytes (bv) -> str`,
`base64-decode->bytes (str) -> bv`, `hex-decode->bytes (str) -> bv`, `bytes->hex (bv) -> str`.

---

## 7. [LOW · ergonomics] Binary I/O rides a Latin-1 string carrier

With no bytevector value kind, all wire data is a *byte string* — codepoints 0–255, one per
wire byte — via `tcp-set-binary` (the same convention as `http/websocket`). It **works** and
is byte-faithful, but every framed message needs a 256-entry `*byte-table*` and explicit
byte↔codepoint conversions (`byte->char`/`byte-at`), and a second representation (byte
vectors) for the crypto/math, with conversions at the seams (`wire/bytes.blsp`).

The roadmap's per-socket bytes mode (`[:tcp sock bytevec]`) and/or a real bytes value type
would remove the carrier dance. Not blocking — just a persistent tax on any binary protocol.

---

## 8. [LOW · types] No decimal/bignum → `numeric` decodes to a string

`store` decodes Postgres `numeric`/`decimal` columns as **strings** to stay lossless
(`wire/types.blsp`): `string->number` would coerce to f64 and lose precision, which is wrong
for money. A `bignum`/`decimal` value type (related to the audit's `string->number` >i64
precision note) would let `numeric` decode to a real number and round-trip exactly.

---

## What the foundation got right

Worth recording, since the gaps above are narrow: `tcp-set-binary` made the wire protocol
faithful with no kernel change; `gen`/`supervisor` made the pooled, self-healing connection
layer straightforward (each connection is a green process owning its socket; the pool
monitors and respawns); selective `receive` made request/response framing over the mailbox
clean; and `crypto`/`hash` had every *algorithm* SCRAM needed — only the *byte-oriented
shapes* were missing. A from-scratch Postgres driver in a young Lisp, talking to a real
server, is a good showing for the runtime.
