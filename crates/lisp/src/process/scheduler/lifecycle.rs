//! Process lifecycle — birth, death, and exit propagation. `spawn`/`spawn_linked`/
//! `spawn_root_program` create a green process; `exit`/`exit_propagate`/`exit_with`
//! and `deregister` retire one (running its links/monitors and cleaning the
//! registries). Split out of `scheduler.rs`; the shared scheduling state (run
//! queue, worker pool, pid/parent tables, counters) stays in the root and is
//! reached here via `use super::*`, so this is a pure relocation.
use super::*;
use crate::error::Pos;
use crate::process::scheduler::pool::wake_a_parked_peer;

/// A human descriptor for a process in death/crash diagnostics: its registered
/// name plus pid when it has one (`ticker (pid 6)`), else the bare pid (`6`).
/// Read the name *before* `deregister` clears it. Used only on the cold death
/// path, so the `name_for_pid` scan is fine. (Keyword and symbol registrations
/// share the interner, so the name prints without a leading `:`.)
pub(super) fn proc_descr(pid: u64) -> String {
    match crate::dist::name_for_pid(pid) {
        Some(name) => format!("{} (pid {})", value::symbol_name(name), pid),
        None => pid.to_string(),
    }
}

/// A process has finished (or crashed): drop its mailbox and fire any
/// monitors, delivering `[:down <mref> <pid> <reason>]` to each watcher —
/// `Local` watchers via `deliver` (in-process mailbox push), `Remote`
/// watchers via the dist layer (an ordinary `send` to a remote pid, which
/// routes over the link). Same `[:down …]` shape in both cases — the
/// receiver code on the wire side is unchanged from local.
/// `heap` is `None` only on the quantum-tail panic-recovery path (`run_one`), where the
/// unwind has already dropped the process and its heap: everything here still runs
/// except `drain_note_exit`, which needs one. Every ordinary death passes `Some`.
pub(super) fn deregister(pid: u64, reason: Message, heap: Option<&Heap>) {
    crate::perf_time!(ns_teardown, { deregister_timed(pid, reason, heap) })
}

fn deregister_timed(pid: u64, reason: Message, heap: Option<&Heap>) {
    EXITED.fetch_add(1, Ordering::Relaxed);
    if crate::process::sysmon::armed() {
        // Exit event first, then the death-disarm check — so a monitor watching
        // :exit still sees every *other* process die, and its own death (never
        // self-reported) cleanly disarms the stream.
        crate::process::sysmon::emit_exit(pid, &reason);
        crate::process::sysmon::clear_if(pid);
    }
    // **NAMES is swept BEFORE the REGISTRY removal, and the order is load-bearing.**
    // The invariant it buys: a pid absent from REGISTRY is already absent from NAMES.
    // `%isolate`'s reap joins on REGISTRY-absence (`list_local_pids`), and with the old
    // order (REGISTRY first, NAMES later in `retire_pid_tail`) that join returned while
    // the dead pid was STILL name-registered — so the very next `(node/serve-spawns)`
    // saw `:remote-spawn` "already registered", declined to restart it, and every
    // subsequent `node/spawn` was sent to a corpse and silently dropped.
    // `tests/remote_spawn_test.blsp` failed 4/4 standalone on exactly this (its first
    // test is `:isolated`; the second ran inside the gap), and the same window under
    // suite load is a KI-80 contributor. The reporter's `proc_descr` reads the name
    // before `deregister` is called, so it still prints. The sweep in
    // `retire_pid_tail` stays (idempotent) for the `retire_root_ctx` path that shares
    // the tail.
    crate::dist::unregister_dead_pid(pid);
    // The three tables are taken **sequentially**, not nested: NAMES first (above),
    // released, then REGISTRY, released, then MONITORS. `add_monitor` and
    // `spawn_or_get` take REGISTRY *nested* inside MONITORS / NAMES
    // respectively for their own atomic check-and-modify steps — both are
    // deadlock-free precisely because `deregister` never holds an outer
    // lock while reaching for REGISTRY. Don't introduce a function that
    // holds REGISTRY while taking NAMES or MONITORS, or this becomes a
    // genuine ordering hazard.
    let mailbox = REGISTRY.remove(pid);
    // A process killed (link/monitor/`exit`) while parked never runs a status
    // transition back out of `ST_WAITING`, so square up the global parked count here —
    // else `parked_count` leaks upward and `report_parked_liveness` keeps scanning.
    if let Some(mb) = &mailbox {
        clear_parked(mb);
    }
    // Balances the `live_process_inc` in `spawn` (see the process-count-aware
    // `gc_floor`). `deregister` runs exactly once per spawned green process.
    crate::core::heap::live_process_dec();
    // Keep the O(1) drain-completion gate's ack count meaning "live processes that
    // acked clean": if this process had acked the current epoch, drop its ack now
    // that it's out of the live set (after the REGISTRY remove above, so the two
    // stay consistent). A no-op when no drain is armed.
    if let Some(heap) = heap {
        heap.drain_note_exit(pid);
    }
    retire_pid_tail(pid, reason);
}

/// The heap-independent half of retirement: sockets, registered names, monitors, links.
/// Split out so [`retire_root_ctx`] — which runs where no `Heap` is reachable — can share
/// exactly this teardown rather than a hand-copied approximation of it.
fn retire_pid_tail(pid: u64, reason: Message) {
    // Close any sockets this process still owns (an OS-process model: fds are
    // reclaimed on exit). Done before `notify_peers` below, so a linked supervisor
    // that restarts a dead listener finds its port already being freed. A process
    // that `tcp-close`d its sockets has none left here — this is a no-op then.
    crate::net::close_process_sockets(pid);
    // Likewise any child OS processes it still owns (ADR-104): kill + reap, the same
    // port-dies-with-its-owner semantics. Without this a crashed owner orphaned the
    // child, its registry entry, and both its reader threads for the runtime's life.
    crate::subprocess::close_process_procs(pid);
    // Drop any registered names that pointed at this pid — Erlang semantics
    // (a name lives only as long as its process). Without this, named-spawn
    // would see the stale entry as "already running" and never respawn.
    crate::dist::unregister_dead_pid(pid);
    let watchers = crate::core::sync::lock(&monitor::MONITORS)
        .remove(&pid)
        .unwrap_or_default();
    for w in watchers {
        monitor::fire_down(w, pid, reason.clone());
    }
    // The dead process's own watches: drop entries where *it* was the watcher,
    // or they leak until each watched target dies (kernel audit). Takes
    // MONITORS sequentially like everything in this function — never nested.
    monitor::sweep_dead_watcher(pid);
    // Links (ADR-067), after monitors and with no table lock held: notify every
    // linked peer — a trappable `[:EXIT pid reason]` if it traps, else an abnormal
    // reason propagates as a hard kill that cascades through *its* links. Mirrors
    // the sequential lock discipline above (never holds REGISTRY/MONITORS here).
    links::notify_peers(pid, &reason);
}

/// Retire a **root-thread context** — a pid + mailbox [`ensure_ctx`] minted for an OS
/// thread that touched `self`/`send`/`receive` without being a scheduler-run green
/// process. Called from that thread's [`RootCtxGuard`] when it exits (and by
/// [`deregister_root_ctx`] for a host that wants it deterministically).
///
/// Such a context had no death path at all before this: `deregister` runs only for green
/// processes and `shutdown_runtime_parked` reaps only *parked waiters*, so an embedder or
/// host that churns threads accumulated a registry entry, a mailbox (and every message
/// anyone ever sent it, since `deliver` still finds it), a monitor/link/name footprint and
/// one more entry in `live_pids()` per thread, for the whole OS-process lifetime — which
/// also inflated the `live` count the RUNTIME drain-completion gate compares against.
///
/// Deliberately **not** `deregister`: that would call `live_process_dec` (unbalanced — a
/// root ctx never ran `live_process_inc`, which is `spawn`'s) and needs a `Heap` for
/// `drain_note_exit`, which a dying thread no longer has. Leaving that drain ack behind is
/// safe: it can only make the O(1) `drain_acked_count() >= live` pre-gate fire early, and
/// the authoritative `gen_drained(&live_pids())` then re-checks against the *live* set,
/// from which this pid is now gone.
pub(super) fn retire_root_ctx(pid: u64) {
    let Some(mailbox) = REGISTRY.remove(pid) else {
        return; // already retired (an explicit `deregister_root_ctx`, then thread exit)
    };
    let reason = Message::Keyword(value::intern(pk::NORMAL));
    if crate::process::sysmon::armed() {
        crate::process::sysmon::emit_exit(pid, &reason);
        crate::process::sysmon::clear_if(pid);
    }
    clear_parked(&mailbox);
    // Drop anything still queued. A root ctx blocks on the condvar rather than parking a
    // `Box<Process>`, so `waiter` is normally `None` — but take it if some path did park
    // one, or its heap would ride out the OS process in a mailbox nothing can reach.
    let waiter = {
        let mut st = crate::core::sync::lock(&mailbox.state);
        st.queue.clear();
        st.kill = None;
        st.waiter.take()
    };
    drop(waiter);
    retire_pid_tail(pid, reason);
}

/// A short phrase for `pid`'s PENDING exit reason — **peeked, never taken**: the
/// retire path owns the reason and must still find it there. Used to give the
/// `Control::Kill` signal a message, so a kill that escapes to a top-level reporter
/// says what killed the process instead of rendering as a bare `runtime error:`
/// with nothing after it.
pub fn pending_kill_phrase(pid: u64) -> Option<String> {
    let mailbox = REGISTRY.get(pid)?;
    let reason = crate::core::sync::lock(&mailbox.state).kill.clone()?;
    Some(match reason {
        Message::Keyword(k) => format!(":{}", value::symbol_name(k)),
        Message::Vector(ref items) => match items.first() {
            Some(Message::Keyword(k)) => format!("[:{} …]", value::symbol_name(*k)),
            _ => "an exit signal".to_string(),
        },
        _ => "an exit signal".to_string(),
    })
}

/// The untrappable hard-kill reason — Erlang's `exit(pid, kill)`. A `:kill` exit
/// fires at the next reduction tick (`preempt`); any other reason is the soft
/// signal that waits for the next `receive` iteration.
pub(super) fn is_kill_reason(reason: &Message) -> bool {
    matches!(reason, Message::Keyword(k) if *k == value::intern(pk::KILL))
}

/// `(exit pid reason)` — deliver an exit signal to a green process (Erlang
/// `exit/2`). `reason = :kill` is the **untrappable hard** kill: the target dies at
/// its next reduction tick (`preempt`), or immediately if it's parked. Any other
/// reason is the **soft** signal: the target dies at its next `receive` iteration
/// (a tight non-`receive` loop won't honour it — cooperative). Monitors fire
/// `[:down mref pid reason]`. A no-op for an unknown / already-dead pid, so it's
/// idempotent (double-exit, exit-of-dead are safe).
pub fn exit(pid: u64, reason: Message) {
    let hard = is_kill_reason(&reason);
    // `:kill` is the *directive* "die untrappably", not a death reason — hardness rides
    // `kill_hard`, independent of the reason (see `exit_propagate`). Storing it verbatim
    // reported `:kill` to monitors and links, which no reader expects: `docs/language.md`
    // and `std/proc/supervisor` both name `:killed`, and `crash-report`'s deliberate-exit
    // list has `:killed` and not `:kill`, so a deliberate kill printed a crash report.
    // Every other path here already defaults to `:killed`; this was the one that didn't.
    let reason = if hard {
        Message::Keyword(value::intern(pk::KILLED))
    } else {
        reason
    };
    exit_with(pid, reason, hard);
}

/// Link propagation's kill: **hard** (the peer dies at its next reduction tick,
/// like `:kill`) but carrying the **originating reason** — so the peer's own
/// monitors and cascading links report *why* the tree fell (the BEAM behaviour),
/// not a blanket `:kill`. Hardness and reason are independent (`request_kill`).
pub(crate) fn exit_propagate(pid: u64, reason: Message) {
    exit_with(pid, reason, true);
}

fn exit_with(pid: u64, reason: Message, hard: bool) {
    let mailbox = match REGISTRY.get(pid) {
        Some(mb) => mb,
        None => return, // already dead / never existed
    };
    mailbox.request_kill(reason, hard);
    // If the target is waiting in `receive` it isn't running, so it'll never reach a
    // `tick` (preempt) or re-enter `receive` on its own — we must rouse it. Two waiting
    // shapes, woken the same way `deliver` wakes them for a message:
    //   * a **green waiter** (a captured, parked continuation): `wake_parked` re-queues
    //     it, and `park_on_receive` retires it on `kill_pending` when it runs.
    //   * a **cv-blocked** native-nested `receive` (or the root thread): no green waiter,
    //     so `wake_parked` is `None` and we `cv.notify_one()` (the `else` below); it wakes
    //     in `wait_for_message`, and `receive_match` unwinds with `Control::Kill`.
    // Taking the waiter under the state lock serialises with `run_one`'s park: either we
    // take an already-parked process here, or `run_one` sees `kill_pending` and retires it
    // instead of parking (exactly one wins). `request_kill` publishes `kill_pending`
    // *before* this lock, so a `wait_for_message` that locks after us sees it and won't
    // block through a lost `notify`.
    let (parked, cv_waiter) = {
        let mut st = crate::core::sync::lock(&mailbox.state);
        st.wake_pending = true;
        (wake_parked(&mut st), st.cv_waiters > 0)
    };
    // Both paths, unconditionally (`mailbox::wake_both`). The `else` this replaces assumed a
    // target is reachable by exactly one of them; a green process inside a native-nested
    // `receive` is reachable by both, and having a `waiter` suppressed the notify — so the
    // kill sat until some unrelated message happened to arrive, which for a process parked on
    // a `receive` nothing will ever send to is never. The cv wake is what makes the blocked
    // receiver see `kill_pending` and unwind with `Control::Kill` (`wait_for_message` /
    // `receive_match`); the re-queue is what retires an already-parked continuation.
    crate::process::mailbox::wake_both(&mailbox, parked, cv_waiter);
}

/// `(%spawn thunk)` — run `thunk` (a 0-arg function) as a new green process.
/// Returns the new pid. The user-facing `spawn` macro wraps an arbitrary
/// expression into such a thunk (`(spawn e)` → `(%spawn (fn () e))`), so the
/// expression's free locals are captured lexically rather than passed as args.
/// Erlang-style let-it-crash: an uncaught throw kills the process, monitors
/// fire `[:down :error …]` immediately.
pub fn spawn(heap: &Heap, f: Value) -> Result<u64, LispError> {
    spawn_impl(heap, f, false, None)
}

/// As [`spawn`], but **atomically links** the new child to the spawner *before* it can
/// run — the Erlang `spawn_link` primitive. Closes the spawn→link gap: a separate
/// `(link pid)` after the child's pid is returned can miss a child that already exited
/// (linking a dead pid yields `[:EXIT pid :noproc]`, *losing the real reason*). With the
/// link registered before the child is enqueued, its eventual exit always reaches the
/// parent as `[:EXIT pid reason]` with the **true** reason — so a fast `:normal` exit is
/// never misread as abnormal (which would spuriously restart a `:transient` supervised
/// child — see `supervisor.blsp`).
pub fn spawn_linked(heap: &Heap, f: Value) -> Result<u64, LispError> {
    spawn_impl(heap, f, true, None)
}

/// As [`spawn`], but **atomically monitors** the new child from the spawner *before* it
/// can run — the Erlang `spawn_monitor` primitive, and the monitor-side twin of
/// [`spawn_linked`]. Returns `(pid, mref)`.
///
/// The gap it closes is the same one, and just as real: `(let (p (spawn …) r (monitor p)))`
/// is the idiomatic monitor pattern, and it is only reliable while the spawner does not
/// yield between the two calls. Let any gap open — a loaded scheduler, or a boundary trace
/// wrapping the code — and a short-lived child exits first, `monitor` takes its
/// already-dead branch, and the DOWN arrives with `:noproc` **in place of the true exit
/// reason**. Measured: with the two calls adjacent, 0 of 300 runs lost the reason; with a
/// single 5 ms yield between them, 40 of 40 did.
///
/// That is a silent wrong answer rather than an error — the receive still fires, carrying
/// a reason that says only "it was already gone" — which is why the atomic form has to
/// exist rather than be a discipline callers remember.
pub fn spawn_monitored(heap: &Heap, f: Value) -> Result<(u64, u64), LispError> {
    let mref = crate::process::monitor::next_ref();
    let pid = spawn_impl(heap, f, false, Some(mref))?;
    Ok((pid, mref))
}

fn spawn_impl(
    heap: &Heap,
    f: Value,
    link_parent: bool,
    monitor_ref: Option<u64>,
) -> Result<u64, LispError> {
    crate::perf_time!(ns_spawn, {
        spawn_impl_timed(heap, f, link_parent, monitor_ref)
    })
}

fn spawn_impl_timed(
    heap: &Heap,
    f: Value,
    link_parent: bool,
    monitor_ref: Option<u64>,
) -> Result<u64, LispError> {
    // The spawner is the parent. Captured before minting the child pid so the
    // root (whose ctx/pid is lazily minted here on its first spawn) gets the
    // lower id. `ensure_ctx` needs no heap.
    let parent = self_pid();
    // Inherit a snapshot of the spawner's capture stack (the same `Arc`s), so a
    // child of an MCP-watchdog'd handler still diverts its output off the JSON-RPC
    // channel. An empty stack (the common case) clones to an empty `Vec`.
    let inherited_capture = CURRENT.with(|c| {
        c.borrow()
            .as_ref()
            .map(|ctx| ctx.capture.clone())
            .unwrap_or_default()
    });
    // Promote the thunk into the shared RUNTIME region so its handle (and any
    // captured local scope) is valid in the child, which shares this runtime's
    // code via the Arcs below. A top-level function is already shared (no-op).
    let f = heap.promote(f);
    if !matches!(f, Value::Fn(_)) {
        return Err(LispError::type_err("spawn: argument must be a function"));
    }
    let prelude = heap.prelude_arc();
    let runtime = heap.runtime_arc();

    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    SPAWNED.fetch_add(1, Ordering::SeqCst);
    if crate::process::sysmon::armed() {
        crate::process::sysmon::emit_spawn(pid, parent);
    }
    // Live green-process gauge: drives the process-count-aware `gc_floor` so a
    // fan-out of many churny processes doesn't each climb to the single-process
    // GC ceiling. Balanced by the `live_process_dec` in `deregister`.
    crate::core::heap::live_process_inc();
    let mailbox = Mailbox::new_with_parent(parent);
    // So `send` can tell this process shares our runtime without touching its heap.
    mailbox.set_runtime_tag(heap.runtime_tag());
    REGISTRY.insert(pid, Arc::clone(&mailbox));

    // Atomic link (`spawn_linked`): register the symmetric parent↔child link NOW — while
    // the child is registered (so `link`'s liveness check passes) but NOT yet enqueued, so
    // it cannot exit before the link exists. This is what makes the child's exit reason
    // reliable instead of a racy `:noproc`.
    if link_parent {
        super::links::link(parent, pid);
    }

    // Atomic monitor (`spawn_monitored`): same window, same reason as the link above —
    // registered while the child is in REGISTRY but not yet enqueued, so it cannot exit
    // before the monitor exists and the DOWN always carries the real reason.
    if let Some(mref) = monitor_ref {
        crate::process::monitor::add_monitor(
            pid,
            crate::process::monitor::Watcher::Local { pid: parent, mref },
        );
    }

    // State capture is the only engine now (ADR-100 §8.4 step 4 — corosensei removed):
    // the worker drives `vm_run_bc` directly, so a paused process is relocatable heap
    // data (migratable, no native stack). A VM-eligible body captures + migrates; a body
    // that defers to the tree-walker (vanishingly rare) runs tree-walked on the worker
    // with blocking `receive`s (`run_process_body`). `f` is a shared-runtime handle valid
    // in the child heap (same `runtime` Arc).
    let mut child = Heap::with_regions(prelude, runtime);
    child.set_global(EnvId::GLOBAL);

    // Package-rooted namespaces (ADR-070): the child inherits the spawner's package
    // context. "Which package is this code from?" is a property of the CODE, not of the
    // process running it — a worker spawned inside project `bedit` is still running
    // bedit's code, so its qualified intra-project references (`commands/cmd-open`) must
    // root exactly as they do on the parent. Without this, every spawned process (a test
    // body, a buffer server, a stream worker) resolves them as unbound the moment the
    // project is rooted. Empty pair outside a package, so this is free in that case.
    let (package_prefix, package_modules) = heap.package_context();
    child.set_package_context(package_prefix, package_modules);

    // Transparent causal-context propagation (opt-in, zero cost when unused): if the
    // spawner has a debugger trace context set (the debugger sets it inside a
    // `span`/`with-debugger`), promote it into the shared runtime — valid in the child,
    // like the thunk `f` above — and seed the child's trace-context slot with it. So a
    // plain `spawn`'d child inherits the debugger + causal span without an explicit
    // hand-off; its `break`/`span` just work and nest under the parent. When no context
    // is set (the default), this is one `Option` check per spawn and nothing more.
    // Gated on `dev-tools` (the debugger's feature), so a lean release compiles it out.
    #[cfg(feature = "dev-tools")]
    if let Some(ctx) = heap.trace_context() {
        // Only the spawner's OWN context propagates — a context merely adopted from a
        // received message stays with the handler, so it can't leak through unrelated
        // spawns (the child then owns its inherited copy and propagates it further).
        if heap.trace_context_own() {
            let ctx = heap.promote(ctx);
            child.set_trace_context(Some(ctx), true);
        }
    }

    ensure_workers();
    // Spawn placement is scan-free (BEAM model): local worker if we're on one, else
    // round-robin; work-stealing rebalances. The O(workers) least-loaded `assign_worker`
    // scan stays only on the wake/migration path (`wake_enqueue`), not the spawn hot path.
    let worker_id = pick_spawn_worker();
    let placed_on_self = CURRENT_WORKER.with(|c| c.get()) == Some(worker_id);
    enqueue(Box::new(Process {
        pid,
        mailbox,
        worker_id,
        heap: child,
        body: f,
        program: None,
        resume: None,
        capture: inherited_capture,
        queued_at: 0,
        spawns_since_park: 0,
    }));
    // We placed the child on our OWN queue, so it will not run until we yield. Whether
    // that matters depends entirely on the spawner: one that blocks for a reply yields in
    // a microsecond and should keep its child local on a warm cache; one that keeps
    // running holds it for a whole quantum. `spawns_since_park` tells them apart by
    // history rather than prediction — a second spawn with no block in between means this
    // process is not about to block — so only then do we hand the child to a parked peer.
    //
    // Gating on this matters: waking unconditionally costs the `supervisor` row 11%
    // (907 → 1007 ms) in futex wakes that find nothing, since its spawner drains its own
    // child every time. Placement itself is unchanged either way.
    if placed_on_self && spawns_since_park_bump() >= 2 {
        wake_a_parked_peer(worker_id);
    }
    Ok(pid)
}

/// Launch the whole top-level program as a single green process (ADR-135) and return the
/// [`ProgramExit`] the caller (the root/main thread) blocks on. `src`/`file` are the
/// program source; `heap` is the caller's heap, used only to borrow the shared
/// prelude/runtime regions (the program runs in its **own** LOCAL heap that shares them,
/// so its `def`s land in the shared runtime globals exactly as any process's do). The
/// forms are read into that heap and pinned on its root stack for the program's life.
///
/// Unlike `eval_source` on the root thread, the program now runs on a worker in capture
/// mode: a top-level driver talking to a spawned worker uses the userspace direct-handoff
/// path (no per-message cross-thread futex), and a top-level `receive` parks-and-captures.
pub fn spawn_root_program(
    heap: &Heap,
    src: &str,
    file: Option<String>,
    preamble: Option<&str>,
) -> Result<Arc<crate::eval::compile::ProgramExit>, LispError> {
    let prelude = heap.prelude_arc();
    let runtime = heap.runtime_arc();
    let mut child = Heap::with_regions(prelude, runtime);
    child.set_global(EnvId::GLOBAL);
    // The program's file must be CURRENT before the read, not merely recorded in
    // `ProgramState` for error rendering: the reader stamps every form's position record
    // with `current_file_arc` at read time, and without this every form of a
    // `brood script.blsp` program was born `file=None`. That stayed invisible for months
    // because the expander's list rebuilds re-stamped positions with the ambient file
    // during eval (right by coincidence), until ADR-297 made rebuilds copy the original
    // record faithfully — faithfully preserving the hole, which broke coverage/attribution
    // for every directly-run program (`std_attribution` + four `coverage_lines` tests,
    // 2026-08-29).
    child.set_current_file(file.clone());

    // Read the program into the child heap and pin every form on its root stack — the
    // driver re-fetches each by index so a collection between forms relocates them safely.
    let mut forms =
        crate::syntax::reader::read_all_positioned(&mut child, src).map_err(|e| match &file {
            Some(f) => e.or_file(f.clone()),
            None => e,
        })?;
    // A preamble runs INSIDE the program's process, ahead of its first form — the hook
    // `brood file` uses to arm the default crash reporter (ADR-305) so that `(self)`
    // there is the program's pid, the one process whose crash the CLI already prints.
    // Armed from the root context instead, the reporter would print every top-level
    // error a second time. Unpositioned: the forms are the runtime's, not the file's.
    if let Some(pre) = preamble {
        let mut lead = crate::syntax::reader::read_all(&mut child, pre)?
            .into_iter()
            .map(|f| (f, Pos { line: 0, col: 0 }))
            .collect::<Vec<_>>();
        lead.append(&mut forms);
        forms = lead;
    }
    let root_base = child.roots_len();
    let mut positions = Vec::with_capacity(forms.len());
    for (form, pos) in &forms {
        child.push_root(*form);
        positions.push(*pos);
    }

    let exit = crate::eval::compile::ProgramExit::new();
    let prog =
        crate::eval::compile::ProgramState::new(root_base, positions, file, Arc::clone(&exit));

    // Register the process exactly as `spawn_impl` does (minus the body-thunk promote): a
    // mailbox in the REGISTRY, the live-process gauge, no parent (it's the root program).
    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    SPAWNED.fetch_add(1, Ordering::SeqCst);
    crate::core::heap::live_process_inc();
    let mailbox = Mailbox::new();
    mailbox.set_runtime_tag(heap.runtime_tag());
    REGISTRY.insert(pid, Arc::clone(&mailbox));

    ensure_workers();
    let worker_id = pick_spawn_worker();
    let placed_on_self = CURRENT_WORKER.with(|c| c.get()) == Some(worker_id);
    enqueue(Box::new(Process {
        pid,
        mailbox,
        worker_id,
        heap: child,
        body: Value::nil(),
        program: Some(Box::new(prog)),
        resume: None,
        // Inherit the caller's capture stack, exactly as `spawn` does above. A ROOT
        // program is still someone's callee: the wasm playground calls
        // `begin_stdout_capture` and then runs the snippet through here, so with an
        // empty stack every `io/puts` went to the real stdout — which in a browser is
        // nowhere, and the snippet's output silently vanished from the page.
        capture: CURRENT.with(|c| {
            c.borrow()
                .as_ref()
                .map(|ctx| ctx.capture.clone())
                .unwrap_or_default()
        }),
        queued_at: 0,
        spawns_since_park: 0,
    }));
    if placed_on_self && spawns_since_park_bump() >= 2 {
        wake_a_parked_peer(worker_id);
    }
    Ok(exit)
}
