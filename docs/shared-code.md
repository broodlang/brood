# Shared code

> Status: **implemented.** This documents **Option A** from the concurrency
> discussion — *true* shared code (not copy-on-spawn). A runtime's inner
> processes share its live code + global table; redefining a function reaches a
> running process on its next lookup (live code reload across running processes).
> Separate runtimes (nodes) stay independent; data is never shared.
>
> This file is the design as it was staged, and "today" in the first sections means
> before the staged work. What shipped differs from that first sketch in these ways:
> the prelude region is `SharedCode` and the mutable one `RuntimeCode`
> (`core/heap/runtime_code.rs`) — `CodeHeap` was the design's name; handles are 64-bit
> with a 2-bit region tag; the RUNTIME region is no longer forever append-only, since a
> two-generation collector reclaims it (ADR-091, §"How it's built"); and `spawn` copies a
> capturing closure's values instead of promoting it (ADR-405).

## Why

Today each process has its own `Heap`, its own copy of the prelude, and can't see
another process's `defn`s. We want the split — **share code, isolate data** —
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

As designed: reuse the top bit of the then-`u32` index as a region flag. As built
(`core/value.rs`): every handle type (`PairId`/`VecId`/`StrId`/`ClosureId`/`EnvId`/
`NativeId`, …) is a `u64` with the region in the top two bits (`REGION_SHIFT` 62), an
age bit at 61, a generation in bits 32–60 and the slab index in the low 32. A LOCAL
handle carries the collector's epoch in the generation bits; a RUNTIME handle carries
its code generation (0 or 1). Region `0b11` is reserved for the `EnvId::GLOBAL`
sentinel and can never be minted.

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
  stays until nothing holds it (ADR-091), a call already running the old closure
  finishes on it, while every new lookup — in any process — gets the new version. This is the
  live-reload semantics we want (Emacs has the same).

## Closures capture the global env *symbolically*

A subtlety found while building: a closure stores the env it was defined in, but
a **shared** top-level closure can't store a per-process `EnvId` — each process
has its own global env. So a closure defined at the global (parent-less) scope
captures the global **symbolically** (`Closure.env == None`); at call time
`bind_params` resolves it to *this process's* global env (the `Heap` knows its
own global). Closures that capture a *local* enclosing scope keep `Some(EnvId)`.
This is what lets one shared closure run correctly in any process. (Done as
step 2a — behaviour-preserving on its own.)

At this step the global env stayed per-process and mutable, outside the immutable
shared region. Step 4 below moved the bindings table into the runtime's shared
`RuntimeCode`: the global scope is now the sentinel `EnvId::GLOBAL`, not a frame.

## `env_get` across regions

A process's innermost scopes are *local* `EnvId`s; `env_get` dispatches per-frame by
region, and a chain bottoms out at `EnvId::GLOBAL`, which routes to `runtime.globals`.

## Interaction with processes

- `spawn` no longer reloads the prelude — the child clones the `Arc` to the shared
  code (O(1)) and starts with a fresh, empty data heap. Cheap.
- **Sending functions becomes easy:** a *top-level* function's code lives in the
  shared region and its env is the shared global, so its `ClosureId` is valid in
  *every* process — sending it is just sending the (shared) handle (shipped for a
  closure already in RUNTIME, `BROOD_NO_SHARE_FN` turns it off). Closures that
  capture locals still have those captured values copied (free-variable capture),
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
3. ✅ **`spawn` shares the prelude.** `Interp::new` no longer reloads the prelude
   (clones the `Arc` + seeds the global table), so spawning is cheap and the
   child can call any prelude/builtin via the shared region.
4. ✅ **The mutable shared RUNTIME region (this doc's payoff).** A third region,
   `RUNTIME` (a per-runtime `Arc<RuntimeCode>`), holds the code `def`'d at
   runtime plus the global bindings table. **All of a runtime's inner processes
   share that same `Arc`**, so a `def` reaches a running process on its next
   lookup. Details below.

### Scope: inner processes share; separate runtimes don't

The requirement that settled the direction: **a long-running spawned process
(e.g. a web server) must pick up a redefinition without being restarted** — but
**updating one runtime must not propagate to other (connected) runtimes/nodes.**
The reconciliation is a matter of *scope*:

- **Inner processes** — everything a runtime `spawn`s — **share that runtime's
  live code + global table.** They resolve globals against the shared table
  (late binding), so a `def` is visible to them on their next call. This is the
  cross-process hot reload (shared current code, every call re-dispatches through
  it — and since Brood is a Lisp-1 with late binding, *every* call already
  re-dispatches, no qualified remote call needed).
- **Separate runtimes (nodes)** each get their **own** `RuntimeCode`, so
  updating one never touches another.

### How it's built

- **Three regions, 2-bit handle tag** (`core/value.rs`): `LOCAL` (per-process data,
  `Vec` slabs), `PRELUDE` (immutable, `Arc<SharedCode>`, shared by all runtimes),
  `RUNTIME` (mutable, per-runtime, shared by inner processes).
- **`RuntimeCode`** (`core/heap/runtime_code.rs`): append-only code slabs (`boxcar::Vec`
  — lock-free reads return stable references that survive concurrent pushes, so process
  threads read closure bodies without locking while a `def` appends), two generations of
  them behind `ArcSwap` so a drained generation can be freed (ADR-091,
  `core/heap/gc_runtime.rs`), + a `RwLock<SymbolMap<Value>>` global bindings table (read
  on a global-lookup miss, per-process inline caches in front of it, written on `def` —
  the only mutation, ADR-026).
- **The global scope is a sentinel `EnvId::GLOBAL`**, not a frame; the env
  routines route it to `runtime.globals`. A local frame chain bottoms out there;
  a top-level closure captures it symbolically (`Closure.env == None`).
- **`def` promotes** the bound value's reachable code from `LOCAL` into the
  `RUNTIME` region (deep copy, append-only) before rebinding in the shared table.
  Append-only means a redefinition adds a *new* version while a call already
  running the old closure finishes on it — correct hot-reload semantics. Old code
  stays live while anything holds it; the collector reclaims it after that (ADR-091).
- **`spawn`** clones the parent's `Arc<RuntimeCode>` (shared code). A closure that
  captures locals is copied into the child's heap, sharing its arms and copying its
  captured values as a message would (`message::copy_closure_into`, ADR-405); anything
  else is `promote`d, a no-op for a top-level function.

### Captured environments cross the boundary too

A closure defined *inside a function call* (not at top level) closes over a local
scope. When such a closure is promoted (a `def`, or a `spawn` fallback), `promote` also
copies its captured environment chain into the `RUNTIME` region (`promote_env`) — without
this, a shared closure with `env = Some(LOCAL …)` would dereference a frame that
doesn't exist in the other process. (Promoted frames are frozen — sound now that
bindings are immutable: a `let`/`fn` binding never changes after creation, ADR-026.)


## Risks

- Largest change since the arena migration; region dispatch is pervasive but
  mechanical (step 1 is a behaviour-preserving no-op, which de-risks it).
- The reference-lifetime issue around shared reads is the real subtlety — handled
  by the immutable-vs-append-only split above; if the append-only store proves
  fiddly, the fallback is clone-on-shared-read (correct, slower) for the mutable
  region only.
- Lock/append synchronisation on the mutable region; the immutable region (the
  common case, prelude/builtins) stays lock-free.

## Decision

Approach **A** was chosen and built in the staged sub-steps above.
