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

## Closures capture the global env *symbolically*

A subtlety found while building: a closure stores the env it was defined in, but
a **shared** top-level closure can't store a per-process `EnvId` — each process
has its own global env. So a closure defined at the global (parent-less) scope
captures the global **symbolically** (`Closure.env == None`); at call time
`bind_params` resolves it to *this process's* global env (the `Heap` knows its
own global). Closures that capture a *local* enclosing scope keep `Some(EnvId)`.
This is what lets one shared closure run correctly in any process. (Done as
step 2a — behaviour-preserving on its own.)

Note this also means the **global env stays per-process and mutable** (so `def`
works); it is *not* part of the immutable shared region. The global env's
*bindings* map symbols to (possibly shared) closure handles; the shared region
holds the closure *code*, not the global bindings table.

## `env_get` across regions

A process's innermost scopes are *local* `EnvId`s; the global env is per-process
(local). `env_get` dispatches per-frame by region — relevant once any frame is
shared. Today only the global frame's *contents* point into the shared region.

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
2. ✅ **2a** — closures capture the global env symbolically (`Closure.env:
   Option<EnvId>`, `None` = global; `Heap` records its process global).
   ✅ **2b** — the prelude (closures + code data + natives) is relocated into a
   shared `Arc<SharedCode>`, built once (lazily) via a builder heap +
   `freeze_as_shared_code` (re-tags handles local→shared). Each `Interp::new`
   shares that `Arc` and seeds a fresh local+mutable global env from the prelude
   bindings — *no prelude reload*. Behaviour-preserving (25 tests + suite green).
3. ✅ **`spawn` shares the code.** Since `Interp::new` no longer reloads the
   prelude (clones the `Arc` + copies the bindings map), spawning a process is
   cheap and the child can call any prelude/builtin via the shared region.
   Parent's *user* `defn`s are not visible to children — see the design change
   below.

### Design change: instances are independent (steps 4–5 dropped)

A later requirement settled the direction: **each instance must be independent —
updating a function in one runtime must *not* update other (connected) runtimes.**
That is the *opposite* of a shared mutable global. So the originally-planned
steps 4–5 (a shared mutable global + cross-process hot-reload propagation) are
**deliberately not built.**

What we keep is exactly right for that model:

- the **prelude is shared read-only** (`Arc<SharedCode>`) — an efficiency win that
  can't leak updates, since it's immutable (redefining a prelude fn just shadows
  it *locally*);
- each instance has its **own mutable global** function table — `def` updates
  only that instance; **single-instance hot-reload works via late binding**
  (re-eval the `def`, callers pick it up).

Optional, not yet built: **snapshot user defns at `spawn`** — copy the parent's
current user functions into the child so a spawned function can call them. A
*snapshot* (no later propagation) is consistent with instance independence. This
reuses the closure-shipping machinery; no shared mutable state.

(`6.` send functions — ship a closure's code + captured free variables — remains
possible if a concrete need arises.)

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
