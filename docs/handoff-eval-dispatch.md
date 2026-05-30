# Handoff — attacking evaluator dispatch (the real perf floor) — 2026-05-29

> Plan, not yet implemented. Born out of the "make `(sort …)` faster" thread:
> measuring that benchmark overturned its own premise and pointed at the
> evaluator's per-step dispatch cost as the one general lever worth pulling.
> The user's call: **stop the sort-specific work** (keep the Rust `%sort-asc`
> fast-path), **but plan and then execute an evaluator-dispatch campaign** —
> it pays off for every Brood program, which is the whole dogfood point.

## How we got here (the measurements that matter)

On the current build (post-ADR-055 copying GC), `(sort < coll)` on 10k random
ints takes **~880 ms** vs **~1.4 ms** for the Rust `%sort-asc` builtin — a ~630×
gap. Decomposing it (`/tmp/probe.blsp`, `/tmp/alloc3.blsp`, `/tmp/scalebench.blsp`):

| Suspected cause | Measured reality |
|---|---|
| Per-comparison `eval::apply` overhead | **~9%.** Multi-arity (ADR-047) already fixed this. Not the floor. |
| Allocation (`cons`) | **~140 ns/cons.** Cheap. The old "~1µs/alloc" figure conflated alloc with dispatch. |
| Copying GC re-tracing live set | **~935 ns/cons — but only above the 64K-cell `gc_floor()`.** The 10k sort's peak live (~40k) is *below* the floor, so **GC never fires for it.** Scales clean O(n log n) (88→98→103 µs/elem at 10k→30k→60k = just the log factor). |
| **Evaluator dispatch** | **The real floor.** Pure tail-recursive loop with no allocation costs **~400 ns/iteration** (`loop0` in `/tmp/alloc3.blsp`), i.e. ~40–130 ns per eval *step*. Merge-sort runs many steps per element (take/drop/append/reverse list-walking), so ~88–103 µs/element. |

**Conclusion:** the merge-sort algorithm is fine. The gap is the interpreter
dispatch tax on walking immutable lists. "Make sort faster the right way" =
**make the evaluator faster** — a general win, much bigger than the sort.

## The architecture, as it stands

`crates/lisp/src/eval/mod.rs` is a **pure tree-walking interpreter**: a single
`'tail: loop` (`eval`, line 103) re-interprets raw s-expression `Value`s every
time. `macroexpand_all` (`eval/macros.rs:130`) runs once per top-level form, but
there is **no further compile/resolution pass**. Closures store
`body: Vec<Value>` (raw forms, `core/value.rs:398`) and are re-walked on every
call. Consequences visible in the hot path:

1. **Variable lookup is O(lexical-chain depth).** `env_get` (`core/heap.rs:2437`)
   walks the parent chain; each `EnvFrame.vars` is a scan-from-end assoc list
   (`SmallVec<[(Symbol, Value); 4]>`, `heap.rs:120`). Every reference to a
   *global* (`cons`, `<`, `-`, `take`, … — most calls in a tight loop) walks the
   **entire** chain to `EnvId::GLOBAL`, then does a HashMap `globals_read().get`.
   This is the single biggest repeated cost.

2. **Special-form dispatch is HashMap + string-match.** `special_name(s)`
   (`eval/mod.rs:53`) does a `SymbolMap` (HashMap) lookup returning `&'static str`,
   and the caller `match`es on that **string** (`mod.rs:198`) — for *every*
   combination, even ordinary function calls (which fall through all ~25 arms to
   the empty-string default).

3. **`apply`-unfold checks by string.** The inline-apply loop compares
   `heap.native(id).name != "apply"` (`mod.rs:489`) — a string compare on every
   native call.

4. **Macro check + re-fetch per call.** Each symbol-headed call `env_get`s the
   head, checks `if let Value::Macro` (`mod.rs:450`), then evaluates args. The
   head lookup duplicates work and is subject to (1).

5. **Cons-spine re-parsing.** `if`/`do`/`let`/call all re-`uncons` the same
   spine every iteration; closure bodies are re-split every call.

## The plan — ordered by leverage ÷ effort

### Step 0 — A divan bench to lock the baseline ✅ DONE (2026-05-29)
Added `cons_build` (global-lookup + alloc heavy) and `sort_brood` (end-to-end
`(sort < …)`, comparator path) to `crates/lisp/benches/eval.rs`, alongside the
existing `sum_tail`/`fib`. **Locked baseline** (this machine): `sum_tail` 100k =
56 ms · `cons_build` 10k/100k = 12.4/150 ms · `sort_brood` 1k/5k = 77/451 ms ·
`fib` 25 = 153 ms. Steps 2/3 measure against these.

### Step 1 — Special-form dispatch: integer, not string ✅ DONE (2026-05-29)
Replaced the per-combination `special_name(s) -> &'static str` + string `match`
with a closed `enum SpecialForm` returned by the same fast integer-hashed
`SPECIAL_IDS` map, matched as a dense jump table (`eval/mod.rs`). `fn`/`lambda`
and `let`/`let*` collapse to one variant each. Suite green, no behaviour change.

**Measured: within noise** (if-heavy loop 406 ns/iter vs 404 ns before). Per-
combination cost is dominated by env-chain *lookups*, not special-form
*classification*, so the dispatch tweak alone doesn't move the needle — it's a
cleanup + the scaffold for Step 3's body pre-tagging. *Deferred from this step:*
the contiguous-low-id `s.0 < N` shape (kept the HashMap — it's already fast and
the win wasn't here), and the `apply` string-compare in the unfold loop (item 3:
assessed as near-free — `String`/`&str` `!=` is length-discriminated first, so
non-5-char names reject in one compare; not worth a struct field). The real
target is Step 2.

### Step 2 (original) — Lexical addressing — ❌ REJECTED AS SCOPED (2026-05-29)
Designed in full ([ADR-057](decisions.md#adr-057--lexical-addressing-o1-variable-lookup-eval-dispatch-step-2),
status *rejected as scoped*), then **not implemented**. A "what's the benefit?"
review measured the thing it targets and found it's **~6% of the eval loop**, not
the bottleneck — so ~1–1.5 weeks of high-churn work (plus the campaign's only real
data-race surface, the global-cell seqlock) bought under 10%. The design is sound
and on record; lexical addressing may return for free as a *by-product* of the
precompiled-body step below. See the ADR for the representation, the seqlock
global cells, and the resolver pass.

### Where the time actually goes (the measurements that re-scoped the campaign)
Same machine, current build, 2 M-iter loops, isolating one cost at a time
(`/tmp/{lookup,read,call}_cost.blsp`):

| component | cost | share of the ~400–480 ns/iter loop |
|---|---|---|
| **local** variable read | ~0 ns over a constant | — (env chains are shallow) |
| **global** variable read | ~9 ns over a constant | **~6%** (lookup — Step 2's target) |
| one **closure call** (`new_env` + `bind_params` + body) | ~52 ns | ~a third |
| **per-combination fixed overhead** | the remainder | **the majority** |

The per-combination overhead = the `tick` / `gc_due` / `soft_limit_hit` TLS guards
that run on *every* combination, plus spine `uncons`, argv `SmallVec` build, and
native dispatch. **That's the real lever, not lookup.**

### Step 2 (re-scoped) — Call path + per-combination overhead  *(the actual win)*
Lower-risk than the rejected lexical-addressing plan and attacks ~90% of the loop:
 - **`new_env` per call** — every function call allocates a fresh `EnvFrame` in the
   local arena (part of the ~52 ns/call). Pool/reuse frames, or use a frame stack,
   to cut a third of the per-call cost. (Mind the GC: frames are relocated by the
   copying collector — any pool must survive `arena_flip`.)
 - **Fold the three per-combination TLS guards into one.** `tick`, `gc_due`, and
   `soft_limit_hit` are three separate thread-local reads on every combination;
   combine into a single counter/check. Cheap, broad, no semantic change.
 - **argv build + native dispatch** — profile whether the `SmallVec` and
   `call_native` path have low-hanging fruit.
Each measured against the Step-0 baseline; profile first (`perf`/a flamegraph)
before cutting, to confirm the split above holds beyond microbenchmarks.

### Step 3 — Pre-tagged / precompiled closure bodies  *(the multiplier, ADR-worthy)*
If the call-path work isn't enough, the real structural fix is to stop re-walking
and re-classifying the s-expression spine every iteration: compile each closure
body once into a pre-tagged form (special-form tag + resolved operands), so the
`'tail` loop dispatches on a tag instead of `uncons`+classify. **Lexical addressing
falls out of this for free** (operands resolve to slots as part of the same pass) —
which is the only context in which ADR-057's work is worth doing. This is the
on-ramp to a bytecode/CPS evaluator; a separate, later decision.

### Step 4 (optional, separate) — GC for retain-heavy workloads
Out of scope for *dispatch*, but the same measurements surfaced it: above the 64K
`gc_floor`, copying re-traces survivors (~935 ns/cons). A generational nursery
(don't relocate short-lived garbage) would help large/server-scale structures.
Track separately; not needed for the sort or for typical programs.

## Guardrails (from CLAUDE.md / the invariants)
- **Proper tail calls are load-bearing** — `tail_calls_do_not_overflow` (sum to
  100,000) must stay green through every step. Don't turn tail positions into
  recursion.
- **All heap construction via `value.rs` helpers**; a new resolved-var `Value`
  kind goes through the same path and needs its `Tag`/bit.
- **Keep the core small** — prefer making the *evaluator* faster over adding
  special forms; the resolver is mechanism, not new language surface.
- **Measure every step** against the Step-0 baseline; record an ADR for Step 2
  (it's a real architectural choice) and dated `docs/devlog.md` entries.

## Pointers
- Hot path: `eval/mod.rs:103` (`'tail` loop), `:197` (special dispatch),
  `:436`–`531` (callee resolve + arg eval + call), `:534` (`apply`).
- Lookup: `core/heap.rs:2437` (`env_get`), `:120`/`:122` (`EnvFrame`/`EnvVars`).
- Special forms: `eval/mod.rs:45` (`SPECIAL_IDS`), `builtins.rs:3437`
  (`SPECIAL_FORMS`, shared with the LSP).
- Closures: `core/value.rs:394` (`ClosureArm`), `:423` (`Closure`).
- Prior perf work: ADR-047 (multi-arity dispatch — done; comparisons are no
  longer the floor, which is why this is next).
