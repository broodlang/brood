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

### Step 2 — Lexical addressing: O(1) variable lookup  *(big, biggest win)* — DESIGNED
**Full design: [ADR-057](decisions.md#adr-057--lexical-addressing-o1-variable-lookup-eval-dispatch-step-2)**
(status: proposed/draft). Summary of what was decided there:

 - **Representation:** two internal `Value` variants — `LocalRef{up,idx}` and
   `GlobalRef{slot,sym}` — produced only by the resolver, consumed only in `eval`,
   and **carved out of the public type universe** (omitted from `ALL_TAGS`/
   `type-of`; reader never makes them; printer/equality/`to_message`
   `debug_assert!`-unreachable). Chosen over (A) full public Value variants
   [type-universe pollution for things that are code, not data] and (B) internal
   `(%local …)` lists [alloc + re-walk per ref, likely no faster].
 - **Globals → seqlock cells:** an append-only slot vector (`boxcar::Vec`, like the
   code region) + the existing `Symbol→slot` map; `def` writes the cell, a resolved
   `GlobalRef{slot}` read skips the *hash + map lock*. Each cell is a **seqlock**,
   not a bare `Value` — a `Value` is multi-word, so an unsynchronized read racing a
   `def` is a torn read / UB. Seqlock read = two acquire loads + compare (cheaper
   than today's rwlock read-acquire) and still skips the hash. **Late binding/hot
   reload preserved** — slot is stable, re-`def` release-publishes an
   already-promoted immutable handle, a running process sees it (ADR-013). Forward
   refs reserve an empty cell.
 - **Pass:** thread a compile-time lexical scope through `macroexpand_all` (which
   already separates binders from refs and leaves quote opaque); resolve *after*
   full expansion; idempotent; runs on dynamically `eval`/`load`ed code too.
 - **Wrinkles handled:** `letrec` double-push → pre-define-then-update-in-place
   (N stable slots); dynamic vars consult the dynamic stack first; **same-node
   `send` keeps `GlobalRef{slot}`** (same runtime, same table), only the **cross-node
   dist wire** downgrades `GlobalRef`→`sym` (independent runtimes — hence `GlobalRef`
   keeps `sym`).

**Rollout (each measured against the Step-0 baseline):**
 - **2a — locals only.** No `RuntimeCode`/message/hot-reload risk; validates the
   resolver + `Value` carve-out + idempotency on the safe path. (Global-heavy
   `sort`/`cons_build` move little here — expected.)
 - **2b — global cells + `GlobalRef`.** The high-impact stage; gated on the
   hot-reload + cross-process suites.
 - **2c — dynamic-var fallback + closure-shipping downgrade** + multi-node tests.

Promote ADR-057 to *accepted* once 2a lands green with a recorded delta.

### Step 3 — Pre-split / pre-tag the loop body  *(medium)*
Once Step 2 exists, the body of a closure (and `if`/`do`/`let` operands) can be
stored **pre-parsed** — special-form tag + operand slots resolved — so the
`'tail` loop dispatches on a tag instead of re-`uncons`ing and re-classifying the
spine each iteration. This is the on-ramp to a real bytecode/CPS form without
committing to one yet. Stop here if the numbers are good enough; the jump to full
bytecode is a separate, later decision (ADR-worthy).

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
- Prior perf work: `docs/handoff-multi-arity.md` (ADR-047, multi-arity dispatch
  — done; comparisons are no longer the floor, which is why this is next).
