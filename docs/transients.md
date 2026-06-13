# Design note — internal transients for fast map building

**Status:** implemented (Phase 1 — bulk build). Supersedes the
externally-suggested "user-facing transients" sketch. The read-modify-write case
(`frequencies`/`group-by`/the Conway tally) remains deferred and is the open
follow-up question (see "Deferred" below). Not yet an ADR.

**Landed:** `champ_assoc` is parameterized over a `watermark: Option<usize>`
(copy-on-write when `None`; transient in-place when `Some`); `Heap::map_from_pairs`
+ `map_from_pairs_into` set the watermark and fold; the `%map-into` builtin backs
the prelude's `into` (map branch) and `zipmap`. Measured ~1.6× on a 50k-entry
build call (~7% end-to-end on build-heavy programs); no small-map regression.
Covered by `tests/transients_test.blsp` (equivalence, input-immutability,
cross-process round-trip, hot-reload), green under `BROOD_GC_STRESS=1`/`GC_VERIFY`.

**Update (Phase 2 shipped — this note's rejection was overruled).** The
user-facing surface *was* subsequently added: `transient` / `assoc!` / `dissoc!`
/ `persistent!` as builtins over a real `Value::Transient` (`heap.rs`
`alloc_transient`/`transient_assoc`/…), with `get`/`count`/`contains?` dispatching
to it for live reads. The prelude's multi-assoc combinators (`merge`,
`merge-with`, `select-keys`, `update-vals`, `update-keys`) build through it
instead of folding immutable `map-assoc` (measured ~1.4–1.6× on large inputs).
Unlike the internal builder, a `Value::Transient` **can be held across a
safepoint**, so the "GC cannot fire mid-build" simplification below does *not*
apply to it. Two mechanisms keep it correct:

1. **Epoch re-anchor** (`transient_reanchor`): a collection bumps `local_epoch`
   and relocates the cell's owned nodes; on the next `assoc!` the stale watermark
   is reset to the current slab length, so relocated nodes read as non-owned and
   are path-copied once (re-establishing owned nodes) rather than mutated by a
   bogus index.
2. **Transient write barrier** (`remembered_transients`): a transient *cell*
   tenured to the old gen is mutable, so a later `assoc!` repoints its `root` at a
   fresh *young* node — an OLD→YOUNG edge a minor *flip* would otherwise skip
   (the flip relies on the immutable-data invariant that old never points young),
   leaving the cell's root dangling. The mutating ops record an old cell in
   `remembered_transients`; the next minor flushes its root in place (dropped on a
   tenure, retained on a flip), exactly as `remembered` does for env frames. The
   tenure path is regression-tested in `gc.rs::transient_survives_tenure_then_flip`
   and `transients_test.blsp` (the "tenures past min_tenure" case). Before this
   barrier the surface corrupted under GC on allocation-heavy builds (a silent
   use-after-GC in release; an epoch-tripwire panic in debug).

## Problem

Building a map by repeated `assoc` is O(N log₁₆ N) *allocations*: every
`Heap::map_assoc` (`core/heap.rs:1343`) runs `champ_assoc`, which path-copies
each node from root to the touched leaf — cloning the node's `data`/`children`
`SmallVec`s and pushing a fresh `MapNode` into the `maps` slab per level
(`alloc_map_node`, `heap.rs:1749`). At N entries that is ≈ N × depth fresh
nodes plus the matching GC churn. This is the dominant cost in `map_from_pairs`
(`heap.rs:1676`) and in every prelude builder that folds `map-assoc`/`assoc`
(`into`, `zipmap`, `frequencies`, `merge`, `update-vals`, `group-by`,
`distinct`, `select-keys` — all in `std/prelude.blsp`).

The downstream report from `brood-life` measured ~12 µs/op on this path and
proposed two fixes: (1) a native `%life-step` primitive, and (2) Clojure-style
**user-facing transients** (`transient` / `assoc!` / `persistent!` as builtins,
plus a `Value::Transient`). This note rejects both as proposed and specifies a
third that fits Brood's invariants.

## Why the two suggested fixes are rejected

- **Native `%life-step`** violates ADR-006 ("write the language in the
  language") and the keep-the-core-small rule: it bakes a Conway's-Life
  operation into a general-purpose runtime. The report itself flags this as an
  architectural smell. Out.

- **User-facing transients** (`assoc!`/`persistent!` exposed to `.blsp`, a new
  `Value::Transient`) directly violate **ADR-026** (decisions.md:868). Its first
  bullet is absolute:

  > **Lisp data is immutable.** No primitive mutates a `Value`; this stays true.

  A Lisp-callable `assoc!` is a data-mutation primitive — exactly the class
  ADR-026 forbids and says "none may be added." It also reopens the bug class
  ADR-026 closed: the report's own caveats ("a transient must not be aliased or
  sent across processes… forbid sending a `Transient` or auto-`persistent!` on
  send") are the `Send`-per-process-heap and copy-on-send invariants breaking
  and being patched back by hand. ADR-026 *does* list "transients" as a deferred
  mitigation (decisions.md:925) — but the deferred thing is the *speed-up*, not
  a new mutable surface in the language.

## The fix: a transient that never escapes a builtin

Use the transient *technique* — mutate trie nodes in place instead of
path-copying — but confine it entirely inside the Rust kernel. The mutable
handle is a Rust local; it is never a `Value`, never reaches `.blsp`, never
crosses a process boundary. From the language's point of view every map builder
still "returns a fresh `Value`" — the in-place writes are an implementation
detail of *constructing* that fresh value, no different from `+` mutating a
register. ADR-026 is untouched (no amendment needed); ADR-006's two dogfooding
bars are both met — it's a general capability (every bulk map build gets
faster) that builds up a real internal primitive rather than a Rust escape
hatch.

### Why this is so much simpler than the external version

**GC cannot fire inside the builtin.** Per ADR-035 (decisions.md:1428), the
collector runs only at the outermost-eval safepoint, i.e. when `GC_BLOCK == 1`;
a running builtin holds `GC_BLOCK ≥ 1` from the eval that called it (ADR-035's
correctness sketch, point 3: "GC and builtin transients are mutually exclusive
on the stack"). `map_from_pairs` does not re-enter `eval`. Therefore **no
collection point exists between the start and end of the build**. Consequences:

- No `Value::Transient`, no `Tag`, no GC-tracer change — the report's "the
  tracer must walk `Tag::Transient`" and "GC during a transient" concerns simply
  don't arise.
- No rooting dance. Handles are slab indices (`MapId`), stable across the `Vec`
  growth that `alloc_slot!` may trigger, so even mid-build slab reallocation is
  fine.
- No owner-epoch field on `MapNode` (the report wanted `edit: Option<u32>` on
  every node — a permanent 4–8 byte tax on a hot struct). Ownership needs **no
  per-node bookkeeping at all** — see the watermark below.

### Ownership rule: a slab watermark (structural-sharing safety)

The build must never mutate a node that belongs to the *input* map (it may be
shared with other live maps, or sit in the read-only PRELUDE/RUNTIME regions).
So:

> A node may be mutated in place **iff this build allocated it.**

Two facts make this a single integer comparison, not a side table:

1. `alloc_slot!` (`heap.rs:801`) only ever **appends** to the nursery
   (`self.local.maps.push(...)`) — it never reuses a slot mid-call.
2. GC can't fire mid-build (above), so nothing else allocates into that slab
   during the build.

So record one **watermark** = `self.local.maps.len()` at build entry. Then:

```rust
fn is_owned(id: MapId, watermark: Option<usize>) -> bool {
    match watermark {
        // LOCAL nursery node allocated *after* the build began ⇒ ours.
        Some(w) => id.region() == LOCAL && !id.is_old() && id.index() >= w,
        None    => false,  // copy-on-write mode: own nothing
    }
}
```

Every input node (and any pre-existing nursery node) has `index < watermark` ⇒
not owned ⇒ copied. Every node the build allocates lands `≥ watermark` ⇒ owned ⇒
mutated in place on its next touch. `old`-gen and shared-region inputs fail the
`region/age` test outright. A `watermark: Option<usize>` is `Copy`, so it threads
through the recursion with no borrow gymnastics.

### Parameterize the existing `champ_assoc`, do not clone it

`equal` on maps is **structural**, not entry-set (`heap.rs` `map_equal`: "CHAMP
is canonical under structural equality, so two equal maps have identical trie
shapes"). So a transient build that produced even a *slightly* different shape
for the same entries would make `(equal transient-built cow-built)` return
**false** — a worse bug than iteration-order drift. The fix is to keep **one**
function making **all** the structural decisions (slot, bitmap, rank, insert
position, size delta), and branch only at the *write*:

```rust
fn champ_assoc(&mut self, id, key, val, hash, depth, wm: Option<usize>) -> MapId {
    let owned = Self::is_owned(id, wm);
    // ...identical slot/bit/rank/position/size computation as today...
    // each case:
    if owned {
        // mutate self.local.maps[id.index()] fields in place using the
        // positions just computed; return id unchanged. (Recurse FIRST, then
        // re-borrow the slot &mut — never hold &MapNode across &mut self.)
        return id;
    }
    // else: the existing copy-on-write arm, byte-for-byte unchanged → allocates
    // a fresh node via alloc_map_node. New callers pass wm=Some(..); every
    // existing caller passes wm=None, so `owned` is always false and the hot
    // CoW path is untouched (one perfectly-predicted branch).
}
```

`champ_split`'s sub-nodes need **no** changes: they're freshly allocated, so
their indices land `≥ watermark` and become owned automatically — the next assoc
into them mutates in place.

The structural decisions are shared (canonical shape guaranteed); only "mutate
field" vs "build-and-alloc node" differs. Shape fidelity across the two write
arms is then the one thing the differential test must lock down.

- For `{}` literals / `(hash-map …)` / `into {}` the build starts from
  `alloc_empty_map()` — a fresh root, `index == watermark`, owned — so *every*
  node is owned and the whole build is in-place after the first insert per path:
  the per-level `SmallVec` clone + slab push collapse to in-place `data.insert` /
  `children[j] = …`.
- For `(into <existing> …)` the existing root isn't owned, so the first assoc
  copies the root + its path (one normal path-copy), and all subsequent inserts
  reuse those now-owned nodes. Correct either way.

The mutation is confined to nodes created during this call, GC can't observe an
intermediate state, and the returned `Value::Map` is indistinguishable from the
copy-on-write result.

## Scope — what this speeds up, and what it honestly does not

**Accelerated (pure bulk insert, last-wins — no per-element Brood callback):**

- `map_from_pairs` itself → `{}` reader literals, `(hash-map …)`, message
  reconstruction (`process/message.rs`), macro map construction, `error.rs`
  maps — all callers at the `map_from_pairs` chokepoint benefit with **zero
  prelude changes**.
- Add a kernel builtin the prelude can route sequence→map builds through (e.g.
  `(%map-from-pairs seq)`): re-point `into {}`, `zipmap`, `select-keys` at it.

**NOT accelerated by the internal-only design — the read-modify-write case:**

`frequencies`, `group-by`, `merge-with`, and the `brood-life` `step` tally all
do `(assoc m k (f (get m k) …))` — they read the accumulator, run a **Brood
combine function**, and write back, per element. The internal builder can't
absorb that loop without calling a Brood closure per element, which:

1. re-enters `eval` (so the build is no longer a self-contained Rust call), and
2. because GC stays blocked for the builtin's whole duration, would accumulate
   the per-element garbage of a large reduction **with no collection point** —
   a memory spike on exactly the big inputs we care about.

So the internal transient deliberately stops at last-wins bulk inserts.
**Note for the `brood-life` consumer:** `step`'s neighbour tally is the
read-modify-write case, so Phase 1 speeds up `step`'s final `into {}`
result-build but **not** the tally itself — it will not on its own hit 60 fps.
Accelerating the tally needs one of the deferred options below; do not promise
the demo's hot loop from Phase 1.

## Deferred (needs a separate decision, gated on a real consumer)

The read-modify-write case has two possible answers, both deferred:

- **(A) A user-visible linear transient.** The genuinely powerful fix, but it
  requires *amending ADR-026* to admit a linear/affine mutable value with a
  defined contract (no aliasing, no send, `persistent!`-or-discard). Large
  decision; do not take it speculatively (ADR-011: defer power features).
- **(B) Targeted bulk primitives** — e.g. promote `frequencies`/`count-by` to a
  kernel primitive over an internal transient. Smaller, but each is a step
  toward domain-specific builtins; weigh against keep-the-core-small per case.

Recommendation: ship Phase 1, measure, and only revisit (A)/(B) when a concrete
workload justifies it.

## Risks & trade-offs

- **Immutability shifts from global to local.** Today immutability is true *by
  construction* (no mutating primitive exists). After this, there is exactly one
  place that writes a live `MapNode`, and the guarantee becomes a *local
  invariant* — "only mutate a node with `index ≥ watermark`." Observable
  behaviour is identical, but the property is now enforced by one function's
  correctness rather than by the absence of the capability. **Failure mode if
  the invariant breaks:** a write to a node with `index < watermark` would mutate
  a node shared with a live map — and PRELUDE/RUNTIME maps are shared read-only
  *across processes*, so the worst case is silent cross-process data corruption.
- **`BROOD_GC_VERIFY` does not catch a mis-owned write** — it checks handle
  bounds/epochs, not "did a builtin write a node it didn't allocate." The
  **differential test is the primary defense**; the watermark's "degrade to a
  copy, never to corruption" property (a too-low watermark only causes extra
  copies; only a *write* through a `< watermark` index corrupts, which the code
  structurally never does) is the secondary one.
- **GC-quietness is load-bearing and constrains Phase 2.** The whole "no rooting,
  no `Tag`" simplicity rests on the builder never re-entering `eval`. A future
  read-modify-write variant that calls a Brood combine fn breaks this *and*
  introduces an uncollectable-garbage spike on large reductions — it cannot just
  extend this builtin; it needs its own rooting story.
- **Small-map regression risk.** Most maps are tiny (`{}` literals, 2–4 keys).
  The watermark approach adds ~nothing (one integer + a predicted branch), so
  unlike a `HashSet` it shouldn't regress small builds — but confirm with the
  bench (`maps` group) that `{}`/small `hash-map` didn't get slower.
- **Standing complexity** in the most GC/sharing-sensitive part of the kernel:
  each `champ_assoc` case now has a CoW arm and an in-place arm. The "shared
  structural decisions, branch only at the write" structure keeps them honest,
  but it is more code to keep correct.
- **Scope:** does not touch `frequencies`/`group-by`/`merge-with`/the
  `brood-life` tally (read-modify-write, deferred above). If the motivating goal
  was that demo's hot loop, this alone won't deliver it.

**Decision gate:** this is a spike. Build it in a worktree, benchmark
build-heavy map workloads (`into`/`zipmap`/`{}`) against `main`, and keep it only
if the win is real and broad and the small-map case is unharmed. Otherwise drop
it — the global-immutability simplicity is worth a lot.

## Implementation steps

1. `core/heap.rs`: thread a `watermark: Option<usize>` through `champ_assoc`
   (CoW callers pass `None`; the new builder passes `Some(len)`), add the
   `is_owned` test and an in-place write arm per case. `champ_split` is
   unchanged. Add a transient `map_from_pairs`/`map_from_pairs_into` that sets
   the watermark and folds. Leave single-assoc `map_assoc` on `None` (a lone
   `assoc` has no reuse to exploit).
2. `builtins.rs`: register `%map-from-pairs` (initial-map + sequence → map) over
   the new builder; it reads the seq into `Vec<(Value, Value)>` then builds.
   (Same `def(heap, name, arity, sig, fn_ptr)` pattern as the existing
   `map_assoc`/`hash-map` entries around `builtins.rs:286`/`1976`.)
3. `std/prelude.blsp`: re-point `into` (map branch), `zipmap`, `select-keys` at
   `%map-from-pairs`. Leave `frequencies`/`group-by`/`merge-with` unchanged
   (deferred scope above).
4. Tests:
   - **Differential** (`crates/lisp/tests/differential.rs`): for random pair
     lists (incl. duplicate keys, hash-collision keys, and `into <existing>`),
     assert the transient build is `equal` to the fold-of-`map_assoc` result
     **and** has identical `map-pairs` iteration order (the CHAMP shape must
     match, not just the entries).
   - **Concurrency** (in-language, per the CLAUDE.md feature checklist): build
     maps via the new path in many `spawn`ed processes, `send` them across
     per-process heaps (proving `to_message`/`promote` round-trip the result),
     fan-in and compare. Run the suite green under `BROOD_GC_STRESS=1`.
5. Docs: tick `docs/roadmap.md`; dated `docs/devlog.md` entry; promote this note
   to an ADR in `docs/decisions.md` once Phase 1 lands (it records the
   "internal-transient, not user-facing" decision against ADR-026/006).

## One-line summary

Take the transient *technique*, not the transient *API*: mutate only
build-local trie nodes inside a single GC-quiet builtin, keep every Brood-facing
contract immutable, and accept that read-modify-write accumulation stays out of
scope until a real consumer justifies amending ADR-026.
