# Findings: `def`-promoting (and sending) a closure that captures a closure overflows

**Status:** open bug, with workarounds in place. Found 2026-05-30 while building
the HTTP server (`std/http.blsp`). Two sites, one root cause.

## Symptom

```lisp
(def g (let (h (fn () 1)) (fn () (h))))   ; stack overflow, aborts the process
```

A minimal reproduction: `def` a closure that **captures another closure**. Real-
world trigger: `(def app (router {"/" home}))` — a router is a closure capturing a
map whose values are handler closures. Crashes at the `def`, before any request.

The twin: **sending such a closure to a spawned process** overflows the same way
(`closure_to_message`), so a spawn-per-connection server (`(spawn (handle c
handler))` capturing the `handler` closure) also crashes.

## Root cause

A closure's captured `env` is the live frame chain at its creation. In
`(let (h (fn () 1)) (fn () (h)))` the `let` frame `F` binds `h` to a closure, and
the outer `(fn () (h))` captures `F`. So the in-heap graph is **cyclic**:
`F.vars[h] = <closure h>` and (because closures capture their whole defining
frame) `h.env = F`. The live heap holds such cycles routinely; that's fine.

The problem is **promotion to the shared RUNTIME region** (`Heap::promote` →
`promote_closure` → `promote_env`, `core/heap.rs`). It deep-copies the graph with
**no forwarding table and no cycle break**:

```
promote_env(F) → promote(h) → promote_closure(h) → promote_env(h.env = F) → …  ∞
```

Contrast the GC's `flush_closure`/`flush_env`, which copy the *same cyclic* live
heap without overflowing. They break cycles by **reserve → record → recurse →
back-patch**:

```rust
let new_idx = new.closures.len();
new.closures.push(Closure::default());     // reserve a slot
fwd.closures.insert(key, new_idx);         // record old→new BEFORE recursing
… recurse (a back-reference now hits fwd and returns new_idx) …
new.closures[new_idx] = Closure { … };     // back-patch the reserved slot
```

That works because `flush`'s target (`Slabs`) is a mutable `Vec` — it supports
`new.closures[new_idx] = …`. **`promote`'s target is the append-only `boxcar`
RUNTIME region** (`runtime.code.closures`/`envs`): `push` only, no index
assignment, so the reserve-then-back-patch trick isn't available, and `promote`
has no forwarding table at all.

So the asymmetry is exactly: *mutable copy target → cycles handled; append-only
copy target → cycles overflow.*

## Why it's not just depth

The repro is **two** levels deep. It's unbounded recursion (a cycle), not a deep
structure, so raising a stack limit wouldn't help — and today the overflow is an
**uncatchable abort/segfault**, not a clean error.

## Workarounds in place (so the HTTP stack ships)

- **Build routers in a fn, never `def` them.** `std/http.blsp`'s `serve` takes the
  handler as an argument; `examples/webserver.blsp` and the `http-server` template
  do `(serve port (router {…}))` / `(defn app () (router {…}))` — the router stays
  a LOCAL value, never promoted.
- **`serve` handles connections inline** (one at a time, selective `receive`)
  rather than `(spawn (handle c handler))`, to avoid shipping the handler closure
  (the `closure_to_message` twin).

Both are noted in `std/http.blsp` and ADR-062. They cost real capability:
top-level handler tables and spawn-per-connection concurrency.

## Fix options (a GC-core decision)

1. **Back-patchable RUNTIME frames + a forwarding table in `promote`.** Mirror
   `flush`: give `promote_*` a forwarding map and the reserve→record→back-patch
   shape. Needs the RUNTIME closure/env store to support back-patching — either
   interior mutability (`boxcar<Mutex<…>>` / a cell), or a **two-pass** build:
   resolve the cycle in a temporary `Vec` with temp indices, then append the
   finished objects to `boxcar` and offset their cross-references by the base.
   *Makes the pattern work.* Most faithful to the language; most work.

2. **Minimize closure capture at creation.** Capture only the *referenced* free
   locals (what `closure_to_message` already computes for shipping) instead of the
   whole frame chain. Then `(fn () 1)` captures nothing and the cycle never forms,
   so both `promote` and `closure_to_message` get acyclic graphs for free. Changes
   the eval hot path (every closure build), with perf/correctness implications;
   also independently shrinks promoted/shipped closures.

3. **Detect-and-error (mitigation, not a fix).** A read-only cyclic-capture walk
   before promotion (or a depth cap) that raises a clean, catchable
   `LispError` ("cannot `def`/send a closure with cyclic capture — build it in a
   `fn` or inline") instead of aborting. Cheap and safe; turns a segfault into a
   message, but still blocks the pattern.

**Recommendation:** (1) is the real fix and the two-pass variant keeps RUNTIME
append-only. (3) is a good cheap stopgap to ship alongside so the failure is never
a crash. (2) is attractive if a capture-minimization pass is wanted for perf
reasons anyway.

## See also

ADR-062 (TCP/HTTP — where the workarounds live), ADR-059 (mailbox seam), the GC
`flush_closure`/`flush_env` forwarding pattern in `core/heap.rs`.
