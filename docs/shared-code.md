# Shared code (design)

> Status: **design, for review.** Not implemented. This documents **Option A**
> from the concurrency discussion — *true* shared code (not copy-on-spawn).

## Why

Today each process has its own `Heap`, its own copy of the prelude, and can't see
another process's `defn`s. We want Erlang's split — **share code, isolate data** —
which unlocks three things at once:

1. **Cheap spawn** — a new process shares the existing code instead of reloading
   the prelude.
2. **Spawned/sent functions resolve their global references** — a process can run
   any function you've defined, not just the prelude.
3. **Live redefinition propagates to every running process** — redefine a
   function and all processes see the new version. This is *the* point of the
   project ("edit the editor while it runs"), now across processes.

## The model

- A single shared **`CodeHeap`** (behind `Arc`) holds **code**: closures, the
  global environment, the symbols/lists/strings that *make up* code, and the
  native builtins.
- Each process keeps its own **data `Heap`** for everything it allocates at
  runtime (cons cells, vectors, strings, call-frame env scopes).
- A `Value` is still a `Copy` handle; we add a **region bit** so a handle knows
  which heap it points into. `Heap`'s accessors dispatch on it:

  ```text
  heap.pair(id):  if id is shared -> self.code.pairs[idx]   (read the Arc<CodeHeap>)
                  else            -> self.pairs[idx]         (local slab)
  ```

  Crucially, **`eval`'s signature does not change** — region routing hides inside
  `Heap` (which gains an `Arc<…>` to the shared code).

### Handle encoding

Reuse the top bit of the existing `u32` index as the region flag
(`SHARED = 1 << 31`); the low 31 bits are the slab index (2³¹ objects per region,
ample). Every handle type (`PairId`/`VecId`/`StrId`/`ClosureId`/`EnvId`/`NativeId`)
carries it. Local allocation sets it to 0; loading code sets it to 1.

## The hard part: shared reads + mutability + reference lifetimes

This is where the care goes. Two forces fight:

- **Reads are hot.** Every global lookup and every closure-body access during
  `eval` reads the shared region. We want those cheap, ideally returning `&str` /
  `&[Value]` without cloning.
- **Code is mutable.** `def`/`defmacro` and hot-reload *add to and rebind* the
  shared region at runtime, from any process thread.

A naive `Arc<RwLock<CodeHeap>>` makes reads take a read lock — and worse, an
accessor like `heap.string(id) -> &str` can't return a reference borrowed from a
temporary lock guard. So the design splits the shared region by mutability:

- **Immutable code region (the prelude + builtins):** loaded once at startup,
  then frozen. Stored as plain `Arc<CodeHeap>` — **lock-free reads**, and
  accessors can return `&str`/`&[Value]` tied to the `Arc` (valid as long as the
  process holds it). No lifetime problem.
- **Mutable code region (runtime `def`s, hot-reload):** an **append-only** store
  so existing references stay valid as new code is added (old code is never moved
  or freed — which also gives correct hot-reload: in-flight calls keep running
  the old closure, new lookups get the new one). Append-only + stable references
  means appends need only light synchronisation, and reads don't invalidate.
  Candidate representations: `Vec<Box<T>>` (boxes don't move on `Vec` growth) or a
  frozen/stable-append vector; the global **bindings** table is a
  `RwLock<HashMap<Symbol, Value>>` (write on `def`, read on lookup).

**Staging consequence:** ship the immutable-shared region first (proves the
mechanics, gives cheap spawn + shared prelude), and add the mutable-shared region
(runtime def to shared + cross-process hot-reload) as a later sub-step.

## Global environment & hot-reload semantics

- The global env lives in the shared region. Lookups walk a process's local
  call-frame scopes and then cross into the shared global frame.
- `def`/`defmacro` at top level **promote** the new closure's code into the shared
  region (its body is data — symbols/lists — so it's copied from the defining
  process's local heap into shared), then rebind the global symbol.
- **Hot-reload:** redefinition adds new shared code and rebinds; because old code
  is never freed (append-only), a call already running the old closure finishes on
  it, while every new lookup — in any process — gets the new version. This is the
  Erlang/Emacs semantics we want.

## `env_get` across regions

A process's innermost scope is a *local* `EnvId`; walking parents eventually
reaches the *shared* global `EnvId`. `env_get` dispatches per-frame: read local
frames from the local heap, the global frame from the shared region.

## Interaction with processes

- `spawn` no longer reloads the prelude — the child clones the `Arc` to the shared
  code (O(1)) and starts with a fresh, empty data heap. Cheap.
- **Sending functions becomes easy:** a *top-level* function's code lives in the
  shared region and its env is the shared global, so its `ClosureId` is valid in
  *every* process — sending it is just sending the (shared) handle. Closures that
  capture locals still need those captured values copied (free-variable capture),
  but the global/native-resolution problem disappears.

## Staged sub-steps (each keeps `cargo test` green)

1. ✅ **Region-tagged handles.** Handles carry a region bit (`SHARED_BIT`); a
   `SharedCode` region (mirroring the heap slabs) sits behind `Arc` on the
   `Heap`; accessors dispatch on the bit. Shared region starts **empty** → all
   reads route local → behaviour identical (25 tests + suite green; `Heap` stays
   `Send`). The safe foundation.
2. **Move startup code to the immutable shared region.** Load prelude + natives +
   the global env into `SharedCode`; the root env is shared. Runtime allocation
   stays local. Single-process behaviour unchanged.
3. **`spawn` shares the code.** Children clone the `Arc` instead of reloading the
   prelude → cheap spawn, and spawned functions can call prelude/builtins via the
   shared globals. (User defns still not visible until step 5.)
4. **Mutable shared region.** `def`/`defmacro` promote code to shared and rebind
   the shared global table (append-only code + locked bindings). Live redefinition
   works within a process.
5. **Cross-process hot-reload.** Redefinition is visible to running processes;
   add a test. Now spawned/sent functions see all user defns.
6. **Send functions** (separate doc/roadmap item): shared `ClosureId` + captured
   free variables.

## Risks

- Largest change since the arena migration; region dispatch is pervasive but
  mechanical (step 1 is a behaviour-preserving no-op, which de-risks it).
- The reference-lifetime issue around shared reads is the real subtlety — handled
  by the immutable-vs-append-only split above; if the append-only store proves
  fiddly, the fallback is clone-on-shared-read (correct, slower) for the mutable
  region only.
- Lock/append synchronisation on the mutable region; the immutable region (the
  common case, prelude/builtins) stays lock-free.

## Decision / next step

Approach **A** is chosen. Build it in the staged sub-steps above, beginning with
**step 1** (region-tagged handles, shared region empty — behaviour-preserving).
Each step lands behind `cargo test` + the Brood suite.
