# Node connect ergonomics (design)

> Status: **implemented** (ADR-068, 2026-05-30). Makes connecting nodes cheap: a
> default shared secret, a name-addressed local transport, and `nest run --name`.
> This file is the design rationale; [`distribution.md`](../distribution.md) is the
> as-built reference. The wire protocol, HMAC handshake, pid routing,
> links/monitors, and ADR-067 cross-node supervision are all unchanged.

## Fit with the project goals

This is not a detour — it's the local leg of M4. The roadmap's destination is "a
modern, Emacs-like editor… runnable locally as a fast native app and **remotely
as a server** for other editor frontends," and M4 names the target directly:
*"the same runtime listens on a socket and serves the M3 protocol,"* *"remote
editor instances attach (**the Emacs `--daemon` / `emacsclient` model**),"* *"one
core, multiple attached frontends"* (`ROADMAP.md`).

Name-addressed Unix sockets *are* the emacsclient model for the local case:
`nest run --name foobar` ≈ `emacs --daemon=foobar`, `(node/connect "foobar")` ≈
`emacsclient -s foobar` — and Emacs uses exactly this `$XDG_RUNTIME_DIR`/`/tmp`
per-user socket convention. It also lands squarely on two guiding principles
(`ROADMAP.md`):

- **"The frontend is a protocol — local-native and remote are the same code path
  with different transports."** The `Stream { Tcp | Unix }` seam below is a
  literal instance: one handshake, one frame codec, two transports.
- **"Keep policy in Brood, mechanism in Rust."** Weighed explicitly in
  [Rust vs. Brood](#rust-vs-brood-adr-006) below.

**Immediate beneficiary:** the M3 observer's remote-attach already rides the dist
node link (`nest observe --connect name@host:port --cookie …`, `ROADMAP.md`).
With this it becomes `nest observe --connect foobar` with the cookie auto-resolved
— the first consumer, today, before the editor exists.

**Sequencing:** M4 distributed nodes (slices 1+2) are ✅ done and the milestones
are pursued vertical-slice style (ADR-045/046), so polishing connect ergonomics
now — with a present-day consumer — is consistent, not premature.

## Why

Before this, a node was hand-wired:

```lisp fragment
(node/start :server "127.0.0.1:9001" "demo-cookie")   ; pick a port, invent a secret
(node/connect "client@127.0.0.1:9002")                 ; know the peer's host:port
```

Three frictions, all incidental to the share-nothing model:

1. **The secret is invented per program.** There's no shared default, so every
   example hardcodes `"demo-cookie"`. The standard fix is a per-user cookie file
   in the home directory.
2. **You must pick an IP and a port** even when both nodes live on one machine —
   which is the common dev case *and* the editor case (a buffer/render server and
   the editor sharing a box). A TCP port is heavyweight addressing for "the brood
   node called `foobar` on this machine."
3. **Bringing a node up is ceremony.** `node/start` has to be called in-program
   with all three args before any app logic runs.

## The shape

```lisp fragment
;; same machine — name is the whole address, secret is implicit
(node/start :server)            ; listen on a per-user Unix socket; cookie auto-loaded
(proc/register :echo (self))

(node/start :client)
(node/connect "server")         ; dial the local node named "server"

;; across machines — explicit, exactly as today
(node/start :server "0.0.0.0:9001")
(node/connect "server@10.0.0.4:9001")
```

```bash
nest run --name foobar app.blsp     # brings the node up, then runs app.blsp
```

## 1. The default cookie file

- **Path:** `~/.config/brood/cookie` (XDG: `$XDG_CONFIG_HOME`, else `$HOME/.config`).
  One line of hex, mode `0600`.
- **Resolution at `node/start`** (when no explicit cookie is passed):
  `$BROOD_COOKIE` → the file → **generate + persist**. The generated secret is 32
  bytes from `getrandom` (the same CSPRNG the handshake nonce already uses,
  `dist/handshake.rs`), hex-encoded.
- **As built, this lives in Brood, not Rust.** The original plan was a Rust
  `dist::default_cookie()`; the implementation instead made cookie resolution
  *policy in Brood* — `(node/cookie)` in `std/node.blsp` (ADR-068) over the
  `os/env` / `rand/token` / `file/spit-private` primitives (the kernel carries only
  bytes). `file/spit-private` is the primitive that does the `0600` write a plain
  `file/spit` can't, so the file-mode mechanism stays in Rust while the resolution
  logic stays in Brood — a per-user cookie file, expressed in the language. There is **no** `dist::default_cookie()` symbol in the tree.

`$BROOD_COOKIE` still wins so CI / multi-tenant setups can override without
touching the file.

**As built, the connecting side resolves its cookie at `node/start`, not via a
handshake fallback.** The original plan was a handshake-time fallback (if
`NODE.cookie` is empty, use `default_cookie()`), so a bare `(node/connect "foobar")`
would work without `node/start`. The implementation took the simpler route:
`node/connect` *requires* a prior `node/start` (it errors `:nonode`
otherwise), and `node/start` is what calls `(node/cookie)` to populate
`NODE.cookie`. The handshake then reads that `NODE.cookie` as-is
(`dist/handshake.rs`) with **no empty-cookie fallback**. So the shared secret is
resolved once, in Brood, at start — not lazily in the Rust handshake.

## 2. Name-addressed Unix-socket transport

A local node binds a `UnixListener` at a path derived from its name:

```
$XDG_RUNTIME_DIR/brood/<name>.sock      # fallback: /tmp/brood-<uid>/<name>.sock
```

To reach it you only need the **name** — the path is derived on both ends. No
port, no IP, no allocation conflicts, and the `0700` directory is a free first
auth layer (same-user only).

**Dispatch reuses the existing `@` split** (`dist.rs`):

| `node/connect` argument | Transport |
|---|---|
| `"foobar"` (no `@`) | Unix socket, by name |
| `"foobar@host:port"` | TCP, exactly as today |

`$XDG_RUNTIME_DIR` can be unset (ssh without a session, cron) → fall back to
`/tmp/brood-<uid>/`. Names longer than the ~108-byte `sun_path` limit are hashed.

### The one real refactor: a transport seam

`establish`/`accept`/the reader+writer threads/`Conn.sock` were typed on
`TcpStream`. The seam, as built:

```rust
enum Stream { Tcp(TcpStream), Unix(UnixStream) }
```

implementing `Read`/`Write`/`shutdown`/`set_{read,write}_timeout` by forwarding,
and make `handshake` generic over `Read + Write` (it already only needs those).
`UnixStream` has the same `Arc`-deref + `shutdown(Shutdown::Both)` + timeout shape
as `TcpStream`, so the threads change only in their stream type. The handshake,
framing, tie-break, and heartbeat are byte-for-byte unchanged.

### Listen-side edge cases (all in Rust)

- Create the socket directory `0700` if absent.
- On `bind` → `AddrInUse`: probe-connect the existing socket. Refused ⇒ it's a
  stale file from a crashed node ⇒ unlink and rebind. Connects ⇒ a live node owns
  the name ⇒ error "name already in use by a live node."
- The cookie handshake still runs over Unix sockets (uniform code path), even
  though same-user filesystem perms already gate access — belt-and-suspenders, and
  it keeps one protocol.

## 3. `node/start` / `node/connect` surface

Multi-arity, **mostly additive** — the 3-arg TCP form and `name@host:port`
`node/connect` are unchanged, so the existing `distribution.rs` TCP suite keeps
passing:

| Form | Effect | Status |
|---|---|---|
| `(node/start name)` | local Unix node, cookie from default source | new |
| `(node/start name "host:port")` | TCP node, default cookie | new |
| `(node/start name "host:port" cookie)` | TCP node, explicit cookie | unchanged |
| `(node/connect "name")` | dial local peer by name (Unix) | new |
| `(node/connect "name@host:port")` | dial peer over TCP | unchanged |
| `(node/cookie)` | the resolved default cookie (reads/creates the file) | new |

`node/start` becomes arity `1..3`. Arity-1 = Unix only; an addr = TCP (preserving
the earlier behavior). **Dual listen** (a TCP node *also* reachable locally by name)
shipped as `node/also-listen` (ADR-074).

## 4. `nest run --name`

`--name NAME` on `nest run` (`std/tool/nest.blsp`): before running the user's file, nest
calls `(node/start NAME)`. Pure CLI→Brood glue; the app file becomes just `(proc/register :echo (self))` + logic.
`nest observe --connect foobar` (bare name) gains Unix support and the cookie-file
fallback for free.

`--name` is for files that *don't* self-start a node; a file that also calls
`node/start` would hit the existing double-start guard (`dist.rs`). Documented
as mutually exclusive rather than silently no-op'd.

## 5. Serving a `ui-run` app — `serve` / `nest attach` (done, ADR-090)

The payoff this design was the local leg of: a daemon **serves a `ui-run` app** and a
thin client paints it. `std/editor/serve.blsp` is the policy (`(require 'editor/serve)`),
purely over `node/start`/`node/connect`/`send`/`monitor` + the M3 display seam:

```
;; daemon — `nest run --name ed app.blsp`, whose main calls:
(editor/serve/serve (fn () {:n 0}) my-view my-update)   ; register the app under :ui, then park
;; client — another terminal / machine:
(editor/serve/attach "ed")   ; or "ed@host:port";  CLI: `nest attach ed`  ≈  emacsclient -s ed
```

The app's *unmodified* `(ui-run model view update display)` runs on the daemon; the
trick is the **display** — `remote-display` `:draw`s by `send`ing the frame over the
link and `:poll`s by `receive`ing the client's keys (the frame is plain Brood data, so
it just travels). `serve` spawns one **independent session per attaching client**, so
several frontends attach at once; `nest attach` is the thin `emacsclient`, connecting
*before* it takes the terminal so a bad spec/cookie is a clean error. The app-on-daemon
*push* complements the observer's *pull* remote-attach (§`nest observe --connect`).
The session's clipboard is the client's (ADR-539): an app calls
`client-clipboard-set` / `client-clipboard-ask` in its `update`, which reach the window's
machine over the link (`[:clipboard-set text]`, `[:clipboard-get session ref]` →
`[:clipboard ref text]`, which arrives at `update` as an event — answered nil by a timer when
the client says nothing, so a dead client never stalls the session).
A session can outlive its client (`serve`'s `{:resume-ms N}`, ADR-519: a named client's session
parks with its state and the next attach under that name takes it over), and a session
supervises its loop (ADR-593/605). `serve-shared` serves one shared model to every client,
all on one cursor. Deferred (ADR-011): per-client cursors in a shared session, per-client
viewports.

## Rust vs. Brood (ADR-006)

The plan put the smart args and the path/cookie conventions in Rust, because the Rust
`nest observe` had to reach them without a Brood image. As built the policy went the other
way: `node/start`, `node/connect` and `node/cookie` are Brood in `std/node.blsp` over thin
`%node-*` primitives, which became simple once `nest observe` itself moved to Brood. Sockets,
perms and the CSPRNG stay in Rust, the mechanism ADR-006 assigns there.

## Build sequence (as planned, 2026-05-30)

1. `Stream` enum + generic `handshake`/`establish` (the transport seam).
2. `socket_path()` + Unix listen/dial + stale-socket handling.
3. `default_cookie()` + `(node-cookie)`.
4. Multi-arity `node/start`/`connect` builtins + `@`-based dispatch.
5. `nest run --name` + `observe` cookie fallback.
6. Tests, examples, docs, ADR-068.

Only step 1 is a nontrivial refactor; the rest is additive.

## Tests

`crates/cli/tests/distribution.rs` spawns real `brood` children — mirrored for Unix:

- `two_unix_nodes_connect_and_message` — two children, `node/start` by name,
  `connect` by name, echo round-trip.
- `wrong_cookie_rejected_over_unix` — MAC mismatch closes the link.
- `cookie_file_autogen_and_reuse` — point `$HOME`/`$XDG_CONFIG_HOME` at a tempdir;
  assert the file appears `0600` and a second run reuses the same secret.

Cross-node behavior needs real processes, so coverage stays in `distribution.rs`
rather than the in-language suite.

## Open questions

- **Windows** — Unix sockets exist on modern Windows but the `$XDG_RUNTIME_DIR`
  path convention doesn't. The Unix-socket transport is `cfg(unix)`; TCP works everywhere.
