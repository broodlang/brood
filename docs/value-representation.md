# Value representation — shrinking the 24-byte `Value` (scope)

> **Status: SCOPE / design (2026-06-17).** Not implemented. The "decide before 1.0" call
> flagged in `docs/vm-perf-and-jit-runway.md` §4.E. Targets the **memory-traffic** tax that
> the alloc-bound benchmarks (bintree ~17×, wordcount ~20×) pay and that no dispatch/JIT
> lever this session could touch (they're bound by allocation + GC copy, not dispatch).
> Background: `docs/jit-stage1.md`/`jit-tier2.md` (the JIT hardcodes the 24-byte/3-word
> layout), `docs/types.md` (the `Tag` system), `docs/memory-model.md` (handle bit layout).

## 1. Current representation (grounded in `core/value.rs`)

`Value` is a **24-byte** `#[repr(C, u8)]` enum (`size_of::<Value>() == 24`, pinned by
`value_layout_is_stable_for_the_jit`): a `u8` discriminant + up to a **two-word** payload,
8-aligned (`jit_layout::PAYLOAD_OFFSET == 8`). The variants and why it's 24, not 16:

- **`Int(i64)`** — full 64-bit. **`Float(f64)`** — full 64-bit. `Ref(u64)`/`Socket(u64)`/
  `Subprocess(u64)` — full 64-bit.
- **`Pid { node: Symbol(u32), id: u64 }`** — the lone **two-word** payload (12 bytes → 16
  aligned). **This is what forces 24 bytes**; every other variant fits in one 8-byte word.
- Handles — `Pair(PairId)`, `Vector/Range/SeqView(VecId)`, `Map(MapId)`, `Str(StrId)`,
  `Rope(RopeId)`, `BigInt(BigIntId)`, `Fn/Macro(ClosureId)`, `Native(NativeId)` — are each a
  **`u64`** with **region in bits 62–63** (`REGION_SHIFT = 62`), age bit, generation, and slab
  index in the low bits. They use the *full* 64 bits.
- `Nil`, `Bool(bool)`, `Sym/Keyword(Symbol=u32)` — small.

Two consumers hardcode this layout: **the JIT** (`jit_layout`: `TAG_INT == 2`, `TAG_PAIR == 9`,
`PAYLOAD_OFFSET == 8`, `STRIDE == 24`; the operand model copies a Value as **three i64 words**,
`Op::Handle` is three registers — `jit-tier2.md §3`) and **the type checker** (`Tag as u8` is a
bit position in the set-theoretic lattice; `tag(v)` reads the discriminant — `types.md`).

## 2. Why shrink it (the hypothesis)

A 24-byte `Value` is a tax on *every* operation: a pair is 48 B (two Values), a vector/map node
is Values, a call frame + the operand stack are Values, and the **moving GC copies all of it**.
Halving (or thirding) the Value would cut memory traffic across the board — most relevantly the
**GC-copy volume** on the alloc-bound benchmarks (bintree builds ~1.6M `[l r]` nodes; wordcount
churns CHAMP nodes) that this session's dispatch/inlining levers couldn't move. It's also what
JIT'd register code wants (a Value in one register, not three). Pre-alpha is the cheapest this
will ever be to change.

**Caveat from this session:** the operand-stack-cursor experiment was *neutral* because dispatch,
not operand traffic, dominated *those* (recursion) benchmarks. Shrinking the Value is a broader
lever (it cuts GC-copy + slab + frame traffic too, not just the operand stack), so it plausibly
helps the *alloc-bound* benchmarks the cursor didn't — but the hypothesis must be **measured**,
not assumed (cf. inline-small-vectors, which was neutral because mimalloc makes the allocs cheap).

## 2a. The target is the BEAM: an 8-byte tagged word (fixnum scheme, NOT NaN-box)

An Erlang/BEAM term is **8 bytes — one machine word** (the runtime we're chasing). Low bits are
tag bits; the rest is an immediate or a tagged pointer: **small ints inline (~60-bit fixnums; 4 tag
bits), atoms/nil/pid/port immediate**; a fixnum overflowing ~60 bits promotes to a **bignum
(boxed)**; **floats are BOXED** (a term is a *pointer* to a heap `f64` — the BEAM does **not** unbox
floats), as are tuples/maps/binaries/bignums. A **cons cell is 16 bytes** (a 2-word heap block:
head, tail) reached through an 8-byte tagged pointer.

So Brood's 48-byte pair vs the BEAM's 16-byte cons is the **3× memory/GC-copy factor** that *is* the
alloc-bound tax. Two design lessons from this:

- **Use the fixnum scheme, not NaN-boxing.** The BEAM reaches 8 bytes by tagging small ints inline +
  **boxing floats** — not by NaN-boxing (which unboxes floats and boxes ints). So §3a's int crux is
  resolved by *following the BEAM*: small ints inline (~60-bit), overflow → bignum (Brood already
  promotes to `BigInt`), **floats boxed**.
- **The tradeoffs are proven livable.** The BEAM lives with ~60-bit smalls and boxed floats and is
  still fast — it even *beat Brood on `mandelbrot`* (273 ms) **with** boxed floats. So "boxed floats
  hurt float-heavy code" is a smaller worry than it appears; the broad GC-copy win dominates.

This makes the 8-byte target concrete and de-risks the scheme choice — but the cruxes below (handle
repack, Pid/Ref boxing, the JIT 1-word rewrite, the `Tag` change) remain, which is why §4's cheaper
16-byte test should still gate the decision.

## 3. The full 8-byte tagged word (fixnum scheme) — four cruxes, all real

A single `u64`: an `f64` is itself when it's not a NaN; every other variant lives in NaN-space
(~48–51 payload bits + a few tag bits). For Brood specifically:

- **(a) `i64` ints don't fit — use the BEAM's fixnum scheme (§2a).** Tag bits leave ~60 inline
  bits, not 64: small ints inline (~60-bit fixnums), and ints beyond promote to `BigInt` (Brood
  already does this on overflow — the threshold just moves from 2^63 to ~2^60). **Floats are boxed**
  (a `Value::Float` becomes a one-word handle into a float slab), exactly as on the BEAM — not the
  NaN-box "native floats + boxed ints," because the BEAM proves the fixnum-with-boxed-floats scheme
  works (§2a). Keep a distinct float/boxed-int tag so `int?`/`float?`/`type-of` stay correct.
- **(b) Handles are 62-bit, payload is ≤48-bit.** Every `Id` (region@62 + gen + index) must
  **repack into ~48 bits** → fewer generation bits (the use-after-GC epoch tripwire is 30-bit
  today) and fewer index bits (max objects/slab). Tightens the GC's safety margin.
- **(c) `Pid`/`Ref`/`Socket`/`Subprocess`** — the 2-word `Pid` must become a boxed handle; the
  `u64` payloads must fit 48 bits or box.
- **(d) The JIT 3-word model** (`Op`, `read_words`/`store_words`, `Op::Handle`, the 24-byte
  stride, the layout asserts — the engine we spent this session on) becomes **1-word**: a real
  JIT rewrite. The `Tag` extraction (type checker) changes too.

Payoff: 24→8 (≈3× less memory traffic). Cost: a multi-session rearchitecture touching every
`Value` construct/match in the codebase, with semantic constraints on int range + GC
generation/index bits. **High-risk, high-effort, partly-irreversible.**

## 4. The cheaper test: 24 → 16 bytes (box `Pid` only)

`Pid` is the *sole* reason `Value` is 24 not 16. Box it (a `Pid` becomes a one-word handle into a
small `Pid` slab/table, like every other heap object) and the largest payload is one 8-byte word →
**`Value` = 16 bytes**, a **33% memory-traffic cut** — with **none** of §3's cruxes:

- `Int(i64)`/`Float(f64)`/handles stay exactly as they are (they fit in the 8-byte word).
- No int-range change, no handle repack, no GC-epoch-bit change, no `Tag` change.
- The JIT goes from a 3-word to a **2-word** copy (`STRIDE 24→16`, `PAYLOAD` unchanged) — a small,
  contained change to the layout constants + the `Op`/word helpers, not a 1-word rewrite.
- `Pid`-boxing is localized: the `Value::Pid` variant, its construct/match sites, the process
  registry, and the message codec (which already deep-copies pids across heaps).

This **tests the same hypothesis** (does shrinking `Value` cut GC-copy enough to move the
alloc-bound benchmarks?) at a fraction of the risk. If 16 B measurably helps bintree/wordcount,
the full 8-B NaN-box's *additional* 16→8 is justified and we know the direction pays. **If 16 B is
neutral** (mimalloc + the GC already cheap, like inline-vectors was), then the 8-B NaN-box — same
memory-traffic class, far more risk — is very unlikely to be worth it, and we've saved the
multi-session rewrite. Measure-first, exactly as the 9 levers before it should have been.

## 5. Recommended plan

1. **Prototype 24→16 (box `Pid`).** Bounded, no semantic cruxes. Full gate (differential + GC
   STRESS+VERIFY + the 2161 suite + the JIT layout asserts updated to 16/2-word). A/B the
   **alloc-bound** benchmarks (bintree, wordcount, sort, nqueens) + a no-regression check on the
   rest. *This is the go/no-go measurement for the whole representation bet.*
2. **If 16 B pays:** scope the 8-B tagged word for real — the **BEAM fixnum scheme** (§2a/§3a:
   ~60-bit inline ints → bignum overflow, **boxed floats**, a distinct float tag), the handle
   repack (§3b), and the JIT 1-word rewrite (§3d). Stage behind the same measure-then-commit
   discipline; it's a multi-session effort and a `decide-before-1.0` checkpoint.
3. **If 16 B is neutral:** stop. The alloc-bound gaps need a *different* lever (FBIP in-place
   reuse, `allocation-elimination.md §3.C` — the other high-risk bet), not a smaller Value.

## 6. Validation (every step)

Moving-GC + JIT-layout change: the JIT≡VM≡tree-walker differential under `BROOD_GC_STRESS=1
BROOD_GC_VERIFY=1`, the `gc` suite, the full `nest test` (2161), and the JIT layout asserts
(`value_layout_is_stable_for_the_jit`) are the net. A `Value`-size change is caught immediately by
those asserts; a wrong `Pid` boxing surfaces in the message/dist round-trip tests and the
cross-process suite blocks. Bench A/B on the alloc-bound set is the go/no-go.

## 7. Key files & symbols

- `crates/lisp/src/core/value.rs` — `enum Value` (`:376`, `#[repr(C, u8)]`), `enum Tag` (`:501`),
  `jit_layout` (`:820`: `PAYLOAD_OFFSET`/`TAG_INT`/`TAG_PAIR`), `value_layout_is_stable_for_the_jit`
  (`:855`), the handle macro (`:231` `local_gen`/`REGION_SHIFT`/`GEN_MASK`), `Value::Pid` (the box
  target).
- `crates/lisp/src/eval/compile.rs` — the JIT `Op` model / `read_words`/`store_words` / `STRIDE`
  (3-word → 2-word for the 16 B step).
- `crates/lisp/src/process/` — the `Pid` registry + message codec (the `Pid`-boxing surface).
- Background: `docs/vm-perf-and-jit-runway.md` §4.E (the original flag), `docs/jit-tier2.md` §3
  (the 24-byte/3-word ABI), `docs/types.md` (the `Tag` lattice).
