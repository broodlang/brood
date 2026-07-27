# Elixir / OTP parity

Brood is architecturally a BEAM language — green processes, share-nothing
immutability, message-passing, links/monitors, hot reload, distribution,
supervision. So Elixir/OTP, not Node, is the meaningful yardstick for its
concurrency and fault-tolerance surface. This document audits where Brood stands
against Elixir *capability by capability* (a separate track measures raw
performance against BEAM).

The headline: **the hard runtime machinery is already present, and several
pieces exceed BEAM.** The remaining gaps are mostly library surface and a couple
of data-model features — not the scheduler/dist core. Audit performed 2026-07-11
against the live tree; file references are `path:line` at that time and are
pointers, not guarantees they haven't moved.

Guiding rule (ADR-039, whose full text is archived in
[`archive/decisions-superseded.md`](archive/decisions-superseded.md)): **mechanism
in Rust** (spawn/send/receive/link/monitor/trap-exit/exit/scheduler/dist), **policy
in Brood** (`std/proc/*`, `std/*`). Most "gaps" below are policy to write in the
language, not kernel work.

## At or above parity — no work needed

| Area | Status | Notes |
|------|--------|-------|
| **Preemptive scheduling** | ✅ meets/exceeds | Reduction-counted preemption (`REDUCTION_BUDGET` ≈ 2000, `scheduler.rs`), continuation capture at frame boundaries — not merely cooperative. |
| **SMP + work-stealing** | ✅ meets/exceeds | Per-worker run queues ≈ `nproc`, work-stealing, **plus live continuation migration across workers** — BEAM migrates processes but not via heap-captured continuations. |
| **Selective `receive`** | ✅ meets | True BEAM save-queue semantics — non-matching messages stay queued, a `scanned` cursor avoids re-scanning (`mailbox.rs`). |
| **`receive ... after`** | ✅ meets | `after ms` deadline and `after 0` non-blocking peek; deadline persists across resumes. |
| **`send` semantics** | ✅ meets | Deep-copy across per-process heaps; `{:name :node}` addressing; silent no-op to dead target (Erlang semantics). |
| **gen_server** (`std/proc/gen.blsp`) | ✅ meets | `handle_call`/`cast`/`info`, `init`/`terminate`; `call` **monitors** the server so a dead server fails fast rather than hanging; 5 s default timeout. Bonus `query` clause for read-only replies. |
| **Supervisor** (`std/proc/supervisor.blsp`) | ✅ meets | All three strategies (`:one-for-one`/`:one-for-all`/`:rest-for-one`); restart intensity (max_restarts/max_seconds); `:permanent`/`:transient`/`:temporary`; child specs; `start-child`/`terminate-child`/`count-children`/`which-children`; reverse-order shutdown; `:brutal-kill`/`:infinity`/ms. |
| **Agent** (`std/agent.blsp`) | ✅ complete | `get`/`update`/`get-and-update` (atomic)/`cast`/`stop`. |
| **Task** (`std/task.blsp`) | ✅ mostly | async/await/cancel + debounce; missing `Task.Supervisor`/`async_stream`. |
| **Links + `trap_exit`** | ✅ meets | Bidirectional links propagate exits; `trap-exit` converts a peer death to `[:EXIT pid reason]`. |
| **Distribution** | ✅ **exceeds** | See below. |
| **Pattern matching** | ✅ mostly | Literals, non-linear binders, pins (`~`, Elixir `^`), guards (`:when`), list head/tail `(p & rest)`, vector/tuple `[…]`, arbitrary nesting. Missing: binary patterns, general map-value subpatterns. |

### Distribution exceeds BEAM

Where classic BEAM distribution ships **cleartext + a static cookie**, Brood's
node links are, per `dist.rs` / `dist/*`:

- **Encrypted by default** — cookie-HMAC handshake, then ephemeral **X25519
  ECDH** (forward secrecy) → HKDF → per-direction **ChaCha20-Poly1305**; a
  tampered/replayed frame fails the AEAD tag and tears the link down.
- **Unix-domain transport** for same-machine nodes — no EPMD, no port
  allocation; **TCP** for cross-machine, same handshake/framing on both.
- **AST/closure shipping** — code travels as data (`remote-spawn`), no
  BEAM-module/code-server dance.
- **Cross-node monitor/link** with `:noconnection`/`:noproc`, heartbeat failure
  detection (2 s/6 s), and **mesh gossip** (`Frame::Peers`) that closes a full
  mesh from one static bootstrap address.
- DoS hardening BEAM lacks: in-flight-handshake cap, bounded writer queue that
  severs on overflow, pre-auth frame cap, decode-depth cap.

## Ranked gaps that make sense

### Tier 1 — high leverage
1. **Binary pattern matching + bit syntax** (`<<a::8, rest::binary>>`). The
   flagship BEAM feature Brood lacks. The ROADMAP already feels the pain — the
   HTTP/WS parsers bridge through a Latin-1 "carrier string" because there's no
   rich bytes-match surface. `Value::Bytes` and the `match*` engine both already
   exist to build on. Highest-leverage gap for protocol/byte parsing and the
   display protocol.
2. **Efficient byte / iolist append buffer.** Kills the O(n²) `(str acc x)` /
   `bytes-concat`-in-a-loop accumulation (the actual ROADMAP stability item).
   Must be a **GC-quiet in-place build inside one Rust builtin that returns a
   fresh immutable `Bytes`** — never a mutable value the language can observe
   (immutability invariant, ADR-026).
3. **Registry + `:global` + `:pg`.** Only a flat unique-name table today
   (`register`/`whereis`, no `unregister`, local-only). Missing: keyed Registry
   (name→many pids + dispatch, `:via` tuples), a cluster-global registry with
   conflict resolution, and process groups. Blocks "name a dynamic pool of
   processes" and most real distributed patterns.

### Tier 2 — real, medium leverage
4. **`send_after` / cancelable timers** (`Process.send_after`/`cancel_timer`).
   The timer subsystem exists but only services `receive ... after`; a
   scheduled-message primitive + cancel is a modest extension.
5. **`spawn_monitor`** (atomic; currently only non-atomic spawn+monitor) **and
   exit-reason fidelity on link propagation** — a non-trapping peer currently
   dies reporting `:kill` to its monitors instead of the originating reason,
   which can subtly break supervisors/tests that match on reason (`links.rs`
   "D-simple" simplification).
6. **Value-returning RPC** (`:rpc.call`/`:erpc`) — today `remote-spawn-sync`
   returns a *pid*, not a computed value; **auto-connect on first send** (`route`
   silently drops to unconnected nodes); **`:nodeup` events + `monitor_nodes`**
   (only `[:nodedown]` exists, and per-node); **always-on remote-spawn service**
   (BEAM's `rex` is always up).
7. **Protocols / multimethods** — open polymorphic dispatch to replace the
   hand-written `type-of` cascades in the generic seq/string ops.

### Tier 3 — ergonomics / defer-until-consumer
- **String interpolation** — ✅ shipped as `fmt`: `(fmt "x={x}")` (Brood's hole
  syntax is `{expr}`, not Elixir's `#{expr}`); lowers to a plain `str`, ADR-154.
- **Named / keyword `&key` arguments** — "designed but not in this version"
  (`language.md`); maps cover options today.
- **Grapheme-correct string API** — strings are **codepoint-indexed** (Rust
  `chars()`), not grapheme-indexed like Elixir's `String`. `unicode-segmentation`
  is already a dependency (wired only to `display-width`), so a `graphemes`
  primitive is cheap.
- **Application behaviour** — start/stop lifecycle + env/config + tree root;
  ADR-011 defers it to a real consumer (the editor app may be it).
- **Per-process `max_heap_size`** — only a global soft memory limit today
  (per-process deferred, ADR-011); a runaway process can OOM the node.
- Lower still: `Task.Supervisor`/`async_stream`, nominal structs with
  `@enforce_keys` + a predicate (`defrecord` is advisory-typed map sugar today),
  gen_server `code_change`/state versioning, process priorities, process
  dictionary, `simple_one_for_one` template-child as a first-class construct.

## Correctly N/A — do not chase

These are BEAM/Erlang traits that do **not** make sense to copy into a Lisp:

- **Charlists** — Brood has exactly one string type; a "list of ints" is just a
  list. No `~c` wart to reconcile.
- **The keyword-*list* type** — maps are the better fit and Brood already uses
  them for options.
- **General sigil syntax** — reader/`format`/`fmt` cover most; only perhaps a
  regex literal is worth wanting (string interpolation now ships as `fmt`).
- **Tuples vs vectors** — already cleanly solved: the vector `[…]` is the
  tuple/tagged-data idiom (`[:ok v]`, `[:add a b]`).

## Notable Brood-specific wins Elixir lacks

- Live continuation migration + on-demand overflow drainer threads in the
  scheduler.
- Closures/AST and ETS-table handles sendable in messages (intra-runtime handle
  sharing; cross-node downgrades to copy).
- Encrypted-by-default, Unix-socket-capable distribution (above).
- Reduction-preemptible bytecode running directly on worker threads.
