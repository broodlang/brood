# Design note — fast immutable-map folds via a linear-accumulator → Table rewrite

**Status:** design, not yet implemented. Targets the `wordcount`-class gap (a
streaming fold that builds an immutable map one update at a time).

**Goal:** make a hand-written immutable-map fold — e.g.

```clojure
(defn gen (x m i)
  (if (>= i n) m
    (let (x2 (lcg x) key (rem x2 kk))
      (gen x2 (map-int-add m key 1) (+ i 1)))))   ; 750k updates
```

run at mutable-`Table` speed (~150 ms for the benchmark) while keeping immutable
semantics, **without** a new user-facing API and **without** touching the GC.

---

## 1. The problem, measured

For the benchmark suite's `wordcount` (750k LCG keys tallied into a ≤1000-entry
map), best-of-5, 4-core pinned:

| variant | wall | note |
|---|---|---|
| JIT'd loop + arithmetic, no map | ~30 ms | the loop itself is free (tier-1 JIT) |
| 750k `map-get` (walk depth-3, **no alloc**) | ~120 ms | walk + call + loop |
| 750k `map-int-add` (walk + **path-copy alloc**) | ~430 ms | the benchmark |
| 750k `table-incr` (in-place mutate) | ~150 ms | the mutable ceiling |
| Elixir `Map.update` (same algorithm) | ~182 ms | |

So **~310 ms of the 430 ms is path-copy allocation**: each immutable update
allocates 3 fresh CHAMP nodes (leaf + 2 ancestors) — ~2.25M node allocations,
each copying 24-byte `Value`-based entries. Reads are cheap; the cost is purely
the *allocation volume* of the persistent update. Elixir is ~2.4× lighter per
update (8-byte tagged words + a decade-tuned map), which is the whole gap.

### What does NOT work (ruled out empirically)

- **Bigger CHAMP node inline capacity** (`SmallVec<[_;4]>` → `[_;16]`): no
  speedup, +40 MB RSS. The build uses mimalloc, so the spill `malloc`s were
  already cheap; the cost is allocation *count/copy*, not `malloc`.
- **Larger GC nursery floor** (`BROOD_GC_FLOOR`): *slower* (0.43→0.97 s). The
  adaptive nursery is already optimal; a bigger one thrashes cache. GC is 36
  cheap minors, zero majors — not the bottleneck.
- **The existing watermark transient** (`champ_assoc`/`is_owned`, behind
  `%map-into`): only sound *inside one builtin* ("GC cannot fire mid-build"),
  and `is_owned` excludes old-gen nodes by construction. A streaming user loop
  spans 36 GC safepoints and tenures the accumulator, so the watermark can't
  apply. The GC-surviving transient that *could* (`Value::Transient` +
  `transient_reanchor`) was deliberately removed per ADR-026.

---

## 2. The idea

The mutable ceiling (`Value::Table`, reference-semantics, in-place,
**already GC-safe**) is ~150 ms — faster than Elixir. The only reason the fold
can't use it is that the source is written against the immutable map API.

So: **a compiler pass that, when it can prove the map accumulator is _linear_,
represents it internally as a private `Table` and snapshots it back to an
immutable map at the boundary.** No new runtime data structure, no GC work — it
reuses `Value::Table`, which already survives collection.

This is ADR-026-clean: `Table` is the sanctioned mutable structure; the
observable result is an ordinary immutable map (`table-snapshot`); and linearity
guarantees the in-place mutation is never observable.

---

## 3. Soundness — the key move is a defensive entry-copy

The accumulator in the target pattern is a **function parameter** threaded
through a self-tail-recursive loop. Naively, deciding it's safe to mutate would
need interprocedural no-alias analysis (the caller might hold the same map).

We avoid that entirely:

> **On entry, copy the input map into a fresh private `Table`.** The input map
> is never mutated; the `Table` is provably unaliased because the function just
> created it.

Given the private Table, soundness reduces to a single **intra-procedural**
check: within the function body, the accumulator slot must flow *only* through

1. whitelisted kernel map ops whose result rewrites the same slot
   (`map-int-add`, `map-assoc`, `map-dissoc`), or value-returning reads
   (`map-get`, `map-count`, `map-contains?`); **and**
2. the self-tail-recursive call (the loop back-edge), in the same arg position;
   **and**
3. the function's return value (the escape point).

If the slot is *ever* used any other way — passed to a non-whitelisted function,
stored to another slot or a global, captured by a closure, sent to a process,
returned as part of a larger structure, or compared for identity — the pass
**bails** and the function compiles unchanged. False negatives are fine; a false
positive would be silent corruption, so the whitelist is closed and
conservative.

Because the entry-copy defends aliasing and the intra-procedural check defends
escape, the rewrite is sound regardless of how callers use their maps.

---

## 4. The rewrite

Conceptually, transform

```clojure
(defn gen (x m i)
  (if (>= i n) m
    (let (... ) (gen x2 (map-int-add m key 1) (+ i 1)))))
```

into

```clojure
(defn gen (x m i)
  (let (m (if (table? m) m (%table-from-map m)))   ; idempotent entry-copy
    (if (>= i n)
      (%table-snapshot m)                          ; exit: Table → immutable map
      (let (...)
        (%table-incr m key 1)                      ; in-place mutate (discard result)
        (gen x2 m (+ i 1))))))                      ; thread the SAME table handle
```

Notes:

- **The entry-convert is idempotent** (`if (table? m) …`). A self-tail-call
  re-enters the arm from the top, so the convert sits inside the loop — but
  after the first iteration `m` is already a `Table`, so the guard is a single
  predictable type check (~free). It also correctly handles the `n=0` case (the
  guard runs before the base-case return, so the return always snapshots a
  Table).
- **`map-int-add` becomes an in-place mutate whose result is discarded**, and
  the self-call threads the *same* table handle (`Local(m_slot)`). Reference
  semantics make the threading a no-op write of the same handle.
- The `table-incr`/`table-put`/`table-delete` builtins currently return the new
  *value*, not the table. Two clean options: (a) add internal `%table-incr!` /
  `%table-put!` / `%table-delete!` that return the table, or (b) at the IR level
  emit the mutation, `Pop` its result, and push `Local(m_slot)` for the
  self-call arg. (b) needs no new builtins.

### IR-level shape (`crates/lisp/src/eval/compile/`)

The accumulator is a frame slot; the self-tail-call is `Inst::SelfCall { argc }`
re-entering the arm in place (`ir.rs`). The pass works on a single compiled arm:

1. **Identify candidate slot.** A parameter slot `s` that (a) is an argument to
   a whitelisted `Inst::Call { head: Some(map-op) }` in the first operand
   position, and (b) is passed at position `s` of the arm's `SelfCall`. Restrict
   v1 to **self-tail-recursive arms** (a `SelfCall` exists) — that is exactly
   the fold/loop shape and bounds the per-call entry-copy cost.
2. **Verify linearity.** Walk the arm's `Inst` stream; every `Inst::Local(s)`
   read must be immediately consumed by a whitelisted op or be the SelfCall
   arg-`s` / a return. Any other `Local(s)` (e.g. feeding a generic `Call`,
   `SetLocal(other)`, `MakeClosure` capture, `MakeVector`/`MakeMap`) → bail.
3. **Rewrite.** Insert the guarded entry-convert at arm start; swap whitelisted
   map-op `Call`s on `s` to the table equivalents; wrap the return of `s` with
   `%table-snapshot`. Leave the self-call threading `Local(s)`.

Keep the pass behind the existing `def`-deopt machinery: a `def` rebind of the
function recompiles the arm (the pass re-runs), and the transform is a pure
function of the arm, so hot-reload holds.

---

## 5. Op mapping

| immutable op | Table op | notes |
|---|---|---|
| `map-int-add m k d` | `table-incr m k d` (return table) | counts |
| `map-assoc m k v` | `table-put m k v` (return table) | |
| `map-dissoc m k` | `table-delete m k` (return table) | |
| `map-get m k default` | `table-get m k default` | returns a value; same semantics |
| `map-count m` | `table-count m` | |
| `map-contains? m k` | `table-has? m k` | |
| entry: input map | `%table-from-map` (deep copy) | once per call |
| exit: return | `table-snapshot` | Table → canonical immutable map |

`%table-from-map` and `%table-snapshot` are kernel helpers (snapshot already
exists as `table-snapshot`; `table-from-map` = `(reduce table-put (table) (map-pairs m))`
or a direct kernel builder).

---

## 6. Edge cases / risks

- **Table deep-clone-on-read.** `table-get`/`table-snapshot` copy keys/values
  in and out. For immutable values this is observationally equivalent (maps
  compare by value), so the *result* is correct. For perf, restrict v1 to
  accumulators whose values are immediates (ints) — wordcount's case — to avoid
  cloning large values per read; widen later.
- **Table key restrictions.** `crate::table::check_key` may reject some keys
  that maps accept. If the analysis can't prove keys are valid table keys
  (int/string/keyword), bail.
- **Multiple accumulators / nested folds.** v1 handles one accumulator slot per
  arm; bail if two candidate slots interact.
- **`map?`/`type-of` on the accumulator.** Would observe `:table` not `:map` →
  such a use is non-whitelisted → bail (the analysis already excludes it).
- **Generic prelude `get`/`count`/`assoc`** (variadic, type-dispatching) are
  *not* whitelisted (only the 3-arg kernel ops are); a generic use → bail.
- **Profitability.** Restricting to self-tail-recursive arms means the
  entry-copy is amortized over the loop. A function called many times with a
  large pre-existing map and few updates would pay the O(map) copy without a
  win; the self-recursion restriction makes that rare, and a later heuristic
  (estimated iteration count, or "accumulator initialized empty at the hot call
  site") can gate it further.

---

## 7. Test plan

- **Equivalence:** for every transformed function, the result map must `=` the
  untransformed result. Add a test harness that runs key folds both ways
  (`BROOD_NO_LINMAP=1` to disable the pass) and asserts equality.
- **Aliasing safety:** a caller that holds the input map and inspects it after
  the call must see it unchanged (proves the entry-copy).
- **Escape safety:** functions that store/capture/return-embedded the
  accumulator must NOT be transformed (assert via a debug counter that the pass
  bailed).
- **GC:** run the suite under `BROOD_GC_STRESS=1` and `BROOD_GC_VERIFY=1` (the
  accumulator Table must survive collections — it already does, but verify).
- **Hot reload:** redefining a transformed function mid-run must stay correct
  (`def` deopt + recompile).
- **Checksum:** `wordcount` must still print `374854840`.

---

## 8. Expected result

`wordcount` 430 ms → ~150 ms (the Table ceiling), **ahead of Elixir (182 ms)**,
while remaining a from-source immutable-map fold. The pass generalizes to any
linear map fold (`group-by`-style builds, accumulating assoc loops), not just
counting — its value is broader than the benchmark.

---

## 9. Staging

1. Kernel helpers: `%table-from-map`, confirm `table-snapshot`; optional
   return-the-table mutation variants.
2. The intra-procedural linearity analysis (conservative; default-bail) with a
   debug counter + `BROOD_NO_LINMAP` kill switch.
3. The IR rewrite (entry-convert / op-swap / exit-snapshot).
4. Equivalence + aliasing + GC-stress tests, then measure.

Develop in isolation (a worktree off a clean commit) — the pass is
compiler-only (`eval/compile/`) and must not entangle with unrelated `heap.rs`
work.
