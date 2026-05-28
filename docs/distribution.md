# Distribution: connecting two nodes

> Status: **slice 1 implemented.** Two Brood runtimes connect over TCP and
> message each other. Erlang-style distribution falls out of share-nothing +
> copy-on-send — *the network is just a longer copy* (ADR-013, ADR-034). The
> design intent lives in [`concurrency.md` §Distribution](concurrency.md); this
> doc is the as-built reference.

## What you can do

```lisp
;; --- node A ---------------------------------------------------------------
(node-start :a "127.0.0.1:9001" "secret")   ; name this runtime + listen
(register :echo (self))                       ; expose this process by name
(defn serve ()
  (receive
    ([:hi from]   (do (send from [:pong (self)]) (serve)))
    ([:ping from] (do (send from [:pong (self)]) (serve)))))
(serve)

;; --- node B ---------------------------------------------------------------
(node-start :b "127.0.0.1:9002" "secret")
(connect "a@127.0.0.1:9001")                  ; cookie-authenticated link
(send {:name :echo :node :a} [:hi (self)])    ; reach A's :echo by name
(def remote (receive ([:pong p] p)))          ; p is A's pid — a remote pid
(send remote [:ping (self)])                  ; now address it directly
(receive ([:pong _] :done))                   ; location-transparent reply
```

## Primitives

| Primitive | Meaning |
|---|---|
| `(node-start name "host:port" cookie)` | Name this runtime and listen for peers. Returns the node name. |
| `(connect "name@host:port")` | Dial + authenticate a peer. Returns the peer's node name. |
| `(register name pid)` | Bind a local name so peers can address this process. Returns the pid. |
| `(node-name)` | This runtime's node name (`:nonode` until `node-start`). |
| `(nodes)` | A list of currently connected peer node names. |
| `(monitor-node name)` | Deliver `[:nodedown name]` to the caller when the link to `name` goes down (clean close or heartbeat timeout). Persistent. |
| `(send target msg)` | `target` is a **pid** (local or remote) or a `{:name :node}` address. |
| `(pid? x)` | True if `x` is a process id. |

## The model

### Pids carry node identity
A pid is a first-class value, `Value::Pid { node, id }` (`Tag::Pid`), printing as
`#<pid node/id>`. `self` and `spawn` return one. A **local** pid carries this
node's name; a **remote** pid (received from a peer) carries the peer's. The same
value addresses a process whether it lives here or across a link — `send`
dispatches on the node part:

- node is us (or `:nonode`, i.e. minted before `node-start`) → deliver in-process
  (the existing `process::deliver`);
- node is a connected peer → encode a `Send` frame and forward over its link.

Sending to an unknown name, a disconnected node, or a dead pid is a **silent
no-op** (Erlang semantics).

### Bootstrapping vs. location transparency
You can't know a remote pid before someone tells you one. So a process is reached
two ways:

1. **By registered name** — `(register :echo (self))` on the peer, then
   `(send {:name :echo :node :a} msg)`. The bootstrap handle.
2. **By pid** — once a reply carries `(self)`, every later `send` targets that
   remote pid directly. This is the payoff: no special-casing "remote" at the call
   site.

### Transport (off the scheduler)
`node-start` binds a `TcpListener` and runs an acceptor thread. `connect` dials.
Both perform a handshake — exchange a `Hello { node, cookie }` and compare the
cookie (shared-secret equality; **not** real security yet, a placeholder for
auth/TLS). On success each connection gets two plain OS threads:

- a **writer** draining an `mpsc` channel onto the socket;
- a **reader** decoding inbound frames and handing messages to `process::deliver`.

These never touch the green-process coroutine scheduler — an inbound message lands
in a local mailbox exactly as an in-process `send` would.

### Wire codec
Hand-rolled and length-prefixed (`[u32 len][payload]`), reusing the `Message`
deep-copy that already crosses per-process heaps. The one cross-process subtlety:
**symbols travel by name** — a pid's `node`, keywords, and symbols are written as
their spelling and **re-interned on arrival**, because separate runtimes have
independent symbol interners. (In-process messages keep the interned id.)

## Scope & limitations (slice 1)

- **One node per OS process.** Node identity, the connection/name tables, and the
  interner are process-global, so a "node" *is* the OS process. Two nodes = two
  `brood` processes (typically over loopback). Testing reflects this: see the
  two-process end-to-end test in `crates/cli/tests/distribution.rs`.
- **Deferred** (later slices): remote `spawn` + code shipping (the closure-as-data
  path of ADR-033 is the missing piece — the wire codec rejects a `Closure`
  today), distributed process monitors/links, net-split handling, and the
  versioned/authenticated handshake (§3 below). Connection de-dup, node-down
  detection, and a generation-checked teardown path landed in
  **[slice 2](#slice-2--connection-lifecycle--liveness-built)**.

## Slice 2 — connection lifecycle + liveness (built)

Slice 1 was a working trusted-peer link; slice 2 makes it sturdy enough to leave
running. **De-dup + tie-break**, **node-down detection**, and the
**generation-checked teardown** they rest on are now implemented. The handshake-
v2 work (versioning + challenge–response) is **still deferred** (§3 below).

### 1. Duplicate / crossing connections (de-dup + tie-break) ✅

`connect` first checks `NODES` for an existing live link to the claimed name and
**reuses it** instead of dialing a redundant socket. For a genuine
simultaneous-connect race, `establish` resolves it under the `NODES` write lock
with a deterministic tie-break: **the link whose connector has the
lexicographically smaller node name wins** — comparing the *spelling*
(`value::symbol_name`), not the interned id, since ids differ per process but
the names match on both ends. The loser's socket is `shutdown` and never
registered; the winner replaces any prior entry under a new generation id, and
the displaced link tears down via the shared path (§4).

**Perf.** Cold path: the tie-break runs only at connection setup. The hot
`send` path is unchanged — still one uncontended `RwLock` read on `NODES` plus a
channel send. The lock-free `local_node()` atomic cache is preserved.

**Resources.** The losing side never spawns threads; the displaced link's reader
runs the single generation-checked teardown (§4), so no socket or thread leaks
across reconnects.

### 2. Node-down detection ✅

Two new wire frames — **`Ping`** and **`Pong`** (5 bytes each) — plus a single
shared **heartbeat thread** started lazily on the first link. Every
`HEARTBEAT_INTERVAL` (2 s) it snapshots `NODES` under the read lock and, for each
link, either declares it **down** (silent past `DOWN_AFTER` = 6 s) by
`shutdown`ing the socket, or sends a `Ping`. The peer's reader answers with a
`Pong`. Every inbound frame — `Send`, `Ping`, or `Pong` — refreshes the link's
`last_seen` atomic, so an idle-but-alive link stays healthy on its heartbeats and
a dead one is detected within a couple of intervals.

Down detection funnels into the same generation-checked teardown (§4), which
fires **`[:nodedown name]`** to every process that called
**`(monitor-node name)`** — the new Brood primitive (persistent, fires on each
down event; mirrors process `monitor` in spirit).

**Perf.** One thread total for all links, not per-link. Probes are idle-gated:
a `Ping` is sent only on the tick, never per-message; an active link's regular
traffic refreshes `last_seen` and the probes are pure no-ops on the receiver.
Snapshotting `NODES` once per tick avoids holding the lock across the actual
`shutdown`/`send`.

**Resources.** Detection is the *trigger* for the §4 teardown — a down node
frees its socket, both threads, and its table entry exactly once. Clean peer
exits (the test exercises this via `[:bye …]`) fire `nodedown` immediately via
reader EOF; heartbeat covers the hard-down (no FIN) case.

### 3. Handshake v2 (versioned + authenticated)

**Problem.** The current `Hello` is a plaintext node name + cookie compared
non-constant-time, with no protocol-version field, so a future wire change can't
be negotiated and a wrong-version peer fails opaquely.

**Approach.**
- Add a **protocol version** to `Hello`; reject or down-negotiate on mismatch with
  a clear error, so the codec can evolve compatibly.
- Replace the plaintext cookie with a **challenge–response** (each side sends a
  nonce; the other returns a MAC of it keyed by the shared cookie), so the secret
  never crosses the wire and a constant-time MAC compare removes the timing
  channel. Keep it pluggable for real TLS later.
- Carry the tie-break nonce from §1 in the same handshake (one round-trip).

**Perf.** Handshake is one-time per connection (cold path) — correctness over
speed. Keep the existing handshake **read timeout** so a stalled/﻿malicious peer
can't pin a thread during negotiation.

**Resources.** A failed/timed-out handshake must close the socket and not spawn
link threads (slice 1 already does this; preserve it).

### 4. Resource cleanup (cross-cutting — no leaks) ✅

Every link funnels through one teardown path. Each `Conn` carries a generation
id and a shared `Arc<TcpStream>` for `shutdown`. Any trigger — peer close, read
or write error, tie-break eviction (§1), heartbeat down (§2) — `shutdown`s the
socket; the reader unblocks and calls `drop_link(peer, id)`, which removes the
`NODES` entry **iff** the stored generation still matches (so an evicted link
can't clobber its replacement). Removal drops the `Conn`, which drops the
writer's channel sender and ends its `for … in rx`, and then fires `nodedown` to
watchers. Each exit frees: one socket, one reader thread, one writer thread —
exactly once.

**Still to do:** a churn test that opens/closes thousands of links under load
and asserts thread/fd counts return to baseline (the e2e covers the common
cases; the long-soak test is its own thing).

**Perf.** Teardown is cold. Hot-path lookups are unchanged — one uncontended
`NODES` read for a remote `send`, plus a lock-free atomic for `local_node()`.

### Sequencing
Built in the planned order: §4 (generation-checked teardown) → §1 (de-dup +
tie-break) → §2 (node-down + `monitor-node`). §3 (handshake v2: protocol-version
+ challenge–response) is still future; the existing cookie compare and version
omission are documented as not-yet-security.

## Where it lives
- `crates/lisp/src/dist.rs` — node state, transport threads, handshake, routing,
  wire codec.
- `crates/lisp/src/core/value.rs` — `Value::Pid` + `Tag::Pid`.
- `crates/lisp/src/process.rs` — `Message::Pid`, `send` dispatch, `pid_value`,
  `deliver` (the shared local-delivery tail).
- `crates/lisp/src/builtins.rs` — the primitives above.
- `std/prelude.blsp` — `pid?`.
