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
  today), distributed monitors/links, and net-split handling. Connection de-dup,
  node-down detection, a versioned/authenticated handshake, and the
  resource-cleanup discipline they all depend on are planned in
  **[Planned hardening — slice 2](#planned-hardening--slice-2-not-yet-built)**.

## Planned hardening — slice 2 (not yet built)

Slice 1 is a working, trusted-peer link. Before it can carry real traffic it
needs connection lifecycle correctness, liveness detection, and a sturdier
handshake. These are **planned, not implemented** — each notes its performance
and resource-cleanup considerations so we don't regress the hot path or leak
threads/sockets when we do build it.

### 1. Duplicate / crossing connections (de-dup + tie-break)

**Problem.** Today connecting to a peer twice — or A and B dialing each other
simultaneously — inserts a second `Conn` under the same node key, clobbering the
first. The replaced writer exits when its channel drops, but its **reader thread
and socket linger** until the peer closes. Two live links can also race messages
out of order. Erlang hit this exactly and solved it in the handshake.

**Approach.**
- Before dialing, check `NODES` for an existing live link to that node name and
  reuse it (don't open a second).
- For the genuine simultaneous-connect race, resolve it **in the handshake** with
  a deterministic tie-break: the node with the lexicographically smaller name (or
  a comparison of a per-connection nonce) wins; the loser closes its socket. Both
  ends apply the same rule, so exactly one link survives.
- On accept, if a link to that peer already exists, apply the same tie-break
  rather than blindly inserting.

**Perf.** Pure connection-setup cost (cold path) — must not touch the per-`send`
routing path. The `NODES` read on `send` stays a single uncontended `RwLock` read
(or move to an `arc-swap`/`RCU` snapshot if it ever shows up in profiles).

**Resources.** The losing side must fully tear down — close the socket *and* stop
its reader/writer threads (see §4 for the shared shutdown handle), with no
half-open leftover.

### 2. Node-down detection

**Problem.** A peer that vanishes without a TCP FIN (cable pull, power loss, kill
-9) leaves our reader blocked on `read` forever, a stale entry in `(nodes)`, and
`send`s silently dropping into a dead writer. There's no signal to the language.

**Approach.**
- **TCP keepalive** on each link socket as the cheap backstop (`SO_KEEPALIVE` +
  tuned idle/interval) so the OS eventually errors a dead reader.
- A lightweight **application heartbeat**: a `Ping`/`Pong` frame on an idle timer;
  if N intervals pass with no traffic, declare the node down. This catches
  half-open and "process alive but wedged" faster than TCP alone.
- On node-down: remove the `NODES` entry, tear down both threads (§4), and
  **surface it to Brood** — deliver `[:nodedown <name>]` to processes that asked
  to watch the node (a `(monitor-node name)` primitive, mirroring process
  `monitor`). This is also what lets distributed monitors fire a `:noconnection`
  DOWN for pids on a downed node.

**Perf.** Heartbeats are per-link and idle-gated (no traffic ⇒ a tiny frame every
few seconds), never per-message. One shared timer thread for all links, not a
thread per link. The idle timer must not wake a sleeping link needlessly.

**Resources.** Detection is the *trigger* for cleanup — wire it to the single
teardown path so a down node frees its socket, both threads, and its table entry.

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

### 4. Resource cleanup (cross-cutting — do not leak)

A connection owns two OS threads + a socket; **every** exit path (peer close,
read/write error, handshake failure, tie-break loss, node-down) must free all
three exactly once.

**Done so far:** the reader and writer tear *each other* down, so neither runtime
exit path half-leaks — reader-dies removes the `NODES` entry (dropping the
writer's channel), and writer-dies `shutdown`s the socket to unblock the reader,
which then removes the entry. Still to do: a shared shutdown signal so the
*deliberate* drops (tie-break loss §1, node-down §2) funnel through the same
idempotent teardown, plus the churn test below.

**Approach.** Give each link a shared shutdown signal (an `AtomicBool` +
`shutdown(Shutdown::Both)` on the socket, or drop the writer's channel and
`shutdown` to unblock the reader). Any path that decides to drop a link sets it,
which unblocks the reader's `read`, drains/stops the writer, and removes the
`NODES` entry — once, idempotently. Add a test that opens/closes many links and
asserts thread and fd counts return to baseline (no leak under churn).

**Perf.** Teardown is cold; the only hot-path constraint is that the liveness
checks and the `NODES` lookup on `send` stay cheap (atomic / single short lock).

### Sequencing
§4 (a single idempotent teardown path) underpins the rest, so build it first;
then §1 (de-dup, which needs teardown for the losing link), then §2 (node-down,
which triggers teardown), then §3 (handshake v2, which carries §1's tie-break).
Each lands behind `cargo test` + the two-process integration test, extended with
churn/kill scenarios.

## Where it lives
- `crates/lisp/src/dist.rs` — node state, transport threads, handshake, routing,
  wire codec.
- `crates/lisp/src/core/value.rs` — `Value::Pid` + `Tag::Pid`.
- `crates/lisp/src/process.rs` — `Message::Pid`, `send` dispatch, `pid_value`,
  `deliver` (the shared local-delivery tail).
- `crates/lisp/src/builtins.rs` — the primitives above.
- `std/prelude.blsp` — `pid?`.
