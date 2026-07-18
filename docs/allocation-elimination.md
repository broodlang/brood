# Allocation elimination — the data-structure gap (lever 2 scope)

> **Status: SCOPE / design (2026-06-16).** Not yet implemented. The second big lever after the
> JIT call-path work (`docs/jit-optimizing-tier.md`). Targets the benchmarks bound by heap
> allocation + per-element heap-FFI + GC traffic: **bintree (8.3×), nqueens (10×), reduce (2.6×),
> sort (2.4×)**, and list code generally. Aligns with `modern-perf-bets` #2 (escape analysis) +
> #6 (inline small vectors) and the FBIP/Perceus bet (#1).

## 1. What these benchmarks are actually bound by (honest, from 2026-06-16 profiling)

Unlike spawn (a single clean cause), this cluster is **mixed-bound** — calibrate expectations
accordingly:
- **bintree**: 57% `vm_run_bc` (its `check` bails the JIT's `2call+walks-structure` gate, so it
  *interprets* the tree walk) + per-node `make_vector2`/`vector_ref`/`alloc`/GC-copy (~5% +
  GC). Re-admitting `Cons` to the JIT gave ~0; relaxing the gate so `check` JITs gave only ~4%.
- **nqueens**: 100% interpreted (`jit_native=0` — every arm bails: `safe?` on the benefit gate,
  `solve` on `MakeClosure`, the `reduce` closure on `Cons`); per-node `reduce`→closure dispatch
  via `vm_apply` + `cons`.
- **reduce/sort/lists**: HOF-closure dispatch + `cons` allocation + list traversal (car/cdr FFI).

So the full win on these needs **both** lever 1 (dispatch — inlining) *and* lever 2 (allocation).
Lever 2 alone yields **incremental** gains; it is not a single-commit 3.8× like the spawn lever.
This doc scopes the allocation half.

## 2. How allocation works today (grounded in `core/heap.rs`)

LOCAL data lives in typed **slabs** (`Slabs`, `heap.rs:441`), each a `Vec` indexed by a handle's
slab index; the GC is a **generational copying collector** that relocates survivors into fresh
slabs and drops the old slabs wholesale.

- **Pairs** (`pairs: Vec<(Value, Value)>`, `alloc_pair` `heap.rs:1652`): a cons cell is an inline
  `(Value, Value)` tuple in the slab — **48 B** (two 24-B `Value`s), **no double-allocation**. A
  list of N is N slab entries. `cons` cost = one `Vec::push` (amortized); GC-copy = copy 48 B.
- **Vectors** (`vectors: Vec<Vec<Value>>`, `alloc_vector` `heap.rs:1657`): a vector is a slab
  entry that is **itself a heap `Vec<Value>`** → **double allocation** (the slab slot's 24-B Vec
  header + the inner heap buffer) and **double GC cost** (relocating a vector allocates a fresh
  inner `Vec` + copies the elements). `[a b]` (bintree's node) pays this per node.
- **GC forwarding** (`FlushForward`, `heap.rs:5476`): **eight per-type `HashMap<u32,u32>`**
  forward tables, **allocated fresh per collection**. Collection-heavy workloads (bintree) churn
  these maps + hash every survivor handle.

## 3. The sub-levers (each helps a different subset)

**A. Inline small vectors** — kill the vector double-allocation. Store a small vector (≤ N elems,
e.g. N=4) inline in the slab instead of as a heap `Vec`. Representation options: a slab of
`enum SmallVec { Inline(u8 len, [Value; N]), Heap(Vec<Value>) }`, or a separate fixed-arity
tuple slab for the common `[a b]`/`[a b c]`. **Helps:** bintree (its nodes are `[l r]`), every
vector/tuple-heavy path — halves their alloc + GC-copy cost. **Effort:** medium (touches
`alloc_vector`, the `vector`/`vector_ref` accessors, the JIT's `make_vector2`/`vector_ref`
lowering, GC `flush` for vectors). **Risk:** medium — pure data-structure change, fully caught by
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` + the differential. **The cleanest bounded first win.**

**B. Escape analysis + scalar replacement** — the strategic structural bet. Prove a value
(vector/pair/closure) **does not escape** its creating arm (never stored into a heap cell that
outlives the arm, never returned, never sent) → don't heap-allocate it: keep its fields in
registers/slots (scalar replacement) or a stack slot. **Immutability makes EA cheap**
(compute-frontier §6): no alias analysis — a value can only be reached through the references the
code explicitly creates, so escape = "reaches a return / a store into something that escapes / a
call arg that escapes". The JIT's `Op::Handle` 3-register model is *already* intra-block scalar
replacement (a `cons`/`car` result lives in registers until stored); generalize "must not cross a
safepoint-live position" into an **escape lattice** over the arm. **Helps:** any transient
intermediate — `(let (p [a b]) (+ (nth p 0) (nth p 1)))`, a tuple returned-and-immediately-
destructured, a short-lived cons in a fold. **Effort:** high (a new analysis pass + JIT
integration). **Risk:** medium (not-allocating can't corrupt — a mis-proof just keeps the alloc).
BEAM does none of this — it's a structural edge, not just catch-up.

**C. FBIP / Perceus reuse** — in-place reuse when the producer is the value's *sole* consumer
(rewrite allocate-fresh → overwrite-in-place). Brood already hand-built the safe nucleus: the
`champ_assoc` **watermark** + `map_from_pairs`/`%map-into`. Generalize it into a reuse pass.
**Helps:** map/update-heavy code, the CHAMP path (`wordcount`). **Effort:** high. **Risk:** HIGH —
Brood is copying-GC not refcounted (novel integration), and a mis-proved reuse writes a shared
PRELUDE/RUNTIME value → cross-process corruption that `BROOD_GC_VERIFY` does NOT catch →
**differential testing is the only net**. Spike at construction chokepoints first. Strategic;
defer behind A/B.

**D. One-pass list builders** — `map`/`filter` reverse-accumulate then `reverse` (2N cons,
`prelude.blsp`). A one-pass kernel builder (like `map_from_pairs` for maps) → N cons. **Helps:**
reduce/map/sort/nqueens (all build lists). **Effort:** medium. **Risk:** medium — the builder
applies a *user fn* mid-build, so it must survive a GC-during-apply (the hazard that blocked the
earlier task-#12 spike); needs the watermark's build-survival discipline for lists. A Rust-kernel
builder that returns a fresh immutable list (never a mutable `Value` — ADR-112) is the shape.

**E. GC forward-table: `Vec<u32>` not `HashMap`** — replace the eight per-collection
`HashMap<u32,u32>` (`FlushForward`) with a `Vec<u32>` indexed by slab index (sentinel = not-yet-
copied), or an in-situ forwarding word. **Helps:** every collection (GC-heavy bintree most).
**Effort:** low-medium. **Risk:** low (localized to the GC copy path; `GC_VERIFY` is the gate).
**A cheap, broad first win independent of the others.**

## 4. Recommended phasing

- **Phase 0 — E (GC `Vec<u32>` forward tables).** Low-risk, broad, cheap; removes per-collection
  HashMap churn. Good warm-up that touches the GC copy path you'll revisit for A.
- **Phase 1 — A (inline small vectors).** The cleanest bounded data-structure win; directly halves
  bintree's per-node alloc + GC cost. Pairs already inline, so this closes the vector gap.
- **Phase 2 — D (one-pass list builders).** Cuts map/filter to N cons; helps the list cluster.
  Needs the GC-during-apply build-survival discipline.
- **Phase 3 — B (escape analysis + scalar replacement).** The structural bet; the biggest ceiling
  but the most work. Hosts on the JIT (extends `Op::Handle`).
- **Phase 4 — C (FBIP/Perceus reuse).** Highest risk/reward; spike narrowly first.

Measure each against the target benchmarks; remember the cap is the mixed-bound nature (§1) — pair
with lever-1 inlining for the full nqueens/bintree win.

## 5. Validation (per increment)

Allocation/GC changes are caught by `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` (walks the whole live
graph at every safepoint) — **non-negotiable** for A/D/E. **C is the exception**: a mis-owned
reuse write is invisible to `GC_VERIFY`, so the **JIT≡tree-walker + VM≡tree-walker differential
corpus** (run cons/map/vector shapes through both engines) is the net — plus the full in-language
suite (cross-process send/promote round-trips a reused value) under stress. Then `make test`, the
benchmark A/B, and a checksum-parity check on the data-structure benches. Per ADR-112: a builder
is a **GC-quiet in-place build inside one Rust builtin returning a fresh immutable `Value`** —
never a mutable `Value` the language can observe.

## 6. Key files & symbols

- `crates/lisp/src/core/heap.rs` — `Slabs` (`:441`, the `vectors: Vec<Vec<Value>>` to inline),
  `alloc_pair`/`alloc_vector` (`:1652`), `FlushForward` (`:5476`, the HashMap→Vec target),
  `flush_value`/`flush_pair`/the vector flush (`:5598+`), `collect`/`minor_collect` (`:4834`),
  the per-deref epoch tripwire (`vector`/`pair` accessors).
- `crates/lisp/src/eval/compile/` — the JIT's `make_vector2`/`vector_ref`/`cons`/`car`/`cdr`
  lowering (must follow an inlined-vector repr), `Op::Handle` (the scalar-replacement seed for B),
  `chunk_walks_structure` / the benefit gates (interact with the mixed-bound §1 picture).
- `std/prelude.blsp` — `map`/`filter`/`reduce` (the 2N-cons builders for D), `sort`.
- `crates/lisp/src/core/heap.rs` — the watermark FBIP nucleus for C: `champ_assoc` (`:1804`, the
  `is_owned`/watermark write modes) + `map_from_pairs` (`:2198`, the GC-quiet in-place build);
  `crates/lisp/src/core/map_champ.rs` is the CHAMP trie node layout underneath.
- Background: `docs/compute-frontier.md` §6 (why EA is sound under immutability), `docs/transients.md`
  (the watermark), `docs/decisions.md` ADR-112 (immutability absolute — the builder constraint).
