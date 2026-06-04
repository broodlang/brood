# Brood kernel audit — 2026-06-03

A thorough review of the Rust kernel (`crates/`) for refactoring opportunities,
performance issues, and GC / memory / VM / security / segfault risks. Conducted
via a multi-agent fan-out across six subsystems (GC/heap, VM/eval, builtins,
process/scheduler, distribution/net, value/types/reader); serious findings were
adversarially verified before inclusion. 33 findings raised, 2 refuted on
verification, 31 kept.

**Health: good.** The moving-GC rooting discipline, the closure-compiling VM,
and the distribution layer are largely sound. Three memory-safety / host-panic
defects rise above the noise; the rest are hardening and cleanup.

The top GC finding (#1) and the trigger path were confirmed directly against the
code at audit time (`heap.rs:4467-4477` drives a flip minor then a same-call
major; `minor_collect` retains `remembered` on a flip at `4588-4590`;
`major_collect` at `4654/4662` bumps `old_epoch` + relocates old frames but never
rewrites `remembered`).

---

## Critical & high-severity

### 1. [HIGH · memory safety] `major_collect` leaves stale env handles in the write-barrier `remembered` set
`crates/lisp/src/core/heap.rs:4652-4687` (consumed at `4565-4583`).

A *flip* minor retains `remembered` (`4588-4590`); `collect()` then runs
`major_collect` in the same call when the old gen has doubled (`4473-4477`).
`major_collect` relocates every old frame and bumps `old_epoch` but never
rewrites `remembered` — its comment ("`remembered` is empty") only holds after a
*tenure*. The next minor then indexes `self.old.envs[e.index()]` with
pre-major indices and **no epoch/bounds/poison check** → silent wrong-frame
read+write or a raw `Vec` OOB. **`BROOD_GC_VERIFY` does not catch it** (its
remembered-walk uses a safe `.get()` and ignores generation). Trigger is narrow
(`tenure → mid-bind env_define → flip → major → minor`) so it can't fire under
pure unbroken `GC_STRESS`, but it is reachable in mixed workloads.

**Fix:** rewrite each retained `remembered` `EnvId` through `fwd.envs` in
`major_collect` (or run a tenuring minor before the major, or skip the major
after a flip with a non-empty `remembered`). Add the interleave as a regression
test.

### 2. [HIGH · memory safety] Tail-call into an `&optional`-default arm registers the live-arm *after* `push_frame`
`crates/lisp/src/eval/compile.rs:1655-1663`.

The trampoline calls `push_frame(&c2, …)` *then* `heap.live_arm_set(arm_slot,
c2)`. But `push_frame` evaluates the arm's real (non-nil) `&optional` defaults,
which can fire a RUNTIME-region compaction (`runtime_collect`) that only
rewrites arms in `live_vm_arms` — and `c2` isn't registered yet, so its body and
unevaluated default RUNTIME handles point into the evacuated region → a
use-after-GC when the trampoline runs `c2.body`. **Release-relevant** (RUNTIME
handles aren't covered by the debug LOCAL epoch tripwire; surfaces as a distant
slab OOB / SIGSEGV). The first-arm path already orders it correctly (`vm_apply`,
`1555`→`1600`); the tail path inverts it.

**Fix (one line):** move `live_arm_set` to *before* `push_frame`. Add a
`BROOD_GC_STRESS` + runtime-churn test on a tail-recursive fn with a non-nil
`&optional` default.

### 3. [HIGH · host panic] `span-runs` overflows i64 on user-controlled `base`
`crates/lisp/src/builtins.rs:4040` (also `3979-3986`, `4105`).

Public builtin (used by `std/editor/highlight.blsp`). `base + chars.len()` is an
unchecked add. **Verified by execution:** `(span-runs "a"
9223372036854775807 [])` SIGABRTs the host in the default debug build and the
documented `debug-assertions=on --release` dev build; in plain release the add
wraps and panics on an OOB char slice. Violates "a Lisp program must never panic
the host."

**Fix:** compute `end` with `checked_add` → `INDEX_OUT_OF_RANGE` LispError;
`saturating_sub` in `span_runs_push`; clamp final slice bounds to `chars.len()`.

### 4. [HIGH · DoS] Authenticated peer can OOM the writer via an unbounded mpsc channel
`crates/lisp/src/dist.rs:949, 1013-1027`.

Each link's writer drains an unbounded `mpsc::channel::<Arc<[u8]>>`;
`WRITE_TIMEOUT` (30s) bounds a single `write_all`, not the queue. A peer that
slowlorises its read window stalls each write while local producers (`route`,
`monitor_remote`, `link_remote`, Pong, mesh gossip) keep enqueuing → unbounded
growth. The `WRITE_TIMEOUT` doc comment itself names this risk; the timeout is
an incomplete mitigation. The queue is filled chiefly by *local* outbound
producers while the remote stalls the drain — realistic in any cluster with
steady outbound traffic.

**Fix:** use a bounded `sync_channel` and tear the link down on `Full` (or track
an outstanding-bytes ceiling per `Conn`). A stalled peer should be disconnected,
not buffered.

### 5. [MEDIUM · DoS] `prealloc()` passes a byte count as an element capacity
`crates/lisp/src/dist/wire.rs:791-793` (sites `595, 604, 612, 645, 653, 661,
674`).

`prealloc(r, n) = n.min(remaining(r))` is a *byte* count but is fed to
`Vec::with_capacity` for elements that aren't 1 byte (`Message` = 48 B,
`(Message, Message)` = 96 B, `(Symbol, Message)` = 56 B). A near-`MAX_FRAME`
(64 MiB) frame with a huge collection count reserves ~6 GiB up front before the
decode fails on EOF — 48–96× amplification. Auth-gated, possible overcommit →
medium. The code already knows the right pattern (`FRAME_PEERS` gates on
`MAX_GOSSIP_PEERS`).

**Fix:** cap to a small constant (e.g. 1024) and let the `Vec` grow, or divide
`remaining()` by the element's minimum wire size.

---

## Performance

1. **[MEDIUM] `to-fixed` unbounded allocation** — `builtins.rs:4185-4196`.
   `(to-fixed 1.0 1000000000)` materialises a ~1 GB string via Rust `format!`,
   bypassing the GC/memory cap. Cap `n` (f64 has ~17 significant digits) like the
   existing `MAX_SHIFT` guard. *Highest-payoff robustness item here.*
2. **[LOW] `worker_count()` reads the `BROOD_J` env var on every spawn** —
   `scheduler.rs:617-631` (~17 µs/spawn + the global env lock). Cache in a
   `LazyLock<usize>`. Using `WORKERS.len()` as the modulus everywhere *also*
   structurally fixes the latent OOB-index finding below.
3. **[LOW] `remembered` grows without de-dup across repeated mid-bind tenures** —
   `heap.rs:3758-3768`. Add an `if !self.remembered.contains(&env)` guard at the
   push site.
4. **[LOW] Interner growth (process-lifetime leaks).** `resolve_in_source` interns
   arbitrary LSP identifiers on every hover/completion (`introspect.rs:173` — use
   `value::intern_existing`); `gensym` grows one entry per gensym per recompile
   (`value.rs:36-59`). Both matter only for the long-lived hot-reload daemon —
   worth a note in `docs/memory-model.md`.

---

## Refactoring

1. **[HIGH value] Delete the dead mark-sweep collector** — `heap.rs:4916-5208`.
   `collect_old` (`#[allow(dead_code)]`, never called), `sweep`, `trace_one`,
   `Marks`/`mark_methods!`/`mark_one`, `push_value`, `push_env`, `TraceItem`, the
   `FreeLists` struct, and the `local_free` field all linger under the live
   generational copying collector. `local_free` is written only by the dead
   `sweep`, so it is always empty — making the `free` subtraction in
   `local_live_count` (`4294-4314`) permanently zero and `purge_above`/`clear`
   no-ops. Several hundred lines of dead complexity in a 6100-line file. Delete it
   all (keep `PoisonBits`), simplify `local_live_count` to a raw slab-length sum.
2. **[LOW value] Stale doc premise** — `compile.rs` has **zero** Rust `unsafe { }`
   blocks; the "8 unsafe blocks" framing in `docs/handoff-vm-gc-memory.md` is
   stale (all `unsafe` matches are the `Scope::unsafe_slots` letrec machinery).
   The real audit surface is the rooting discipline + the live-arm registry.
3. **[LOW value] Harden "unreachable by prior check" sites** if ever touched —
   `builtins.rs:1974-1975, 2018-2019, 2392-2397, 2673, 2683, 2693` use
   `expect`/`unreachable!` guarded by preceding tag checks (safe today; prefer
   `LispError` on edit).

---

## Lower-confidence / accepted-by-design

### Distribution within the shared-cookie / Erlang trust model (confirmed behavior, accepted)
- Dialer doesn't verify the handshake-returned node name matches the intended
  peer — `dist.rs:873-894`. Cosmetic; crosses no real trust boundary (a
  cookie-holder already has RCE-by-design).
- HMAC accepts an empty/short cookie with no minimum-strength check —
  `handshake.rs:224, 74-77`. Default cookie is strong (`random-token 32`).
  **Worth a guardrail:** reject empty / <16-byte cookies in `node_listen`.
- Mesh gossip lets an authenticated peer steer outbound dials (SSRF-style) —
  `dist.rs:1167-1202`. Documented ADR-088 auto-mesh; fan-out capped
  (`MAX_GOSSIP_PEERS=4096`) + deduped. Informational.
- Inbound `Send`/`Exit`-by-pid to arbitrary local pid — `dist.rs:1055,
  1243-1252`. Erlang semantics; a per-pid capability model is the deferred
  multi-tenant ADR. Informational.
- **Positive finding:** `closure_from_message` rebuilds shipped code as inert
  data and never evals on receipt — `message.rs:429-470`. Closes the
  closure-shipping concern; keep the no-eval-until-applied invariant.

### Latent / not currently triggerable
- Epoch tripwire false-positives after 2^29 collections (29-bit GEN vs unmasked
  u32 epoch) — `heap.rs:2955-2972`. Debug-only, astronomically rare. Mask
  `expected` to `GEN_MASK`.
- `assign_worker`/`enqueue` OOB if `set_max_parallel` is called after the pool
  starts — `scheduler.rs:569-594`. Unreachable via Brood or current CLI callers.
  Structurally fixed by using `WORKERS.len()` as the modulus.
- Dead watcher's monitor entries leak from `MONITORS` until the watched target
  dies — `scheduler.rs:651-680`. Bounded-per-target. Sweep dead-watcher entries
  in `deregister`.
- `check_file` leaks GC roots only if a `(require)` eval *panics* —
  `check.rs:222-307`. The cited triggers return `Err`, not panic; ns/imports
  self-heal via `mem::replace`. RAII Drop guard would close it.
- `expr_ty`/`check_into` mutual recursion has no own depth guard —
  `guards.rs:233-423`, `walk.rs:263-483`. Safe today only transitively (reader
  cap 256 + macroexpand cap 256). Thread a depth counter for defense-in-depth.

### Correctness, low-impact (confirmed)
- `macroexpand` runtime fixpoint loop is unbounded — `macros.rs:712-726` (Rust)
  and `prelude.blsp:331` (the REPL-facing prelude one). Mitigated by green-process
  preemption / the MCP deadline watchdog; only a no-deadline root-thread
  expansion hard-hangs. Fix both with a `MAX_DEPTH`-style cap.
- `string->number` loses precision for integers > i64 (no bignum path) —
  `builtins.rs:5330-5339`, breaking the `number->string` inverse. Insert a
  `BigInt` parse between the i64 and f64 branches.
- `net.rs` tcp reader uses `from_utf8_lossy` — `net.rs:55-61, 93`. Corrupts
  binary protocol data *and* well-formed UTF-8 split across read-chunk
  boundaries. Faithful fix blocked on a missing `Message::Bytes` / bytevector
  Value type.
- Scanner counts only `\n` as a line break — `scanner.rs:57-64`. Lone-CR and
  U+2028/U+2029 sources get wrong `line:col` in diagnostics (CRLF is fine).
- `scan_string_body` silently swallows malformed `\x`/`\u{}` escapes as literal
  text — `scanner.rs:176-290`. A silent-wrong-output footgun; add a
  `StringScan::BadEscape` variant.
- `request_kill` publishes `kill_pending` outside the state lock —
  `mailbox.rs:125-139`. Correct (the state Mutex provides happens-before; the
  Relaxed flag is a fast pre-check). Comment-precision nit only.
- Worker thread can die if `deregister` panics (only `resume()` is in
  `catch_unwind`) — `scheduler.rs:761-831`. No reachable panic site today;
  defense-in-depth would wrap the whole `run_one` body.

---

## Recommended order

Fix the three memory-safety / host-panic items first (all small, localized):
the `live_arm_set` reorder (#2, one line) and `span-runs` overflow guards (#3)
are the highest-certainty, lowest-risk; the GC `remembered` rewrite (#1) deserves
its own focused change with a dedicated regression test. Then the two DoS gaps
(#4, #5) and the `to-fixed` cap. The dead-collector deletion is a high-value,
independent cleanup.
