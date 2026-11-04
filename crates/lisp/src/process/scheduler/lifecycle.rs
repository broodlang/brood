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
pub(in crate::process) fn proc_descr(pid: u64) -> String {
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
        // Exit event first, then (below, after the REGISTRY removal) the death-disarm
        // — so a monitor watching :exit still sees every *other* process die, and its
        // own death (never self-reported) cleanly disarms the stream.
        crate::process::sysmon::emit_exit(pid, &reason);
    }
    // The name sweep and the REGISTRY removal are ONE step under the NAMES lock
    // (`unregister_dead_pid_while`), which buys two invariants: a pid absent from
    // REGISTRY is already absent from NAMES (`%isolate`'s reap joins on REGISTRY
    // absence, and a name left on the corpse made the next `node/serve-spawns` decline
    // to restart — KI-80), and a concurrent `proc/register` either binds before the
    // sweep or sees the pid gone. Lock order is NAMES → REGISTRY; never hold REGISTRY
    // while taking NAMES, MONITORS, LINKS or the sysmon table.
    let mailbox = crate::dist::unregister_dead_pid_while(pid, || REGISTRY.remove(pid));
    // The death-disarm runs AFTER the removal, and unconditionally. After: a
    // `proc/system-monitor` install checks liveness under the sysmon lock, so either it
    // saw this pid alive and installed before this clear (which removes it), or it sees
    // the pid gone and refuses — before, an install landing between the clear and the
    // removal armed `:exit-abnormal` on a corpse for good, which silenced the kernel's
    // own crash line (`crash_reported_elsewhere`) for every later crash. Unconditional:
    // `armed()` is the selected-KIND mask, 0 for a subscriber that selects nothing, and
    // such a subscriber was never reaped (KI-97); `clear_if` early-outs on its own
    // subscription count.
    crate::process::sysmon::clear_if(pid);
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
        // The tables it created (and owns) die with it — before
        // the downs below make the death observable (KI-183's rule).
        crate::core::table::release_owned(pid, heap.owned_tables());
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
    crate::host::net::close_process_sockets(pid);
    // Likewise any child OS processes it still owns (ADR-104): kill + reap, the same
    // port-dies-with-its-owner semantics. Without this a crashed owner orphaned the
    // child, its registry entry, and both its reader threads for the runtime's life.
    crate::host::subprocess::close_process_procs(pid);
    // And any wasm instances it loaded — each holds a whole store and linear memory.
    #[cfg(feature = "wasm")]
    crate::host::wasm::close_process_instances(pid);
    // And the tree-sitter trees it held (ADR-495): a handle is its owner's, never evicted
    // by anyone else's allocations, so its owner's exit is what frees a forgotten one.
    crate::host::treesit::close_process_trees(pid);
    // Registered names and node monitors were swept with the REGISTRY removal
    // (`unregister_dead_pid_while`), by both callers.
    //
    // The dead process's own watches (it as the watcher) are dropped BEFORE the downs
    // fire: a `[:down …]` makes this death observable, so everything it releases must
    // be released by then (KI-183). Takes MONITORS sequentially — never nested.
    monitor::sweep_dead_watcher(pid);
    // The LOCAL downs are pushed while `MONITORS` is still held, so `take_target` and
    // the pushes are one step to a `demonitor` on another thread: with the lock released
    // between them, a `demonitor` + `(receive ([:down ^m …]) (after 0))` in that gap
    // found nothing to remove and nothing queued, and the down landed after — 31 leaks
    // in 80 000 rounds (KI-213). Safe to hold here: `deliver` takes only the target's
    // mailbox lock and a scheduler queue, and nothing takes those and then `MONITORS`.
    // A remote down goes over the wire, after the lock is released.
    let remote = {
        let mut mons = crate::core::sync::lock(&monitor::MONITORS);
        let mut remote = Vec::new();
        for w in mons.take_target(pid) {
            match w {
                monitor::Watcher::Local { .. } => {
                    monitor::fire_down_held(&mons, w, pid, reason.clone())
                }
                monitor::Watcher::Remote { .. } => remote.push(w),
            }
        }
        remote
    };
    for w in remote {
        monitor::fire_down(w, pid, reason.clone());
    }
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
    // Swept with the removal, under the NAMES lock — see `deregister_timed`.
    let Some(mailbox) = crate::dist::unregister_dead_pid_while(pid, || REGISTRY.remove(pid)) else {
        return; // already retired (an explicit `deregister_root_ctx`, then thread exit)
    };
    let reason = Message::Keyword(*pk::NORMAL_SYMBOL);
    if crate::process::sysmon::armed() {
        crate::process::sysmon::emit_exit(pid, &reason);
    }
    // Unconditional, after the REGISTRY removal above — see `deregister_timed`.
    crate::process::sysmon::clear_if(pid);
    clear_parked(&mailbox);
    // Drop anything still queued. A root ctx blocks on the condvar rather than parking a
    // `Box<Process>`, so `waiter` is normally `None` — but take it if some path did park
    // one, or its heap would ride out the OS process in a mailbox nothing can reach.
    let waiter = {
        let mut st = crate::core::sync::lock(&mailbox.state);
        st.clear_queue();
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
    let reason = crate::core::sync::lock(&mailbox.state).kill_reason()?;
    Some(exit_reason_phrase(&reason))
}

/// A short phrase naming an exit `reason` — `:noproc`, `[:error …]` — for the
/// "killed by an exit signal: …" message.
pub(crate) fn exit_reason_phrase(reason: &Message) -> String {
    match *reason {
        Message::Keyword(k) => format!(":{}", value::symbol_name(k)),
        Message::Vector(ref items) => match items.first() {
            Some(Message::Keyword(k)) => format!("[:{} …]", value::symbol_name(*k)),
            _ => "an exit signal".to_string(),
        },
        _ => "an exit signal".to_string(),
    }
}

/// The untrappable hard-kill reason — `(exit pid :kill)`. A `:kill` exit
/// fires at the next reduction tick (`preempt`); any other reason is the soft
/// signal that waits for the next `receive` iteration.
pub(super) fn is_kill_reason(reason: &Message) -> bool {
    matches!(reason, Message::Keyword(k) if *k == *pk::KILL_SYMBOL)
}

/// `(exit pid reason)` — deliver an exit signal to a green process.
/// `reason = :kill` is the **untrappable hard** kill: the target dies at
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
        Message::Keyword(*pk::KILLED_SYMBOL)
    } else {
        reason
    };
    exit_with(pid, reason, hard);
}

/// Link propagation's kill: **hard** (the peer dies at its next reduction tick,
/// like `:kill`) but carrying the **originating reason** — so the peer's own
/// monitors and cascading links report *why* the tree fell,
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
    //   * a **green waiter** (a captured, parked continuation): the `Wake` re-queues it,
    //     and `park_on_receive` retires it on `kill_pending` when it runs.
    //   * a **cv-blocked** native-nested `receive` (or the root thread): the `Wake`'s
    //     condvar notify wakes it in `wait_for_message`, and `receive_match` unwinds with
    //     `Control::Kill`.
    // Taking the waiter under the state lock serialises with `run_one`'s park: either we
    // take an already-parked process here, or `run_one` sees `kill_pending` and retires it
    // instead of parking (exactly one wins). `request_kill` publishes `kill_pending`
    // *before* this lock, so a `wait_for_message` that locks after us sees it and won't
    // block through a lost `notify`.
    let wake = crate::core::sync::lock(&mailbox.state).take_wake();
    // Both paths, unconditionally (`mailbox::Wake`). The `else` this replaces assumed a
    // target is reachable by exactly one of them; a green process inside a native-nested
    // `receive` is reachable by both, and having a `waiter` suppressed the notify — so the
    // kill sat until some unrelated message happened to arrive, which for a process parked on
    // a `receive` nothing will ever send to is never. The cv wake is what makes the blocked
    // receiver see `kill_pending` and unwind with `Control::Kill` (`wait_for_message` /
    // `receive_match`); the re-queue is what retires an already-parked continuation.
    wake.fire(&mailbox);
}

/// `(%spawn thunk)` — run `thunk` (a 0-arg function) as a new green process.
/// Returns the new pid. The user-facing `spawn` macro wraps an arbitrary
/// expression into such a thunk (`(spawn e)` → `(%spawn (fn () e))`), so the
/// expression's free locals are captured lexically rather than passed as args.
/// Let-it-crash: an uncaught throw kills the process, monitors
/// fire `[:down :error …]` immediately.
pub fn spawn(heap: &Heap, f: Value) -> Result<u64, LispError> {
    spawn_impl(heap, f, false, None)
}

/// As [`spawn`], but **atomically links** the new child to the spawner *before* it can
/// run — the `spawn-link` primitive. Closes the spawn→link gap: a separate
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
/// can run — the `spawn-monitor` primitive, and the monitor-side twin of
/// [`spawn_linked`]. Returns `(pid, mref)`.
///
/// The gap it closes is the same one, and just as real: `(let (p (spawn …) r (monitor p)))`
/// is the idiomatic monitor pattern, and it is only reliable while the spawner does not
/// yield between the two calls. Let any gap open — a loaded scheduler, or a boundary trace
/// wrapping the code — and a short-lived child exits first, `monitor` takes its
/// already-dead branch, and the DOWN arrives with `:noproc` **in place of the true exit
/// reason**; a single yield between the two calls is enough.
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
    // Everything the child takes from the spawner's context, in ONE borrow of it:
    //
    // - the spawner is the parent. Read before minting the child pid so the root (whose
    //   ctx/pid is lazily minted here on its first spawn) gets the lower id. Minting
    //   needs no heap.
    // - a snapshot of the spawner's capture stack (the same `Arc`s), so a child of an
    //   MCP-watchdog'd handler still diverts its output off the JSON-RPC channel. An
    //   empty stack (the common case) clones to an empty `Vec`.
    // - the isolate scope we are running under, stamped on the child so an `%isolate`
    //   can reap what it spawned without walking a `parent` chain that dies with any
    //   intermediate process (KI-89). Only this process's own `%isolate` changes it, and
    //   not during this call, so reading it here rather than at the mailbox's creation
    //   below reads the same value. `0` when no isolate is active.
    let (parent, inherited_capture, isolate_scope) = super::with_ctx(|ctx| {
        (
            ctx.pid,
            ctx.capture.clone(),
            ctx.mailbox.isolate_scope.load(Ordering::Relaxed),
        )
    });
    if !matches!(f, Value::Fn(_)) {
        return Err(LispError::type_err("spawn: argument must be a function"));
    }
    let prelude = heap.prelude_arc();
    let runtime = heap.runtime_arc();
    let mut child = Heap::with_regions(prelude, runtime);
    // The body, valid in the child: a closure that captures locals is copied into the
    // child's own heap, code shared and captured values copied (ADR-405); anything else is
    // promoted into the shared RUNTIME region — a no-op for a top-level function.
    let f = crate::process::message::copy_closure_into(heap, f, &mut child)
        .unwrap_or_else(|| heap.promote(f));

    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    SPAWNED.fetch_add(1, Ordering::SeqCst);
    if crate::process::sysmon::armed() {
        crate::process::sysmon::emit_spawn(pid, parent);
    }
    // Live green-process gauge: drives the process-count-aware `gc_floor` so a
    // fan-out of many churny processes doesn't each climb to the single-process
    // GC ceiling. Balanced by the `live_process_dec` in `deregister`.
    crate::core::heap::live_process_inc();
    // Stamp the child with the isolate scope read above — OUR scope, which `%isolate`
    // swaps for the duration of its thunk.
    let mailbox = Mailbox::new_with_parent(parent, isolate_scope);
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

    // The acting writer (ADR-596): what the child registers outside a load is credited to
    // whoever the spawner's writes are — the module whose load is spawning it, else the
    // spawner's own acting writer. A timer or a worker a plugin starts thereby registers as
    // the plugin, and taking the plugin's writes back takes those too. A copy of a `Copy`
    // field: nothing for a spawn outside a load with no acting writer.
    child.acting_writer = heap.current_registry_writer();

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
    // Spawn placement is scan-free: local worker if we're on one, else
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
    // Gating on this matters: waking unconditionally costs a spawner that drains its own
    // child every time (the `supervisor` row) futex wakes that find nothing. Placement
    // itself is unchanged either way.
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
    want_result: bool,
) -> Result<Arc<crate::eval::compile::ProgramExit>, LispError> {
    let prelude = heap.prelude_arc();
    let runtime = heap.runtime_arc();
    let mut child = Heap::with_regions(prelude, runtime);
    child.set_global(EnvId::GLOBAL);
    // Package-rooted namespaces (ADR-070): the program inherits the caller's package
    // context, exactly as a spawned child does above. `nest run FILE` runs `project/setup`
    // (which installs the project's context) and then launches the file through here —
    // without this the file's `(:use model)` would load the project's module a second
    // time, bare, beside the rooted one, registering every `deftype` alias twice
    // (KI-129). Empty pair outside a package, so this is free for a plain `brood FILE`.
    let (package_prefix, package_modules) = heap.package_context();
    child.set_package_context(package_prefix, package_modules);
    // The program's file must be CURRENT before the read, not merely recorded in
    // `ProgramState` for error rendering: the reader stamps every form's position record
    // with `current_file_arc` at read time, and without this every form of a
    // `brood script.blsp` program is born `file=None` — which, since rebuilds copy the
    // original record (ADR-297), breaks coverage/attribution for directly-run programs.
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

    let exit = crate::eval::compile::ProgramExit::new(want_result);
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

#[cfg(test)]
mod root_ctx_name_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    /// A retiring root context drops its registered names in the same step as its
    /// REGISTRY entry: an observer never sees the pid dead yet still named (which is
    /// what `%isolate`'s reap and named-spawn rely on). Probabilistic by nature — the
    /// observer spins across 2000 retirements to land in any window that exists.
    #[test]
    fn a_retired_root_ctx_is_never_dead_but_still_named() {
        let name = value::intern("root-ctx-retire-probe");
        let current = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let observer = {
            let (current, stop) = (current.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut violations = 0u64;
                while !stop.load(Ordering::Acquire) {
                    let pid = current.load(Ordering::Acquire);
                    if pid != 0
                        && !crate::process::is_alive(pid)
                        && crate::dist::whereis(name) == Some(pid)
                    {
                        violations += 1;
                    }
                }
                violations
            })
        };
        for _ in 0..2000 {
            let current = current.clone();
            std::thread::spawn(move || {
                let pid = crate::process::self_pid();
                crate::dist::register(name, pid);
                current.store(pid, Ordering::Release);
                assert!(crate::process::deregister_root_ctx());
            })
            .join()
            .expect("root-ctx thread panicked");
        }
        stop.store(true, Ordering::Release);
        let violations = observer.join().expect("observer panicked");
        assert_eq!(
            violations, 0,
            "a root ctx was observed dead in REGISTRY while still holding its name"
        );
    }
}

#[cfg(test)]
mod down_order_tests {
    use super::*;

    /// KI-183: a dying process releases the monitors IT held before any watcher of it is
    /// told it died. The waiter a `[:down …]` wakes may read a target's `:monitored-by`
    /// straight away, and it must not count the dead process. Fake pids, no processes: the
    /// table is built directly and only the death path's ordering is under test.
    #[test]
    fn a_dying_process_has_released_its_own_monitors_when_its_down_fires() {
        let (watcher, dying, target) = (u64::MAX - 30, u64::MAX - 31, u64::MAX - 32);
        monitor::test_insert_local(dying, watcher, u64::MAX - 40); // watcher monitors dying
        monitor::test_insert_local(target, dying, u64::MAX - 41); // dying monitors target
        monitor::down_order_probe::take();
        retire_pid_tail(dying, Message::Nil);
        let still_held = monitor::down_order_probe::take();
        crate::core::sync::lock(&monitor::MONITORS).take_target(target);
        assert_eq!(
            still_held,
            vec![false],
            "the down fired while the dying process still held its own monitors (KI-183)"
        );
    }
}
