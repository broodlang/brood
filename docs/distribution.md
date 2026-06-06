# Distribution: connecting two nodes

> Status: **slices 1 + 2 implemented; connect ergonomics per ADR-068.** Two Brood
> runtimes connect and message each other — on one machine over a Unix-domain
> socket addressed by name, across machines over TCP. Erlang-style distribution
> falls out of share-nothing + copy-on-send — *the network is just a longer copy*
> (ADR-013, ADR-034). The design intent lives in
> [`concurrency.md` §Distribution](concurrency.md); the ergonomics rationale in
> [`node-connect.md`](node-connect.md); this doc is the as-built reference.

## What you can do

```lisp
;; --- node A (same machine) ------------------------------------------------
(node-start :a)                               ; local Unix-socket node, by name
(register :echo (self))                       ; expose this process by name
(defn serve ()
  (receive
    ([:hi from]   (do (send from [:pong (self)]) (serve)))
    ([:ping from] (do (send from [:pong (self)]) (serve)))))
(serve)

;; --- node B (same machine) ------------------------------------------------
(node-start :b)
(connect "a")                                 ; dial A by name — no port, no IP
(send {:name :echo :node :a} [:hi (self)])    ; reach A's :echo by name
(def remote (receive ([:pong p] p)))          ; p is A's pid — a remote pid
(send remote [:ping (self)])                  ; now address it directly
(receive ([:pong _] :done))                   ; location-transparent reply

;; --- across machines: TCP, explicit host:port -----------------------------
(node-start :a "0.0.0.0:9001")                ; listen over TCP
(connect "a@10.0.0.4:9001")                   ; dial a remote peer
```

The shared cookie authenticating links lives in `~/.config/brood/cookie`
(auto-generated `0600` on first use; `$BROOD_COOKIE` overrides). All your nodes on
a machine share it — no secret to pass. `nest run --name foo app.blsp` brings a
local node up before running `app.blsp` (the Emacs `--daemon` model).

## Node names are `name@host` (ADR-073)

A node's identity is `name@host`, Erlang-style — globally unique, carried in every
pid (`#<pid a@whkbus/3>`). `node-start` qualifies a bare name: a **local** node
takes this machine's short `(hostname)` (`:a@whkbus`); a **TCP** node takes its
listen address's host (`:a@127.0.0.1`), so peers and `ensure-link` derive the same
name. Pass an explicit `:name@host` for a long/FQDN name. **`connect` returns the
peer's authoritative `name@host`** — address peers with that value (a `let`/`def`
binding, or `(nodes)`), not a bare literal.

## Primitives

| Primitive | Meaning |
|---|---|
| `(node-start name)` | Start a **local** node (Unix-domain socket, no port). Returns its `name@host`. |
| `(node-start name "host:port")` | Start a node listening over **TCP** for remote peers. |
| `(node-start name "host:port" cookie)` | …with an explicit cookie (the default is `(node-cookie)`). |
| `(node-also-listen)` / `(node-also-listen "host:port")` | **Dual-listen** (ADR-074): add another front door to this node — the local Unix socket, or a TCP endpoint — sharing its identity + cookie. |
| `(connect "name")` | Dial a local peer by name (Unix socket). Returns the peer's `name@host`. |
| `(connect "name@host:port")` | Dial a remote peer over TCP. Returns its `name@host`. |
| `(remote-spawn node expr)` | Run `expr` in a fresh process on `node` (fire-and-forget, returns nil). |
| `(remote-spawn-sync node expr)` | Like `remote-spawn` but returns the child's (node-tagged) pid — `monitor`/`link`-able. |
| `(node-cookie)` | The shared link secret: `$BROOD_COOKIE` → `~/.config/brood/cookie` → freshly minted. |
| `(hostname)` | This machine's short hostname (used to qualify a local node name). |
| `(register name pid)` | Bind a local name so peers can address this process. Returns the pid. |
| `(node-name)` | This runtime's node name (`:nonode` until `node-start`). |
| `(nodes)` | A list of currently connected peer node names. |
| `(monitor-node name)` | Deliver `[:nodedown name]` to the caller when the link to `name` goes down (clean close or heartbeat timeout). Persistent. |
| `(disconnect name)` | Tear the link to `name` down now, without exiting this process (Erlang's `disconnect_node`). Fires `[:nodedown name]` on both sides, prunes `(nodes)`. Returns `true` if a link existed, else `false`. |
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
`node-start` binds a listener — a `UnixListener` for a local name or a
`TcpListener` for `host:port` — and runs an acceptor thread; `connect` dials the
matching carrier. A single `Stream { Tcp | Unix }` enum (ADR-068) carries the
link, so everything below is transport-agnostic. Both ends perform the
authenticated handshake (ADR-034 v2; wire **v5**, ADR-089): a 4-byte
magic+version prefix (`b"BRD\x05"`), then a `Hello { node, nonce, eph_pub, addr }`
exchange (each side a fresh 32-byte nonce, a fresh **ephemeral X25519 pubkey**,
and the address peers should dial it at), then an `Auth { mac }` exchange where
each side sends `HMAC-SHA256(cookie, peer_nonce || peer_eph_pub || peer_name ||
my_name || my_addr || my_eph_pub)`. The cookie is **never on the wire** — it's an
HMAC key, so an eavesdropper can't replay either it or a captured `Auth`; folding
both ephemeral pubkeys into the MAC authenticates the DH (a man-in-the-middle can't
swap a key without the cookie). A mismatch on the magic, the MAC, or either Hello
aborts before the link enters `NODES`. On success each connection gets two plain OS
threads:

- a **writer** draining an `mpsc` channel onto the socket;
- a **reader** decoding inbound frames and handing messages to `process::deliver`.

These never touch the green-process coroutine scheduler — an inbound message lands
in a local mailbox exactly as an in-process `send` would.

### Channel encryption (ADR-089)
The handshake authenticates the peer; the **session encrypts the link**. After the
MAC verifies, both ends derive a shared secret from the exchanged ephemeral X25519
keys (forward secrecy — recorded traffic stays secret even if the cookie later
leaks), run it through HKDF-SHA256 to two **directional keys**, and from then on
**every steady-state frame is sealed with ChaCha20-Poly1305** (the Poly1305 tag is
a per-frame MAC). The writer owns the send key + a monotonic counter nonce, the
reader the receive key + counter — so the per-direction AEAD maps cleanly onto the
reader/writer thread split (a single TLS connection couldn't be driven from both,
which is *why* it's a Noise-style session rather than TLS). A forged, tampered,
replayed, or reordered frame fails to open and tears the link down — so a
post-handshake `Send`-carrying-a-closure injection (→ RCE) is impossible without the
cookie. Handshake metadata (names, nonces, pubkeys) stays plaintext (none secret);
only the steady-state frames — including shipped closure source — are encrypted.
Uniform over TCP **and** Unix. **A TCP node is now safe on an untrusted network**
(closure-shipping between *trusting* nodes is still RCE-by-design — the Erlang model;
a mutually-distrusting/multi-tenant boundary is a separate future ADR).

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
- **Built since slice 1:**
  - **Closure-as-data path** (ADR-033) — `(send target fn)` and the `[:run f x
    …]` pattern work cross-node. The wire codec's `M_CLOSURE` encodes every
    `ClosureMsg` field (name, params, optionals + default *forms*, rest, body
    forms, doc, captured free locals); the receiver's `closure_from_message`
    rebuilds against its own prelude, so free globals re-resolve there.
  - **`(remote-spawn node expr)`** macro (`std/prelude.blsp`) — the surface
    convenience over the `[:run …]` pattern; ships the closure to a
    `:remote-spawn` server on `node` (lazily started via `(start-remote-spawn)`).
    See `remote_spawn_runs_a_thunk_on_a_peer`.
  - **Source positions across the wire** — `Message::List` carries an optional
    trailing `Pos`; on rebuild the receiver's `set_form_pos` re-stamps it, so
    `(form-pos …)` and the eval loop's `or_form_pos` work on remote-shipped
    code. See `source_positions_survive_a_cross_node_send`.
  - **Distributed pid monitors** — `(monitor remote-pid)` ships a
    `Frame::Monitor` to the peer, which routes through the same shared
    `process::add_monitor` core the local monitor uses (one `Watcher` enum,
    one `MONITORS` table). On the watched process's death the peer fires
    `[:down …]` as an ordinary `send` to the remote watcher. Net-split fires
    `[:down mref pid :noconnection]` via the sender-side `PENDING_REMOTE`
    table and `handle_node_down`. See `cross_node_pid_monitor_fires_down` and
    `remote_monitor_fires_noconnection_on_node_down`.
  - **Distributed links** (ADR-067) — the symmetric cousin of monitors.
    `(link remote-pid)` ships a `Frame::Link`; each node keeps its half in
    `links::REMOTE_LINKS` (`local_pid → (node, remote_pid)`). A linked process's
    death ships a `Frame::Exit { link: true }` routed through the trap-or-propagate
    path (a trapping peer gets `[:EXIT remote-pid reason]`); `(exit remote-pid
    reason)` ships `Frame::Exit { link: false }` → `scheduler::exit`. Net-split
    fires `:noconnection` to local peers via `links::handle_node_down` (wired into
    `fire_nodedown` beside the monitor path). This is what makes cross-node
    supervision work (`brood-supervisor/src/proc/supervisor.blsp`). See the `remote_link_death_*`,
    `remote_exit_kills_*`, and `supervisor_restarts_a_remote_child` tests.
  - **Auto-reconnect** — `(ensure-link "name@host:port")` (Brood policy in
    `std/prelude.blsp`) maintains a peer link across restarts: synchronous
    initial `connect`, then a small supervisor that `monitor-node`s the peer
    and retries `connect` with a 200ms backoff on every `[:nodedown …]` until
    success. See `ensure_link_reconnects_across_a_node_restart`.
  - **Handshake v2 + encrypted session** (ADR-034 v2 + ADR-089) — magic+version
    prefix, nonce + ephemeral-pubkey `Hello`s, HMAC-SHA256 `Auth` (cookie never on
    the wire), then a forward-secret ChaCha20-Poly1305 channel (see *Channel
    encryption* above). See `non_brood_peer_is_rejected_at_magic_prefix`,
    `mismatched_cookie_is_rejected`, the `dist::handshake::tests` (MAC symmetry +
    key agreement) and `dist::session::tests` (seal/open, tamper/replay rejection).
- **Still deferred** (later): standards TLS *on the wire* as a third transport —
  open only if some external, non-Brood client must ever speak the node protocol
  (none does; brood-to-brood links are already encrypted via ADR-089).

## Slice 2 — connection lifecycle + liveness (built)

Slice 1 was a working trusted-peer link; slice 2 makes it sturdy enough to leave
running. **De-dup + tie-break**, **node-down detection**, and the
**generation-checked teardown** they rest on are now implemented. Handshake v2
(versioning + HMAC challenge–response) landed too — see §3 below.

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

**Deliberate teardown — `(disconnect name)`.** The three triggers above are all
*involuntary* (a peer left, or the link died). `disconnect` is the *voluntary*
one — Erlang's `disconnect_node/1`. It `shutdown`s the peer's socket (so the
peer's reader hits EOF and fires its own node-down) and runs the same §4
`drop_link` on our side, firing `[:nodedown name]` to our monitors and pruning
`(nodes)` — all without exiting the process. It is the clean way to **leave a
node/cluster while staying alive** (a server dropping one client; a node bowing
out of a multi-node group), so an application no longer needs an ad-hoc
`[:bye]`-broadcast convention to get prompt pruning. Our own reader also hits
EOF and calls `drop_link` again, but the generation-id guard (§4) makes that a
no-op — `nodedown` fires exactly once. Returns `true` if a link existed.

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

## Cluster mesh — connecting transitively (ADR-088)

Nodes form a **full mesh** by default, Erlang-style: connecting to *one* member
of a cluster transitively connects you to **every** node it knows. So with nodes
A, B, C where A connects to hub B and C connects to hub B, A and C end up linked
to each other too — you don't have to dial every peer by hand.

**How it works.**
- The handshake's `Hello` now carries each node's **advertised dial address**
  (`unix:PATH` / `tcp:HOST:PORT`) — how a *third* node should reach it. It's
  folded into the auth MAC, so a man-in-the-middle can't rewrite it without the
  cookie. Each link stores its peer's address (`Conn.addr`).
- When a **new** peer joins, the node broadcasts a `Frame::Peers` gossip frame —
  its table of `(node-name, dial-addr)` pairs — to every connected peer. The new
  peer learns the incumbents; the incumbents learn the newcomer.
- On receiving gossip, a node **dials any peer it isn't already connected to**
  (each on a short-lived thread; a `PENDING_DIALS` set dedupes concurrent gossip
  naming the same peer). Each new link re-gossips, so the mesh closes
  transitively and then goes quiet (a reconnect/duplicate doesn't re-broadcast).
- Simultaneous cross-dials (A dials C while C dials A) collapse to one link via
  the existing **connector tie-break** (§1). `(nodes)` reflects the full mesh.

**Address chosen to advertise:** the first TCP listener if any (reachable
locally over loopback *and* remotely), else the local Unix socket. A dual-listen
node therefore advertises its TCP endpoint.

**Opt out:** `BROOD_NO_MESH=1` keeps links strictly point-to-point — you connect
to exactly the nodes you dial, with no transitive discovery.

**Limitations (v1, ADR-011 — additive when a consumer needs more).**
- *No auto-reconnect / re-heal.* The mesh forms on join; a transient link drop
  isn't re-dialed on its own (consistent with Erlang). Use `ensure-link` for a
  persistently-maintained link.
- *Address must be routable from the discoverer.* A node advertises its own
  listen address; meshing assumes peers can route to it (the same assumption
  `name@host` already makes). A unix-only node gossiped to a different machine
  can't be reached — use TCP nodes for cross-machine clusters. A wrong/
  unreachable advertised address fails the dial harmlessly (and the cookie gate
  means only same-cluster nodes ever link).
- *Trust:* gossip comes from an already-authenticated peer (it holds the cookie),
  so meshing crosses no new trust boundary — it dials with our cookie, and only
  same-cookie nodes link. Do not expose a TCP node on an untrusted network until
  channel TLS lands (ADR-081), mesh or not.

## Where it lives
- `crates/lisp/src/dist.rs` — node state, transport threads, handshake, routing,
  wire codec, **cluster-mesh gossip** (`broadcast_peer_table`/`mesh_consider`).
- `crates/lisp/src/core/value.rs` — `Value::Pid` + `Tag::Pid`.
- `crates/lisp/src/process.rs` — `Message::Pid`, `send` dispatch, `pid_value`,
  `deliver` (the shared local-delivery tail).
- `crates/lisp/src/builtins.rs` — the primitives above.
- `std/prelude.blsp` — `pid?`.
