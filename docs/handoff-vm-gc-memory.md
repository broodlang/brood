# Handoff — VM / GC / Memory: what's left

**As of 2026-05-31.** A cold-start map of the open work on the execution engine
(VM), the garbage collector, and memory. Most of this subsystem has *landed*; the
remainder is one real bug, a few deferred-by-design items, and perf follow-ups.
Authoritative detail lives in `docs/decisions.md` (ADRs cited inline),
`docs/memory-review.md`, `docs/memory-model.md`, and `docs/findings-closure-promotion-overflow.md`.

> **Stale memory notes — do not act on these without re-checking.** The
> auto-memory notes `no-gc-suite-memory` ("the tracing GC is still a no-op" —
> **false now**, the copying collector landed), and the older halves of
> `gc-entry-depth-leak` / `multi-arity-handoff` ("uncommitted") predate the GC
> and VM work below and are **superseded**. This doc is the current truth.

## Baseline — what is DONE (so you know the floor)

- **VM (ADR-076):** the engine is a **closure-compiling VM**, now the **default**.
  `BROOD_VM=0` is a kept tree-walker fallback; `crates/lisp/tests/differential.rs`
  pins VM≡tree-walker semantics. Lexical addressing, local-capture, prelude-closure
  compilation all done.
- **GC (ADR-035 → 055 → 058 → 061 → 072):** per-process **semi-space copying
  collector**, **generational** (nursery + tenured), **collects at any eval depth**
  (operand stack `roots` + `env_roots`), region-check rooting perf pass, `promote`
  cycle guard (forwarding table for env↔closure cycles during tracing).
  Observability: `(gc-stats)`, `(gc-collect)`, `(gc-trace on?)`, `BROOD_GC_TRACE`.
  Validated by `crates/lisp/tests/gc.rs` + `BROOD_GC_STRESS=1`/`BROOD_GC_VERIFY=1`
  under debug-assertions. **A full GC/mem-safety audit (2026-05-30) came back clean
  except the one bug below.**
- **Memory safety net (ADR-043):** byte stack-guard (E0044), soft mem cap (E0043),
  host-survival caps (5 GiB hard / 4 GiB soft — a backstop, *never* 0/unlimited).
- **Per-op cost (ADR-047):** native multi-arity dispatch kept `+`/`-`/`=` in Brood
  *and* fast — `(sum-to 100000)` 497 MB → 61 MB (8.1×).

## What's LEFT

### GC

1. **`promote` / `closure_to_message` cyclic-capture overflow — RESOLVED
   (2026-05-31).** Was the sole open memory-safety hole: `def`-ing or `send`-ing a
   closure that *captures another closure* stack-overflowed. Both sites are now
   fixed — `promote` got the two-pass back-patching `PromoteForward` table over
   `OnceLock` slabs (`def` path, commit `517d6d1`); `closure_to_message` is sound
   via capture-minimization + a `visited` cycle guard (`send`/`spawn` path). The
   `std/http.blsp` workarounds are reverted (spawn-per-connection + top-level
   router `def`s). Covered cross-process by `gc.rs::promotes_cyclic_local_closures_without_crashing`
   and `gc.rs::sends_closure_capturing_closure_without_crashing`, green under
   `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`. Full writeup + acceptance repros:
   `docs/findings-closure-promotion-overflow.md`. **GC has no known
   memory-safety holes left.**

2. **RUNTIME-region collector (deferred, ADR-072 / `live-editing.md` "Stage 5 later
   half").** The LOCAL-heap GC is done; the **shared mutable RUNTIME code region**
   (where `def`/hot-reload `promote`s code) is **never collected**, so it grows with
   hot-reload churn. Doesn't matter for short runs; matters for a long-lived,
   live-edited server. Its own stage — design not started.

3. **`macros.rs` rooting during expansion (deferred, low priority).** The compile
   pass opts *out* of GC via the `MACRO_BLOCK` guard (collection suppressed during
   `macroexpand_all`) rather than rooting its transients. Only needed if we ever
   want GC to fire *during* expansion. (Note: the macro *runtime* paths —
   quasiquote, the macroexpand fixpoint — ARE rooted; only the compile-pass walk is
   exempt.)

4. **Design follow-up — the "single-shot Rust primitive" rule (ADR-006 aligned).**
   ~~Move `quasiquote` off the runtime walker.~~ **DONE (2026-05-31, ADR-084).**
   The GC hazard only exists at a Rust frame that loops/accumulates across `eval`;
   Brood code is immune. `quasiquote` was the worst offender and is now a pure
   **compile/eval-time transform to builder code** (`expand_quasiquote`) — it calls
   no `eval`, so its bespoke operand-stack rooting (`expand_seq`/`teardown_err`) is
   deleted. The rule is recorded as ADR-084. **Still open (same rule, lower
   priority):** the `macroexpand` fixpoint and `reload-defs` are the remaining
   rooted-Rust re-entry points to shrink the same way.

### VM (ADR-076 — "still open, pure perf, deferrals already correct")

5. **`match*` / pattern-clause coverage** — the VM defers pattern-matching clauses
   to the tree-walker.
6. **Real-default `&optional` coverage** — also deferred to the tree-walker.
7. **Tree-walker retirement is blocked** on 5–6: the VM depends on the tree-walker
   for every deferred form, so the fallback can't be removed until the VM is a
   complete engine. Closing 5 and 6 is the path to that.
8. **Bytecode lowering — explicitly premature.** No profiling shows node-dispatch
   dominating; do *not* start this until a profile justifies it.

### Memory

9. **Revisit the ADR-043 caps now that the GC reclaims.** The 5 GiB/4 GiB caps were
   a pre-GC host-survival backstop. With reclamation working, decide whether to
   tighten them and/or introduce a real working-set budget. Per-run override:
   `BROOD_MEM_LIMIT`.
10. **Re-measure suite peak memory and refresh the stale note.** The old
    `no-gc-suite-memory` note recorded a ~18 GB transient peak in the *no-GC* era;
    with the collector live this should be far lower. Measure (`make test` /
    per-file) and either retire or rewrite that memory note.

## Suggested order if picking this up cold

1. ~~Fix #1 (promote cyclic-capture)~~ — **DONE (2026-05-31).** GC has no known
   memory-safety holes left.
2. ~~#4 (quasiquote → compile/eval-time transform)~~ — **DONE (2026-05-31, ADR-084).**
3. **#10 then #9** — cheap: measure suite peak, then right-size the ADR-043 caps.
4. **#5/#6 (VM coverage)** when perf wants it — unlocks #7 (retire the tree-walker).
5. **#2 (RUNTIME-region GC)** is the larger, lower-urgency item.

## Key files

- VM: `crates/lisp/src/eval/compile.rs` (+ `eval/mod.rs` tree-walker fallback),
  `crates/lisp/tests/differential.rs`.
- GC: `crates/lisp/src/core/heap.rs` (`collect`/`arena_flip`/`promote`/operand
  stack), `crates/lisp/tests/gc.rs`, `docs/memory-model.md`, `docs/memory-review.md`.
- Promote bug: `docs/findings-closure-promotion-overflow.md`.
- Caps: `crates/lisp/src/core/alloc.rs`.
