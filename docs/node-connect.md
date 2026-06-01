# Node connect ergonomics (design)

> Status: **implemented** (ADR-068, 2026-05-30). Makes connecting nodes cheap: a
> default shared secret, a name-addressed local transport, and `nest run --name`.
> This file is the design rationale; [`distribution.md`](distribution.md) is the
> as-built reference. The wire protocol, HMAC handshake, pid routing,
> links/monitors, and ADR-067 cross-node supervision are all unchanged.

## Fit with the project goals

This is not a detour — it's the local leg of M4. The roadmap's destination is "a
modern, Emacs-like editor… runnable locally as a fast native app and **remotely
as a server** for other editor frontends," and M4 names the target directly:
*"the same runtime listens on a socket and serves the M3 protocol,"* *"remote
editor instances attach (**the Emacs `--daemon` / `emacsclient` model**),"* *"one
core, multiple attached frontends"* (`roadmap.md:441`).

Name-addressed Unix sockets *are* the emacsclient model for the local case:
`nest run --name foobar` ≈ `emacs --daemon=foobar`, `(connect "foobar")` ≈
`emacsclient -s foobar` — and Emacs uses exactly this `$XDG_RUNTIME_DIR`/`/tmp`
per-user socket convention. It also lands squarely on two guiding principles
(`roadmap.md:453`):

- **"The frontend is a protocol — local-native and remote are the same code path
  with different transports."** The `Stream { Tcp | Unix }` seam below is a
  literal instance: one handshake, one frame codec, two transports.
- **"Keep policy in Brood, mechanism in Rust."** Weighed explicitly in
  [Rust vs. Brood](#rust-vs-brood-adr-006) below.

**Immediate beneficiary:** the M3 observer's remote-attach already rides the dist
node link (`nest observe --connect name@host:port --cookie …`, `roadmap.md:366`).
With this it becomes `nest observe --connect foobar` with the cookie auto-resolved
— the first consumer, today, before the editor exists.

**Sequencing:** M4 distributed nodes (slices 1+2) are ✅ done and the milestones
are pursued vertical-slice style (ADR-045/046), so polishing connect ergonomics
now — with a present-day consumer — is consistent, not premature.

## Why

Today a node is hand-wired:

```lisp
(node-start :server "127.0.0.1:9001" "demo-cookie")   ; pick a port, invent a secret
(connect "client@127.0.0.1:9002")                      ; know the peer's host:port
```

Three frictions, all incidental to the share-nothing model:

1. **The secret is invented per program.** There's no shared default, so every
   example hardcodes `"demo-cookie"`. Erlang solved this in 1998 with
   `~/.erlang.cookie`.
2. **You must pick an IP and a port** even when both nodes live on one machine —
   which is the common dev case *and* the editor case (a buffer/render server and
   the editor sharing a box). A TCP port is heavyweight addressing for "the brood
   node called `foobar` on this machine."
3. **Bringing a node up is ceremony.** `node-start` has to be called in-program
   with all three args before any app logic runs.

## The shape

```lisp
;; same machine — name is the whole address, secret is implicit
(node-start :server)            ; listen on a per-user Unix socket; cookie auto-loaded
(register :echo (self))

(node-start :client)
(connect "server")              ; dial the local node named "server"

;; across machines — explicit, exactly as today
(node-start :server "0.0.0.0:9001")
(connect "server@10.0.0.4:9001")
```

```bash
nest run --name foobar app.blsp     # brings the node up, then runs app.blsp
```

## 1. The default cookie file

- **Path:** `~/.config/brood/cookie` (XDG: `$XDG_CONFIG_HOME`, else `$HOME/.config`).
  One line of hex, mode `0600`.
- **Resolution at `node-start`** (when no explicit cookie is passed):
  `$BROOD_COOKIE` → the file → **generate + persist**. The generated secret is 32
  bytes from `getrandom` (the same CSPRNG the handshake nonce already uses,
  `dist/handshake.rs:174`), hex-encoded.
- **Lives in Rust** as `dist::default_cookie() -> io::Result<String>`, exposed to
  Brood as `(node-cookie)`. It must be Rust: the `nest observe --connect` path
  resolves a cookie without a Brood image (`nest/src/main.rs:667`), and a `0600`
  write + CSPRNG are OS mechanism, not language policy. This mirrors Erlang
  reading `~/.erlang.cookie` inside the runtime, not in userland. (`spit` can't
  set a file mode — `builtins.rs:846` — which is the concrete reason this is a
  primitive.)

`$BROOD_COOKIE` still wins so CI / multi-tenant setups can override without
touching the file.

**The fallback must cover the connecting side too, not just `node-start`.** The
handshake reads the cookie from the global `NODE` (`dist/handshake.rs:58`). A
runtime that calls `(connect "foobar")` without first calling `node-start` has an
empty `NODE.cookie` and would fail auth against a default-cookie peer. So the
resolution belongs at the handshake: **whenever `NODE.cookie` is empty, fall back
to `default_cookie()`** — which makes "just connect, no node-start" work out of
the box (the common client case), with the same secret both ends already share.

## 2. Name-addressed Unix-socket transport

A local node binds a `UnixListener` at a path derived from its name:

```
$XDG_RUNTIME_DIR/brood/<name>.sock      # fallback: /tmp/brood-<uid>/<name>.sock
```

To reach it you only need the **name** — the path is derived on both ends. No
port, no IP, no allocation conflicts, and the `0700` directory is a free first
auth layer (same-user only).

**Dispatch reuses the existing `@` split** (`dist.rs:536`):

| `connect` argument | Transport |
|---|---|
| `"foobar"` (no `@`) | Unix socket, by name |
| `"foobar@host:port"` | TCP, exactly as today |

`$XDG_RUNTIME_DIR` can be unset (ssh without a session, cron) → fall back to
`/tmp/brood-<uid>/`. Names longer than the ~108-byte `sun_path` limit are hashed.

### The one real refactor: a transport seam

`establish`/`accept`/the reader+writer threads/`Conn.sock` are currently typed on
`TcpStream` (`dist.rs:590`, `:646`, `:657`). Introduce:

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

## 3. `node-start` / `connect` surface

Multi-arity, **mostly additive** — the 3-arg TCP form and `name@host:port`
`connect` are unchanged, so the existing `distribution.rs` TCP suite keeps
passing:

| Form | Effect | Status |
|---|---|---|
| `(node-start name)` | local Unix node, cookie from default source | new |
| `(node-start name "host:port")` | TCP node, default cookie | new |
| `(node-start name "host:port" cookie)` | TCP node, explicit cookie | unchanged |
| `(connect "name")` | dial local peer by name (Unix) | new |
| `(connect "name@host:port")` | dial peer over TCP | unchanged |
| `(node-cookie)` | the resolved default cookie (reads/creates the file) | new |

`node-start` becomes arity `1..3`. Arity-1 = Unix only; an addr = TCP (preserving
today's behavior). **Dual listen** (a TCP node *also* reachable locally by name)
is deferred per ADR-011 until the editor needs it.

## 4. `nest run --name`

Add `--name NAME` to the `Run` command (`nest/src/main.rs:86`). Before running the
user's file, nest evals `(node-start 'NAME)`. Pure CLI→Brood glue — no new policy
in Rust; the app file becomes just `(register :echo (self))` + logic.
`nest observe --connect foobar` (bare name) gains Unix support and the cookie-file
fallback for free.

`--name` is for files that *don't* self-start a node; a file that also calls
`node-start` would hit the existing double-start guard (`dist.rs:482`). Documented
as mutually exclusive rather than silently no-op'd.

## 5. Serving a `ui-run` app — `serve` / `nest attach` (done, ADR-090)

The payoff this design was the local leg of: a daemon **serves a `ui-run` app** and a
thin client paints it. `std/editor/serve.blsp` is the policy (`(require 'editor/serve)`),
purely over `node-start`/`connect`/`send`/`monitor` + the M3 display seam:

```
;; daemon — `nest run --name ed app.blsp`, whose main calls:
(serve (fn () {:n 0}) my-view my-update)   ; register the app under :ui, then park
;; client — another terminal / machine:
(attach "ed")          ; or "ed@host:port";  CLI: `nest attach ed`  ≈  emacsclient -s ed
```

The app's *unmodified* `(ui-run model view update display)` runs on the daemon; the
trick is the **display** — `remote-display` `:draw`s by `send`ing the frame over the
link and `:poll`s by `receive`ing the client's keys (the frame is plain Brood data, so
it just travels). `serve` spawns one **independent session per attaching client**, so
several frontends attach at once; `nest attach` is the thin `emacsclient`, connecting
*before* it takes the terminal so a bad spec/cookie is a clean error. The app-on-daemon
*push* complements the observer's *pull* remote-attach (§`nest observe --connect`).
Deferred (ADR-011): a shared model across clients, live resize, per-client viewports.

## Rust vs. Brood (ADR-006)

Everything new is **mechanism** — Unix sockets, filesystem paths, `0600`/`0700`
perms, CSPRNG — which ADR-006 assigns to Rust ("primitives the language can't
bootstrap: low-level I/O"), and which `nest observe` must reach without a Brood
image. So the smart-args live in the `node-start`/`connect` builtins, and the
path/cookie conventions are shared Rust functions rather than a `std/node.blsp`
wrapper.

*Alternative considered:* push the friendly wrappers + path derivation into a
`(require 'node)` module over thin `%node-listen`/`%node-dial` primitives. More
"language in the language," but it duplicates path derivation for the Rust
`nest observe` path. Rejected for now on that duplication; revisit if a second
consumer of the path convention appears that *is* Brood.

## Build sequence

1. `Stream` enum + generic `handshake`/`establish` (the transport seam).
2. `socket_path()` + Unix listen/dial + stale-socket handling.
3. `default_cookie()` + `(node-cookie)`.
4. Multi-arity `node-start`/`connect` builtins + `@`-based dispatch.
5. `nest run --name` + `observe` cookie fallback.
6. Tests, examples, docs, ADR-068.

Only step 1 is a nontrivial refactor; the rest is additive.

## Tests

`crates/cli/tests/distribution.rs` spawns real `brood` children (`:45`) — mirror
that for Unix:

- `two_unix_nodes_connect_and_message` — two children, `node-start` by name,
  `connect` by name, echo round-trip.
- `wrong_cookie_rejected_over_unix` — MAC mismatch closes the link.
- `cookie_file_autogen_and_reuse` — point `$HOME`/`$XDG_CONFIG_HOME` at a tempdir;
  assert the file appears `0600` and a second run reuses the same secret.

Cross-node behavior needs real processes, so coverage stays in `distribution.rs`
rather than the in-language suite.

## Open questions

- **Rust vs. `std/node.blsp` boundary** — keep it in Rust builtins (recommended),
  or the module split above?
- **Dual listen** — should a node serve *both* a local Unix socket (for
  same-box frontends) and a TCP/TLS endpoint (for remote attach) at once? The
  emacsclient/daemon end-state in M4 (`roadmap.md:441-444`) ultimately *wants*
  this — one editor core, local frontends by name + remote frontends over the
  network. The current plan defers it (arity-1 = Unix, addr = TCP) per ADR-011
  and "every milestone usable," shipping the single-transport forms first. Worth
  deciding whether the daemon goal pulls dual-listen forward, or whether it's
  cleanly additive later (it is — adding a second listener to an existing node
  needs no protocol change).
- **Windows** — Unix sockets exist on modern Windows but the `$XDG_RUNTIME_DIR`
  path convention doesn't. Out of scope until there's a Windows target; TCP still
  works everywhere.
