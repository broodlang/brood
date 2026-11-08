//! The process primitives: spawn/link/monitor/send, the named-process registry, process
//! flags and introspection (`%processes`, scheduler statistics), and the system monitor.
//! Mechanism only — `std/prelude/process.blsp` and `std/proc/*` are the policy.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use super::args::{arg, expect_keyword};
use super::nodes::expect_node_name;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::{Arity, Tag};
    use crate::types::{Sig, Ty};
    // processes (concurrency)
    primitives.def(
        "%spawn",
        Arity::exact(1),
        Sig::new(vec![callable], pid_ty),
        &["thunk"],
        "Run thunk (a 0-arg fn) in a new green process; returns its pid. Use the `spawn` macro.",
        spawn,
    );
    primitives.def(
        "%spawn-link",
        Arity::exact(1),
        Sig::new(vec![callable], pid_ty),
        &["thunk"],
        "Like %spawn but atomically links the child to the caller before it runs (no spawn->link :noproc race). Use the `spawn-link` macro.",
        spawn_link);
    primitives.def(
        "%spawn-monitor",
        Arity::exact(1),
        Sig::new(vec![callable], vec_ty),
        &["thunk"],
        "Like %spawn but atomically monitors the child from the caller before it runs, returning [pid ref] — so the [:down ref pid reason] carries the child's REAL exit reason even for an instant exit, instead of the :noproc a separate (monitor p) reports when the child got there first. Use the `spawn-monitor` macro.",
        spawn_monitor,
    );
    primitives.def(
        "%spawn-named",
        Arity::exact(2),
        Sig::new(vec![sym.union(kw), callable], pid_ty),
        &[],
        "",
        spawn_named,
    );
    // `send`'s target is a pid, a registered name (keyword), OR a `{:name :node}` address map.
    primitives.def(
        "send",
        Arity::exact(2),
        Sig::new(vec![pid_ty.union(map_ty).union(kw), any], nil_ty),
        &["target", "msg"],
        "Copy msg into target's mailbox; target is a pid, a registered name (a keyword — the local node's `{:name :node}`), or a {:name :node} address. Routes locally or over a node link. Returns nil.\n\n    (do (send (self) :doc-hello) (receive (:doc-hello :got-it)))   ;=> :got-it",
        send);
    // Mailbox lanes (ADR-490): a message a receiver takes AHEAD of its backlog, and one a
    // newer message with the same key replaces while it is still queued. Both are `send`
    // to a live local pid and an ordinary `send` to anything else.
    primitives.def(
        "proc/send-priority",
        Arity::exact(2),
        Sig::new(vec![pid_ty.union(map_ty).union(kw), any], nil_ty),
        &["target", "msg"],
        "Send msg to target on its mailbox's PRIORITY lane: `proc/take-priority` in the receiver takes it ahead of every ordinary message queued before it, oldest priority message first. What a window delivers as the user's keys and mouse is on this lane, so a UI loop serves input before a flood of background replies (ADR-490). An ordinary `receive` ignores the lane and sees it in arrival order. The lane holds for any target: a local registered name (`:editor`), a `{:name :node}` address, and a pid or name on another node, whose runtime queues the message in the same lane (ADR-492). A dead target drops it, as `send` does. Returns nil.\n\n    (do (send (self) :background) (proc/send-priority (self) :key) [(proc/take-priority) (receive (m m))])   ;=> [[:key] :background]",
        send_priority);
    primitives.def(
        "proc/take-priority",
        Arity::exact(0),
        Sig::nullary(vec_ty.union(nil_ty)),
        &[],
        "Take the calling process's oldest PRIORITY-lane message (`proc/send-priority`, a window's input) out of its mailbox, as `[msg]` — or nil at once when none is queued. Never waits and never looks at an ordinary message, so it costs nothing in a mailbox thousands of background messages deep: the input-first half of a UI loop's poll (ADR-490).\n\n    (do (send (self) :background) (proc/take-priority))   ;=> nil",
        take_priority);
    primitives.def(
        "proc/receive-lane",
        Arity::exact(1),
        Sig::new(vec![int.union(nil_ty.clone())], vec_ty.union(nil_ty)),
        &["ms"],
        "Take the oldest message of any kind, waiting up to ms (nil: forever), and say which lane it came on: `[msg :priority]` for one sent with `proc/send-priority` (a window's input), `[msg :normal]` otherwise; nil on timeout. `(receive (m m) (after ms nil))` with the lane — what lets a UI loop that parked tell the key that woke it from a background reply (ADR-490).\n\n    (do (send (self) :reply) (proc/send-priority (self) :key) [(proc/receive-lane 0) (proc/receive-lane 0) (proc/receive-lane 0)])   ;=> [[:reply :normal] [:key :priority] nil]",
        receive_lane);
    primitives.def(
        "proc/send-latest",
        Arity::exact(3),
        Sig::new(vec![pid_ty.union(map_ty).union(kw), any, any], nil_ty),
        &["target", "key", "msg"],
        "Send msg to target, REPLACING any message sent with the same key that target has not received yet — for state that only matters at its newest value (a file's diagnostics, a progress figure, a status chip), so a fast producer cannot bury a slow receiver in stale copies (ADR-490). The replacement queues at the back, in its own arrival order; a message already received is never touched. key is compared by its printed form: a keyword, string, number or a vector of them. Messages without a key are never replaced. The lane holds for any target: a local registered name (`:editor`), a `{:name :node}` address, and a pid or name on another node, whose runtime queues the message in the same lane (ADR-492). A dead target drops it, as `send` does. Returns nil.\n\n    (do (proc/send-latest (self) :progress 1) (send (self) :other) (proc/send-latest (self) :progress 2) [(receive (m m)) (receive (m m)) (receive (m m) (after 0 :empty))])   ;=> [:other 2 :empty]",
        send_latest);
    primitives.def(
        "proc/interruptible",
        Arity::exact(2),
        Sig::new(vec![int.union(nil_ty.clone()), callable], vec_ty.clone()),
        &["ceiling-ms", "thunk"],
        "Run `(thunk)` in an INTERRUPT WINDOW (ADR-491): `[:ok value]`, or `[:interrupted e]` when it was stopped mid-computation — by `proc/interrupt`, by the process's quit key (`proc/set-quit-key`), or, when ceiling-ms is a number, by itself once that many ms have passed. `e` is the map a `catch` binds (`{:kind :interrupted :code \"E0047\" …}`). The interrupt is raised at the next safepoint — within about a millisecond of work, at every tier, inside long builtins — and it is STICKY: a `catch` inside thunk that swallows it is interrupted again at its next safepoint, so only this call ends it. Any other error propagates as it would. Windows nest (ADR-493): a window opened inside an open one is stopped by a request while it is the innermost, and by its own ceiling; an outer window's ceiling passes through it to the outer call, and an interrupt it answered does not stop the outer one. A process blocked in `receive` is not interrupted until it runs again (a ceiling) or at all (a request).\n\n    (proc/interruptible nil (fn () 42))   ;=> [:ok 42]\n    (proc/interruptible nil (fn () (first (proc/interruptible 0 (fn () (letrec (spin (fn (n) (spin (inc n)))) (spin 0)))))))   ;=> [:ok :interrupted]\n    (first (proc/interruptible 0 (fn () (letrec (spin (fn (n) (spin (inc n)))) (spin 0)))))   ;=> :interrupted\n    (first (proc/interruptible 0 (fn () (letrec (stubborn (fn (n) (do (try (stubborn-spin n) (catch e nil)) (stubborn (inc n)))) stubborn-spin (fn (n) (stubborn-spin (inc n)))) (stubborn 0)))))   ;=> :interrupted",
        interruptible);
    primitives.def(
        "proc/interrupt",
        Arity::exact(1),
        Sig::new(vec![pid_ty], bool_ty),
        &["pid"],
        "Interrupt pid at its next safepoint (ADR-491) — Emacs's quit, from outside: its running computation raises a catchable `{:kind :interrupted …}` there. Only a local process RUNNING inside an open interrupt window (`proc/interruptible`) is interrupted; a process blocked in `receive`, one with its window closed, a dead or a remote pid is left alone, and nothing is remembered for later. True when the interrupt landed.\n\n    (first (proc/interruptible nil (fn () (proc/interrupt (self)) (letrec (spin (fn (n) (spin (inc n)))) (spin 0)))))   ;=> :interrupted\n    (proc/interrupt (self))   ;=> false",
        interrupt_proc);
    primitives.def(
        "proc/busy?",
        Arity::exact(1),
        Sig::new(vec![pid_ty], bool_ty.union(nil_ty.clone())),
        &["pid"],
        "Whether pid is BUSY (ADR-491): true while it has been running in one open interrupt window (`proc/interruptible`) for at least its quit key's busy-ms (300 without one), false while its window is open but younger, nil when none is open — it is idle, gone, or remote. What a frontend's busy cursor asks of the loop it feeds: nil says stop asking until the next input.\n\n    (proc/busy? (self))   ;=> nil\n    (proc/interruptible nil (fn () (proc/busy? (self))))   ;=> [:ok false]",
        busy_proc);
    primitives.def(
        "proc/gc-us",
        Arity::exact(0),
        Sig::nullary(int),
        &[],
        "The calling process's cumulative garbage-collection time in µs: the wall time its own LOCAL heap's collections took, since it started. Two reads around a step say how much of it was GC — what a UI loop's slow-turn report carries as `:gc-ms`, telling a collection from slow code (ADR-490). Two counter reads, no allocation: cheap enough to take around every turn.\n\n    (>= (proc/gc-us) 0)   ;=> true",
        gc_us);
    primitives.def(
        "proc/set-quit-key",
        Arity::range(2, 3),
        Sig::with_optional(
            vec![kw.union(string).union(int).union(vec_ty.clone()).union(nil_ty.clone()), int],
            vec![pid_ty.union(nil_ty.clone())],
            nil_ty.clone(),
        ),
        &["key", "busy-ms", "from"],
        "Declare the calling process's QUIT KEY (ADR-491), Emacs's `quit-char`: when key — a keyword, string or integer, the shape a frontend delivers a keystroke in (a window's C-g is `:ctrl-g`) — arrives on its PRIORITY lane (`proc/send-priority`) while it has been running in an open interrupt window for at least busy-ms, the kernel interrupts it as `proc/interrupt` does. key may instead be a VECTOR of such atoms, the shape a wire frontend ships a key in (`[:key :ctrl-g]` from a thin client): it then matches a vector message that BEGINS with them, on any lane — lanes do not cross a node link, and such a message's tag already says it is input. The message is queued as well, so the app still sees its key. A key from ANOTHER node (a thin client's, over the link) interrupts only when it comes from the node of `from` — the pid of the client this loop serves — or, with no `from`, from a full peer: never from a GUEST link (ADR-415) that merely reaches this pid. nil clears it; with none set a delivery pays nothing. Returns nil.\n\n    (do (proc/set-quit-key :ctrl-g 0) (let (r (proc/interruptible nil (fn () (proc/send-priority (self) :ctrl-g) (letrec (spin (fn (n) (spin (inc n)))) (spin 0))))) (proc/set-quit-key nil 0) [(first r) (receive (m m))]))   ;=> [:interrupted :ctrl-g]",
        set_quit_key);
    // Arg shape: (matcher: callable|spec vector, timeout: int|nil, tags: vector|nil). The
    // `receive` macro in `std/prelude.blsp` expands to exactly this. The matcher
    // answers `[idx var…]` for the clause that matched (nil = no match) — a closure, or
    // a pattern spec the kernel matches itself when every clause is a simple shape
    // (ADR-409, `process/receive_spec.rs`); a timeout
    // answers nil. `tags` is the set of leading keywords the clauses can match, or
    // nil to scan everything — a pure filter that lets the scan reject a message by
    // peeking its tag instead of rebuilding it into the heap (see `receive--tags`).
    // `pin` (4th) is the receive-mark hint: the value every clause pins, when they all pin
    // the same one (see `receive--pin`). If it is a `ref` this process minted, the scan can
    // start past every message that predates it — nil disables the hint (ADR-195).
    primitives.def(
        "%receive",
        Arity::exact(4),
        Sig::new(vec![any, int.union(nil_ty), any, any], any),
        &[],
        "",
        receive_match,
    );
    primitives.def(
        "%receive-spec-key",
        Arity::exact(1),
        Sig::new(vec![vec_ty.clone()], kw.union(nil_ty.clone())),
        &[],
        "",
        receive_spec_key,
    );
    primitives.def(
        "self",
        Arity::exact(0),
        Sig::nullary(pid_ty),
        &[],
        "This process's own pid (carries this node's identity).\n\n    (pid? (self))        ;=> true\n    (= (self) (self))    ;=> true",
        self_pid,
    );
    primitives.def(
        "ref",
        Arity::exact(0),
        Sig::nullary(ref_ty),
        &[],
        "A fresh, globally-unique reference token (tags a request to its reply).\n\n    (= (ref) (ref))           ;=> false\n    (let (r (ref)) (= r r))   ;=> true",
        make_ref,
    );
    // `(exit pid reason)` — send an exit signal. `:kill` is the
    // untrappable hard kill; any other reason is the soft (next-`receive`) signal.
    primitives.def(
        "exit",
        Arity::exact(2),
        Sig::new(vec![pid_ty, any], nil_ty),
        &["pid", "reason"],
        "Send an exit signal to process pid, local or remote. reason :kill is the untrappable hard kill — pid dies at its next reduction tick, or immediately if parked. Any other reason is the soft signal — pid dies at its next receive. Monitors fire [:down ref pid reason]. A remote pid is routed to its node over the link. No-op for a dead/unknown pid. Returns nil.\n\n    (let (p (spawn (receive (_ nil))) r (monitor p)) (exit p :kill) (receive ([:down ^r ^p why] why)))   ;=> :killed",
        exit_proc);
    // `monitor` also accepts a name map (forwarded to the remote node).
    primitives.def(
        "monitor",
        Arity::exact(1),
        Sig::new(vec![pid_ty.union(map_ty)], ref_ty),
        &["pid"],
        "Watch pid; returns a monitor ref. Delivers [:down ref pid reason] when pid dies.\n\n    (let (p (spawn (receive (:go :done))) r (monitor p)) (send p :go) (receive ([:down ^r ^p why] why)))   ;=> :normal",
        monitor,
    );
    primitives.def(
        "demonitor",
        Arity::exact(1),
        Sig::new(vec![ref_ty], nil_ty),
        &["mref"],
        "Drop the monitor identified by mref (best-effort).\n\n    (let (p (spawn (receive (_ nil))) r (monitor p)) (demonitor r) (exit p :kill) (receive ([:down ^r _ _] :fired) (after 50 :quiet)))   ;=> :quiet",
        demonitor,
    );
    // One-shot reply aliases: the kernel drops a
    // message addressed to a ref its owner has deactivated, at DELIVERY — so a reply
    // that misses its caller's deadline never enters the mailbox at all.
    primitives.def(
        "%ref-deactivate",
        Arity::exact(1),
        Sig::new(vec![ref_ty], nil_ty),
        &["ref"],
        "Deactivate ref as a one-shot reply alias on this process's mailbox: a later message addressed to it ([:tag ref ...]) is dropped at delivery and never queued. What gen/call-timeout uses so a reply posted after the deadline cannot pile up. Already-queued messages are untouched. Returns nil.",
        ref_deactivate,
    );
    // Links (ADR-077): symmetric failure coupling + `trap_exit`, the bidirectional
    // cousin of `monitor`. `link`/`unlink` couple the current process to a pid;
    // `proc/trap-exit` turns a linked peer's death into a `[:EXIT pid reason]` message.
    primitives.def(
        "link",
        Arity::exact(1),
        Sig::new(vec![pid_ty], nil_ty),
        &["pid"],
        "Symmetrically link the current process and pid, local or remote. When either dies, the other gets a [:EXIT pid reason] message if it set (proc/trap-exit true), else dies too on an abnormal reason (propagation cascades through links; :normal does not propagate). A remote link fires :noconnection on net-split; linking an already-dead/unreachable pid notifies the caller (:noproc / :noconnection). Returns nil.\n\n    (link pid)   ;~> nil, and a crash of either now reaches the other",
        link_proc);
    primitives.def(
        "unlink",
        Arity::exact(1),
        Sig::new(vec![pid_ty], nil_ty),
        &["pid"],
        "Drop the symmetric link between the current process and pid (local or remote; best-effort). Returns nil.\n\n    (unlink pid)   ;~> nil",
        unlink_proc);
    primitives.def(
        "proc/trap-exit",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        &["on"],
        "Set the current process's trap_exit flag; returns the previous value. When on, a linked peer's death arrives as a trappable [:EXIT pid reason] message instead of killing this process.\n\n    (proc/trap-exit true)   ;~> false, the previous setting",
        trap_exit_proc);
    primitives.def(
        "%process-flag",
        Arity::range(1, 2),
        Sig::new(vec![any, any], any),
        &["flag", "&optional", "value"],
        "Read or set a per-process runtime flag on the current process; returns the previous (or, with no value, current) setting. Flags: :max-heap — this process's heap limit in bytes (positive int sets, nil clears, absent reads). Checked after each GC against the live footprint; exceeding it raises a catchable E0045 error in this process only — uncaught, it kills just the offender (the global BROOD_MEM_LIMIT hard cap aborts the whole runtime). Set it first thing in a spawned fn to cap that process: (spawn (fn () (proc/flag :max-heap 8000000) (work))). :max-mailbox — this process's mailbox bound in MESSAGES (ADR-307; positive int sets, nil clears — clearing also cancels a pending trip — absent reads). Checked by every sender at enqueue; a breach raises a catchable E0046 in THIS process at its next safepoint or receive. The sender is never blocked and no message is dropped: this is the guard against a receiver that cannot keep up eating the machine, not a backpressure channel (that stays a library concern - gen/call with a timeout). :send-errors — when truthy, a (send …) whose target NODE is unknown/disconnected raises a catchable E0060 noconnection error instead of silently dropping the message (process liveness stays silent either way) — so a sender can queue-and-retry across a net-split; pairs with the reconnect reconnector. :no-break — when truthy, a breakpoint (std/debug break) passes this process through instead of parking it: for infrastructure other processes wait on (a buffer process, a registry), where a parked process freezes whoever asked it something and nothing could resume it. :responsive — for a process whose job is to keep answering (an editor loop, a REPL): a native call that waits on the outside world (os/cmd, run-process, a clipboard read) is reported to it after the wait as the message [:blocking-call {:native :detail :ms}] when truthy, or refused with a catchable E0048 before the wait when :refuse (ADR-517); nil or false is ordinary. The read answers nil, :report or :refuse.",
        process_flag);
    primitives.def(
        "%hibernate",
        Arity::exact(0),
        Sig::nullary(Ty::of(Tag::Int)),
        &[],
        "Tell the runtime this process is about to idle for a long time: collect, shrink its heap slabs and root vectors, and drop its inline caches and compiled-body cache. Returns the bytes of slab capacity released; the process resumes from its receive. Use it in a process that will wait a long while (a pooled connection, an idle session actor) — it trades a one-off cache rebuild on the next call for a substantially smaller idle footprint. Do NOT use it in a request loop: dropping the caches per park costs message-heavy code 12-26%, which is exactly why this is an explicit call and not automatic.",
        hibernate_proc);
    primitives.def(
        "%sched-stats",
        Arity::exact(0),
        Sig::nullary(map_ty),
        &[],
        "A snapshot map of the scheduler's cumulative counters: {:spawned :exited :preempts :steals :migrations :workers :peak-threads}. :spawned - :exited is the live-process figure; :preempts counts reduction-budget quantum exhaustions; :steals/:migrations count work-stealing activity. The scheduler half of the observability timing tier (pairs with gc-stats' :pause-* keys).",
        sched_stats);
    primitives.def(
        "proc/system-monitor",
        Arity::range(0, 2),
        Sig::new(vec![any, any], any),
        &["&optional", "pid", "opts"],
        "Read, arm, or clear a subscription to the kernel system monitor — runtime events pushed to subscriber processes as [:system kind subject-pid detail] mailbox messages (the observability event stream's kernel sources). Kinds: :gc {:pause-us :collections :live} (a collection of subject's heap finished), :spawn (detail = parent pid), :exit (detail = the structured exit reason monitors see; :exit-abnormal selects only CRASHES — a reason other than :normal, :kill/:killed, :shutdown or [:shutdown x], the deliberate exits a supervisor produces — filtered before any message is built), :deopt (detail = the JIT arm's fn name, or nil). ONE SUBSCRIPTION PER SUBSCRIBER PID (ADR-305): no args reads the CALLER's config map (nil if none); (proc/system-monitor :all) lists every subscription; (proc/system-monitor nil) clears the caller's and (proc/system-monitor nil pid) clears pid's; (proc/system-monitor pid) arms every event at pid (raising if pid is not a live process);(proc/system-monitor pid {:gc true :gc-min-pause-us 1000 :exit-abnormal true}) selects exactly the truthy keys (:gc-min-pause-us = report only pauses that long). Arming/clearing returns that pid's PREVIOUS config. Events about a subscriber itself are never sent to it (no feedback loops), and a subscriber's death drops its subscription. Policy lives in telemetry/watch-runtime (re-emits as telemetry events) and crash-report (the default crash reporter).\n\n    (proc/system-monitor (self) {:exit-abnormal true})   ;~> nil, the previous config",
        system_monitor);
    primitives.def(
        "%spawn-count",
        Arity::exact(0),
        Sig::nullary(int),
        &[],
        "How many green processes have been spawned since program start.",
        spawn_count,
    );
    primitives.def(
        "%peak-threads",
        Arity::exact(0),
        Sig::nullary(int),
        &[],
        "High-water mark of OS threads running processes concurrently.",
        peak_threads,
    );
    primitives.def(
        "%worker-threads",
        Arity::exact(0),
        Sig::nullary(int),
        &[],
        "The size of the scheduler's worker-thread pool (about nproc).",
        worker_threads,
    );
    primitives.def(
        "%steal-count",
        Arity::exact(0),
        Sig::nullary(int),
        &[],
        "How many fresh processes the scheduler work-stole across worker threads since program start; 0 means placement-at-spawn kept the pool even.",
        steal_count);
    primitives.def(
        "%list-processes",
        Arity::exact(0),
        Sig::nullary(list_ty),
        &[],
        "Every currently-live pid on this runtime (one per registered mailbox). Order is unspecified — sort if you need stability. For agents/tools enumerating spawned processes.",
        list_processes);
    primitives.def(
        "proc/register",
        Arity::exact(2),
        // A name may be a symbol OR a keyword — `expect_node_name` accepts both, and
        // `:name` lookups in `send`/`node-name` use keywords, so the sig must too.
        Sig::new(vec![sym.union(kw), pid_ty], pid_ty),
        &["name", "pid"],
        "Bind a local name so peers can address this process via {:name name :node this-node}. Returns the pid. Replaces any previous holder of the name. A pid that has already exited binds nothing — the same outcome as registering it and the process exiting, which releases its names.\n\n    (proc/register :logger pid)   ;~> pid",
        register_name);
    primitives.def(
        "proc/whereis",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw)], pid_ty.union(nil_ty)),
        &["name"],
        "The local pid registered under `name`, or nil. Strictly local — does not query other nodes.\n\n    (proc/whereis 'no-such-registered-name)   ;=> nil",
        whereis_name);
    primitives.def(
        "proc/unregister",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw)], bool_ty),
        &["name"],
        "Release the name `name`, so nothing is registered under it; true if a process was bound to it, false if it was already free. The inverse of `proc/register`, which had none — a name could only be released by its process dying, so a service could not hand its name to a replacement or step down without exiting. Strictly local.\n\n    (proc/unregister :brood-doc-nobody)   ;=> false",
        unregister_name);
    primitives.def(
        "proc/alive?",
        Arity::exact(1),
        Sig::new(vec![pid_ty], bool_ty),
        &["pid"],
        "Is `pid` a live process on this node? False for a dead pid, and false for a remote one (liveness is not knowable locally). Cheaper than the `(proc/info pid)` snapshot, which answered the same question by allocating a whole map.\n\n    (proc/alive? (self))   ;=> true",
        process_alive);
    // The one process-introspection accessor the language can't reach from Brood
    // (the mailbox queue lives behind the scheduler registry). Everything else an
    // observer shows — pid id, liveness — is assembled in Brood (std/tool/observer.blsp).
    primitives.def(
        "%mailbox-size",
        Arity::exact(1),
        Sig::new(vec![pid_ty], int.union(nil_ty)),
        &["pid"],
        "How many messages are queued in pid's mailbox (its receive backlog), or nil if pid is not a live local process. The one process-introspection accessor not reachable from Brood; see std/tool/observer.blsp.",
        mailbox_size);
    // `(process-info pid)` — a snapshot map for a
    // live local process (nil for remote/dead), the introspection surface a
    // process observer/debugger reads. Assembled in Rust because every field is
    // kernel-internal (registry / scheduler / monitor tables). ADR-051.
    primitives.def(
        "%process-info",
        Arity::exact(1),
        Sig::new(vec![pid_ty], map_ty.union(nil_ty)),
        &["pid"],
        "A snapshot map of a live local process: {:id :pid :node :name :status :mailbox :monitored-by :parent :memory :collections :reductions} (:pid the process's pid value, for acting on it with exit/send/monitor; :status is :running or :waiting; :name nil if unregistered; :parent the spawner's id, nil for the root; :memory the LOCAL heap bytes and :collections the cumulative GC count, both as of the process's last receive; :reductions the cumulative reduction count — the scheduling unit, updated every quantum; exact for spawned processes, coarse for the root). nil for a remote/dead pid. The introspection the observer reads; see std/tool/observer.blsp.",
        process_info);
}

pub(super) fn spawn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pid = crate::process::spawn(heap, arg(args, 0))?;
    Ok(crate::process::pid_value(pid))
}

/// `(%spawn-link thunk)` — atomic `spawn` + `link`: the new child is linked to the
/// caller *before* it runs, so its exit reason is delivered reliably even on an instant
/// exit (no spawn→link `:noproc` race). The `spawn-link` macro wraps an expression.
pub(super) fn spawn_link(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pid = crate::process::spawn_linked(heap, arg(args, 0))?;
    Ok(crate::process::pid_value(pid))
}

/// `(%spawn-monitor thunk)` — atomic `spawn` + `monitor`: the caller monitors the new
/// child *before* it runs, so the DOWN carries the child's true exit reason even for an
/// instant exit (no spawn→monitor `:noproc` race). Returns `[pid ref]`. The
/// `spawn-monitor` macro wraps an expression.
pub(super) fn spawn_monitor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (pid, mref) = crate::process::spawn_monitored(heap, arg(args, 0))?;
    Ok(heap.alloc_vector2(crate::process::pid_value(pid), Value::ref_(mref)))
}

/// `(%spawn-named name thunk)` — idempotent named spawn. If `name` (a
/// keyword or symbol) is currently registered to a still-alive pid, return
/// that pid and **do not** spawn — `thunk` is never evaluated. Otherwise,
/// drop any stale registration, spawn the thunk as a new green process,
/// register it under `name`, and return the new pid.
///
/// The check-or-spawn step is atomic under `NAMES`'s write lock — two
/// concurrent `(spawn :name …)` calls can't both spawn; the loser sees
/// the winner's pid. The user-facing `(spawn name expr)` macro wraps an
/// expression into a thunk the same way `(spawn expr)` does, so the
/// expression's free locals are captured lexically (ADR-033).
pub(super) fn spawn_named(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Keyword(s) | Value::Sym(s) => s,
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%spawn-named",
                "keyword or symbol",
                v,
            ));
        }
    };
    let thunk = arg(args, 1);
    if !matches!(thunk, Value::Fn(_)) {
        return Err(LispError::wrong_type(
            heap,
            "%spawn-named",
            "function",
            thunk,
        ));
    }
    // `spawn_or_get`'s spawner is fallible — `?` propagates a real
    // `LispError` if `process::spawn` rejects the thunk (defensive: with the
    // `Value::Fn(_)` type-check above, that shouldn't fire today, but a
    // future change to `promote`/`spawn` won't silently panic).
    let pid = crate::dist::spawn_or_get(name, || crate::process::spawn(heap, thunk))?;
    Ok(crate::process::pid_value(pid))
}

pub(super) fn send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::send(heap, arg(args, 0), arg(args, 1))?;
    Ok(Value::nil())
}

/// `(proc/send-priority target msg)` — `send` on the priority lane (ADR-490).
pub(super) fn send_priority(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::send_in_lane(heap, arg(args, 0), arg(args, 1), None)?;
    Ok(Value::nil())
}

/// `(proc/send-latest target key msg)` — `send`, superseding a queued message with `key`.
pub(super) fn send_latest(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::send_in_lane(heap, arg(args, 0), arg(args, 2), Some(arg(args, 1)))?;
    Ok(Value::nil())
}

/// `(proc/receive-lane ms)` — the oldest message as `[msg lane]`, or nil on timeout.
pub(super) fn receive_lane(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::receive_lane(heap, arg(args, 0))
}

/// `(proc/take-priority)` — the oldest priority-lane message as `[msg]`, or nil.
pub(super) fn take_priority(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::take_priority(heap)
}

/// A non-negative millisecond count, or nil.
fn optional_ms(heap: &Heap, who: &str, value: Value) -> Result<Option<u64>, LispError> {
    match value {
        Value::Int(n) if n >= 0 => Ok(Some(n as u64)),
        Value::Nil => Ok(None),
        other => Err(LispError::wrong_type(
            heap,
            who,
            "a non-negative number of ms or nil",
            other,
        )),
    }
}

/// `proc/interruptible`'s ceiling argument: a non-negative number of ms, or nil for none.
/// Shared with the bytecode driver, which reads it when it runs the call as a region.
pub(crate) fn interrupt_ceiling(heap: &Heap, value: Value) -> Result<Option<u64>, LispError> {
    optional_ms(heap, "proc/interruptible", value)
}

/// `(proc/interruptible ceiling-ms thunk)` — run thunk in an interrupt window (ADR-491).
///
/// The bytecode driver never calls this for a VM thunk: it runs the call — `apply`'d too — as
/// an interrupt REGION on its own frame stack (`ChunkExit::Interruptible`), and the tier-0
/// machine as a frame (`KontKind::Interruptible`, ADR-494), so a `receive` in the thunk parks
/// the process. This is what is left: a thunk with no VM arm above tier 0, or a call made
/// under a native frame (a builtin's callback), where the thunk runs nested under this frame
/// and a `receive` in it blocks the worker as any native-nested receive does — the worker
/// leaves the executors and its queue moves (ADR-100 §7.4). Both open and close the window and
/// catch the interrupt with no Brood code between a raise and the close for a sticky
/// re-raise to land in. A window opened inside an open one is a window of its own (ADR-493):
/// it answers `[:interrupted e]` only for an interrupt raised FOR it — a request while it is
/// the innermost, or its own ceiling — and lets one raised for an outer window pass.
///
/// A `receive` under this frame never SUSPENDS (which would close the window here and re-run
/// the thunk on the replay, as a nested `%try` would): the thunk runs in a nested driver, and
/// a nested receive blocks instead of capturing (`capture_top_level`).
pub(super) fn interruptible(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let ceiling = interrupt_ceiling(heap, arg(args, 0))?;
    let thunk = arg(args, 1);
    let window = crate::process::open_interrupt_window(ceiling);
    let outcome = crate::eval::compile::apply_engine(heap, thunk, &[], env);
    let ours = matches!(&outcome, Err(e) if !e.is_control()
        && e.kind == crate::error::ErrorKind::Interrupted)
        && crate::process::interrupt_raised_for(window);
    crate::process::close_interrupt_window(window);
    match outcome {
        Ok(value) => Ok(heap.alloc_vector2(value::kw("ok"), value)),
        Err(e) if ours => {
            let caught = e.caught_value(heap);
            Ok(heap.alloc_vector2(value::kw("interrupted"), caught))
        }
        Err(e) => Err(e),
    }
}

/// `(proc/interrupt pid)` — interrupt a running local process in its window (ADR-491).
pub(super) fn interrupt_proc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            Ok(Value::boolean(crate::process::interrupt_process(id)))
        }
        Value::Pid { .. } => Ok(Value::boolean(false)),
        other => Err(LispError::wrong_type(heap, "proc/interrupt", "pid", other)),
    }
}

/// `(proc/busy? pid)` — is a local process past its busy-ms in one interrupt window (ADR-491)?
/// `(proc/gc-us)` — the calling process's cumulative GC pause time, µs.
pub(super) fn gc_us(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(Value::int((heap.gc_pause_ns().0 / 1_000) as i64))
}

pub(super) fn busy_proc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            Ok(crate::process::interrupt_busy(id).map_or(Value::nil(), Value::boolean))
        }
        Value::Pid { .. } => Ok(Value::nil()),
        other => Err(LispError::wrong_type(heap, "proc/busy?", "pid", other)),
    }
}

/// `(proc/set-quit-key key busy-ms [from])` — the message that interrupts this process
/// (ADR-491), and the client pid it may come from when it comes from another node.
pub(super) fn set_quit_key(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crate::process::interrupt::QuitPattern;
    let busy_ms = optional_ms(heap, "proc/set-quit-key", arg(args, 1))?.unwrap_or(0);
    let key = match arg(args, 0) {
        Value::Nil => None,
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let atoms = items
                .into_iter()
                .map(|item| quit_atom(heap, item))
                .collect::<Result<Vec<_>, _>>()?;
            Some(QuitPattern::Prefix(atoms))
        }
        atom => Some(QuitPattern::Atom(quit_atom(heap, atom)?)),
    };
    let from = match arg(args, 2) {
        Value::Nil => None,
        Value::Pid { node, .. } => Some(node),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "proc/set-quit-key",
                "the client pid a key may come from, or nil",
                other,
            ));
        }
    };
    crate::process::set_quit_key(key.map(|key| (key, busy_ms)), from);
    Ok(Value::nil())
}

/// One atom of a quit key: a keyword, string or integer.
fn quit_atom(heap: &Heap, value: Value) -> Result<crate::process::interrupt::QuitAtom, LispError> {
    use crate::process::interrupt::QuitAtom;
    match value {
        Value::Keyword(name) => Ok(QuitAtom::Keyword(name)),
        Value::Int(n) => Ok(QuitAtom::Int(n)),
        Value::Str(_) => Ok(QuitAtom::Str(
            super::args::expect_string_ref(heap, "proc/set-quit-key", value)?.to_string(),
        )),
        other => Err(LispError::type_err(format!(
            "proc/set-quit-key: expected a keyword, string, integer, a vector of them, or nil, \
             got {}",
            crate::syntax::printer::print(heap, other)
        ))),
    }
}

/// `(exit pid reason)` — send an exit signal to a local green process.
/// `reason = :kill` is the untrappable hard kill (dies at its next
/// reduction tick, or now if parked); any other reason is the soft signal (dies at
/// its next `receive`). Returns nil. A no-op for a dead/unknown pid.
pub(super) fn exit_proc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let reason = crate::process::to_message(heap, arg(args, 1))?;
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::process::exit(id, reason);
            Ok(Value::nil())
        }
        // Cross-node exit (ADR-077): ship a non-link `Frame::Exit` routed to the
        // peer's `scheduler::exit` (kill-style, like the local path).
        Value::Pid { node, id } => {
            crate::dist::exit_remote(node, id, reason);
            Ok(Value::nil())
        }
        _ => Err(LispError::type_err("exit: first argument must be a pid")),
    }
}

/// `(link pid)` — symmetrically link the current process and `pid`, local or
/// remote (ADR-077). A cross-node link ships a `Frame::Link`; either side's death
/// reaches the other, and a net-split fires `:noconnection`. Returns nil.
pub(super) fn link_proc(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::process::link_self(id);
            Ok(Value::nil())
        }
        Value::Pid { node, id } => {
            crate::dist::link_remote(node, id, crate::process::self_pid());
            Ok(Value::nil())
        }
        _ => Err(LispError::type_err("link: argument must be a pid")),
    }
}

/// `(unlink pid)` — drop the link between the current process and `pid` (local or
/// remote).
pub(super) fn unlink_proc(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::process::unlink_self(id);
            Ok(Value::nil())
        }
        Value::Pid { node, id } => {
            crate::dist::unlink_remote(node, id, crate::process::self_pid());
            Ok(Value::nil())
        }
        _ => Err(LispError::type_err("unlink: argument must be a pid")),
    }
}

/// `(process-flag flag [value])` — read or set a per-process runtime flag on the
/// **current** process, returning the
/// previous (read: current) value. Flags:
///
/// - `:max-heap` — this process's heap limit in bytes.
///   With a positive int: set it; with `nil`: clear it; with no value: read it.
///   Checked after each collection against the *live* (post-GC) footprint; when
///   exceeded, the next safepoint raises a catchable `E0045` error in this
///   process only — uncaught, it kills just the offender, unlike the global
///   ADR-043 hard cap (whole-OS-process abort). Policy lives in Brood: a spawn
///   wrapper that sets the limit first is `(spawn (fn () (process-flag
///   :max-heap n) (work)))`.
/// `(hibernate)` — tell the runtime this process is going idle for a long time, so it
/// should give back everything it can: collect, shrink its heap slabs and root vectors,
/// and drop its inline caches and compiled-body cache. It takes no continuation
/// argument: Brood processes park in `receive`, so there is no need to re-enter via an
/// explicit function.
///
/// Returns the bytes of slab capacity released.
pub(super) fn hibernate_proc(_args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(Value::Int(heap.hibernate() as i64))
}

pub(super) fn process_flag(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let flag = expect_keyword(heap, "proc/flag", arg(args, 0))?;
    match value::symbol_name_ref(flag) {
        "max-heap" => {
            let prev = if args.len() < 2 {
                heap.proc_mem_limit()
            } else {
                match arg(args, 1) {
                    Value::Int(n) if n > 0 => heap.set_proc_mem_limit(Some(n as usize)),
                    Value::Nil => heap.set_proc_mem_limit(None),
                    other => {
                        return Err(LispError::wrong_type(
                            heap,
                            "proc/flag :max-heap",
                            "positive int (bytes) or nil",
                            other,
                        ));
                    }
                }
            };
            Ok(prev.map(|n| Value::int(n as i64)).unwrap_or(Value::nil()))
        }
        "max-mailbox" => {
            let pid = crate::process::current_pid()
                .ok_or_else(|| LispError::runtime("proc/flag :max-mailbox: no calling process"))?;
            let prev = if args.len() < 2 {
                crate::process::max_mailbox(pid)
            } else {
                match arg(args, 1) {
                    Value::Int(n) if n > 0 => {
                        crate::process::set_max_mailbox(pid, Some(n as usize))
                    }
                    Value::Nil => crate::process::set_max_mailbox(pid, None),
                    other => {
                        return Err(LispError::wrong_type(
                            heap,
                            "proc/flag :max-mailbox",
                            "positive int (messages) or nil",
                            other,
                        ));
                    }
                }
            };
            Ok(prev.map(|n| Value::int(n as i64)).unwrap_or(Value::nil()))
        }
        "send-errors" => {
            let prev = if args.len() < 2 {
                heap.proc_send_errors()
            } else {
                let on = !matches!(arg(args, 1), Value::Nil | Value::Bool(false));
                heap.set_proc_send_errors(on)
            };
            Ok(Value::boolean(prev))
        }
        "no-break" => {
            let prev = if args.len() < 2 {
                heap.proc_no_break()
            } else {
                let on = !matches!(arg(args, 1), Value::Nil | Value::Bool(false));
                heap.set_proc_no_break(on)
            };
            Ok(Value::boolean(prev))
        }
        "responsive" => {
            use crate::process::responsive::Responsive;
            let prev = if args.len() < 2 {
                heap.proc_responsive()
            } else {
                let mode = match arg(args, 1) {
                    Value::Nil | Value::Bool(false) => Responsive::Off,
                    v @ Value::Keyword(_)
                        if expect_keyword(heap, "proc/flag :responsive", v)
                            .is_ok_and(|k| value::symbol_name_ref(k) == "refuse") =>
                    {
                        Responsive::Refuse
                    }
                    _ => Responsive::Report,
                };
                heap.set_proc_responsive(mode)
            };
            Ok(match prev {
                Responsive::Off => Value::nil(),
                Responsive::Report => value::kw("report"),
                Responsive::Refuse => value::kw("refuse"),
            })
        }
        other => Err(LispError::runtime(format!(
            "proc/flag: unknown flag :{other} (known: :max-heap, :max-mailbox, :send-errors, :no-break, :responsive)"
        ))),
    }
}

/// `(proc/trap-exit on)` — set the current process's `trap_exit` flag; return the
/// previous value. Only `nil`/`false` are falsy (the language truthiness rule).
pub(super) fn trap_exit_proc(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let on = !matches!(arg(args, 0), Value::Nil | Value::Bool(false));
    let prev = crate::process::set_trap_exit(crate::process::self_pid(), on);
    Ok(Value::boolean(prev))
}

/// `(monitor pid)` — watch `pid`; returns a monitor `ref`. The caller receives
/// `[:down <ref> <pid> <reason>]` when `pid` dies (immediately, reason `:noproc`,
/// if it is already dead).
pub(super) fn monitor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            // Local pid: in-process registration, returns a fresh mref.
            Ok(crate::process::monitor(id))
        }
        Value::Pid { node, id } => {
            // Remote pid: same shape — mint a mref, register *here* (so
            // demonitor can find it later, and net-split can fire
            // `:noconnection`), and ship a `Frame::Monitor` to the peer
            // which routes through the same `process::add_monitor` on the
            // far side.
            let mref = crate::process::next_ref();
            let watcher = crate::process::self_pid();
            crate::dist::monitor_remote(node, id, watcher, mref);
            Ok(Value::ref_(mref))
        }
        // `{:name n :node node}` address: resolve to a pid via `proc/whereis` and
        // monitor that pid; on another node the peer resolves it on arrival
        // (`dist::monitor_remote_name`). Either way an unregistered name fires `:noproc`.
        Value::Map(mid) => {
            // `read_name_address` is shared with `send` and hardcodes a `send:` prefix
            // in its two shape errors, so `(monitor {:oops 1})` reported "send: name
            // address needs …" — naming an operation the caller never invoked. Retag
            // the prefix to this operation. (The clean fix is an `op: &str` parameter on
            // `read_name_address` in `process/mailbox.rs`, passed "send"/"monitor" by
            // its two call sites; this retag is the caller-side equivalent.)
            let (name, node) = crate::process::read_name_address(heap, mid).map_err(|mut e| {
                if let Some(rest) = e.message.strip_prefix("send: ") {
                    e.message = format!("monitor: {rest}");
                }
                e
            })?;
            if crate::dist::is_local(node) {
                match crate::dist::whereis(name) {
                    Some(pid) => Ok(crate::process::monitor(pid)),
                    // Unregistered name: behave as if the pid were already
                    // dead — fire :noproc immediately. `process::monitor`
                    // already does this for an unknown local pid, so route
                    // through it with a fresh-but-dead id placeholder.
                    None => Ok(crate::process::monitor(u64::MAX)),
                }
            } else {
                let mref = crate::process::next_ref();
                crate::dist::monitor_remote_name(node, name, crate::process::self_pid(), mref);
                Ok(Value::ref_(mref))
            }
        }
        _ => Err(LispError::type_err(
            "monitor: first argument must be a pid or a {:name :node} address",
        )),
    }
}

/// `(demonitor mref)` — drop the monitor created by `(monitor …)`. Tries the
/// local table first; if the mref isn't there it must have been on a remote
/// peer, so a `Frame::Demonitor` is fanned out to every connected peer that
/// holds a pending remote monitor with this watcher + mref.
pub(super) fn demonitor(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Ref(n) => {
            // Local first (in-process MONITORS table).
            crate::process::demonitor(n);
            // Then ask any peer holding this mref to drop their watcher.
            // We scan PENDING_REMOTE for matching entries and `Demonitor` each
            // unique peer once. The same `process::drop_monitor` predicate the
            // local demonitor used is reused on the far side via the frame
            // handler.
            crate::process::demonitor_remote_fanout(n);
            Ok(Value::nil())
        }
        _ => Err(LispError::type_err(
            "demonitor: argument must be a monitor ref",
        )),
    }
}

/// `(%receive matcher timeout tags)` — the selective-receive primitive the `receive`
/// macro (`std/prelude.blsp`) expands to. See `crate::process::receive_match`.
/// `(%receive-spec-key patterns)` — the keyword naming a `receive`'s native pattern spec
/// (ADR-409), for the macro at expansion time, or nil for patterns the kernel cannot
/// interpret. See `process/receive_spec.rs`.
pub(super) fn receive_spec_key(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(crate::process::receive_spec_key(heap, arg(args, 0)))
}

pub(super) fn receive_match(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::receive_match(heap, arg(args, 0), arg(args, 1), arg(args, 2), arg(args, 3))
}

pub(super) fn self_pid(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(crate::process::pid_value(crate::process::self_pid()))
}

/// `(ref)` — a fresh, globally-unique reference token. Shares the runtime's ref
/// counter with `(monitor …)` so every ref is distinct.
pub(super) fn make_ref(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = crate::process::next_ref();
    // Stamp the receive-mark (ADR-195): a `receive` pinned on this ref can skip every
    // message already in our mailbox, since none of them can carry a ref that did not
    // exist when they were enqueued. One mailbox-lock acquisition (`next_seq` lives under
    // it — see `self_mailbox_seq` for why there is no lock-free copy), no ctx clone.
    heap.set_recv_mark(id, crate::process::self_mailbox_seq());
    Ok(Value::ref_(id))
}

/// `(%ref-deactivate r)` — deactivate `r` as a **one-shot reply alias**: from now on a
/// message addressed to it (`[:tag r …]`) is dropped at delivery into this process's
/// mailbox instead of queueing there forever.
///
/// The kernel half of process reply aliases. `gen/call-timeout` calls it on the
/// deadline path, so a reply the server posts *after* the caller gave up is discarded
/// by the runtime rather than accumulating in a mailbox nothing can ever match it out
/// of (unbounded growth, plus a re-scan on every later selective receive). Only the
/// process that owns the mailbox can deactivate a ref on it. Returns nil.
pub(super) fn ref_deactivate(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Ref(id) => {
            crate::process::deactivate_alias(id);
            Ok(Value::nil())
        }
        other => Err(LispError::wrong_type(heap, "%ref-deactivate", "ref", other)),
    }
}

/// `(proc/register name pid)` — bind a local name so peers can address this process by
/// `{:name name :node this-node}` before they hold its pid. Returns the pid.
///
/// Replaces any previous holder. A pid that is no longer alive binds nothing (see
/// `dist::Registration::DeadPid` for why that is not an error).
pub(super) fn register_name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "proc/register", arg(args, 0))?;
    match arg(args, 1) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            match crate::dist::register(name, id) {
                crate::dist::Registration::Bound | crate::dist::Registration::DeadPid => {
                    Ok(Value::pid(node, id))
                }
            }
        }
        Value::Pid { .. } => Err(LispError::type_err(
            "proc/register: can only register a local pid",
        )),
        _ => Err(LispError::type_err(
            "proc/register: second argument must be a pid",
        )),
    }
}

/// `(proc/unregister name)` — release a registered name; true if one was bound.
pub(super) fn unregister_name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "proc/unregister", arg(args, 0))?;
    Ok(Value::boolean(crate::dist::unregister(name)))
}

/// `(proc/alive? pid)` — is `pid` a live process on THIS node?
///
/// `(proc/info pid)` already answered this by returning nil, but it assembles a whole
/// snapshot map — several lock acquisitions and an allocation — to produce a boolean.
/// A remote pid is `false`: liveness is not knowable locally, and claiming otherwise
/// would be a guess.
pub(super) fn process_alive(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            Ok(Value::boolean(crate::process::is_alive(id)))
        }
        Value::Pid { .. } => Ok(Value::boolean(false)),
        other => Err(LispError::wrong_type(heap, "proc/alive?", "pid", other)),
    }
}

/// `(proc/whereis name)` — the **local** pid registered under `name`, or `nil`.
/// Lets idempotent bootstrap shapes test for "is this server already running
/// here?" before re-`spawn`ing — see `remote-spawn` in `std/prelude.blsp`.
/// A remote-side registration isn't visible here; this is a strictly local
/// lookup over the `NAMES` table.
pub(super) fn whereis_name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name(heap, "proc/whereis", arg(args, 0))?;
    match crate::dist::whereis(name) {
        Some(id) => Ok(Value::pid(crate::dist::local_node(), id)),
        None => Ok(Value::nil()),
    }
}

/// `(%spawn-count)` — how many green processes have been spawned since the program
/// started. (Green processes are cheap coroutines, not OS threads — step 4b.)
pub(super) fn spawn_count(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::process::spawn_count() as i64))
}

/// `(%peak-threads)` — high-water mark of processes running *simultaneously*
/// (bounded by the worker-pool size); how much parallelism was actually reached.
pub(super) fn peak_threads(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::process::peak_threads() as i64))
}

/// `(%worker-threads)` — size of the scheduler's worker-thread pool that runs the
/// green processes (≈ `nproc`, or the `-j` setting); 0 until the first spawn.
pub(super) fn worker_threads(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::process::worker_threads() as i64))
}

/// `(%sched-stats)` — one snapshot map of the scheduler's cumulative counters
/// (the scheduler half of the observability timing tier): `:spawned`/`:exited`
/// totals (their difference is the live-process figure), `:preempts` (quantum
/// exhaustions), `:steals` + `:migrations` (work-stealing activity),
/// `:workers` and `:peak-threads` (pool size / high-water parallelism).
pub(super) fn sched_stats(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pairs = vec![
        (
            value::kw("spawned"),
            Value::int(crate::process::spawn_count() as i64),
        ),
        (
            value::kw("exited"),
            Value::int(crate::process::exit_count() as i64),
        ),
        (
            value::kw("preempts"),
            Value::int(crate::process::preempt_count() as i64),
        ),
        (
            value::kw("steals"),
            Value::int(crate::process::steal_count() as i64),
        ),
        (
            value::kw("migrations"),
            Value::int(crate::process::migrate_count() as i64),
        ),
        (
            value::kw("workers"),
            Value::int(crate::process::worker_threads() as i64),
        ),
        (
            value::kw("peak-threads"),
            Value::int(crate::process::peak_threads() as i64),
        ),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// The `(proc/system-monitor)` return shape: one subscription as a map, or nil.
fn sysmon_config_map(heap: &mut Heap, m: Option<crate::process::sysmon::SysMon>) -> Value {
    match m {
        None => Value::nil(),
        Some(m) => {
            let pairs = vec![
                (value::kw("pid"), crate::process::pid_value(m.pid)),
                (value::kw("gc"), Value::boolean(m.gc)),
                (
                    value::kw("gc-min-pause-us"),
                    Value::int(m.gc_min_pause_us as i64),
                ),
                (value::kw("spawn"), Value::boolean(m.spawn)),
                (value::kw("exit"), Value::boolean(m.exit)),
                (value::kw("exit-abnormal"), Value::boolean(m.exit_abnormal)),
                (value::kw("deopt"), Value::boolean(m.deopt)),
                (
                    value::kw("long-schedule-us"),
                    Value::int(m.long_schedule_us as i64),
                ),
            ];
            heap.map_from_pairs(pairs)
        }
    }
}

/// The calling process's pid — the subscriber the no-pid forms of
/// `proc/system-monitor` act on. The root context (a `brood` invocation before
/// `run_program`, an embedding host) has one too; only a bare `Interp` outside
/// any scheduler context has none, and that context cannot receive messages
/// anyway.
fn sysmon_caller() -> Result<u64, LispError> {
    crate::process::current_pid().ok_or_else(|| {
        LispError::runtime(
            "proc/system-monitor: no calling process — arm or clear a named pid instead",
        )
    })
}

/// `(proc/system-monitor [pid opts])` — read, arm, or clear a subscription to the
/// kernel **system monitor**: runtime events (`:gc`/`:spawn`/`:exit`/
/// `:exit-abnormal`/`:deopt`) delivered to a subscriber process as
/// `[:system kind subject-pid detail]` messages (see `process/sysmon.rs`). One subscription per subscriber pid (ADR-305). No args
/// reads the **caller's** subscription; `:all` lists every subscription; `nil`
/// clears the caller's and `(nil pid)` clears `pid`'s; a local pid arms it — with
/// no opts map every event is selected, with one exactly the truthy keys are.
/// Arming/clearing returns that pid's *previous* config (map or nil), so callers
/// can save/restore.
pub(super) fn system_monitor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crate::process::sysmon::{self, SysMon};
    if args.is_empty() {
        let me = sysmon_caller()?;
        return Ok(sysmon_config_map(heap, sysmon::current_for(me)));
    }
    let prev = match arg(args, 0) {
        Value::Keyword(k) if k == value::intern("all") => {
            let maps = sysmon::all()
                .into_iter()
                .map(|m| sysmon_config_map(heap, Some(m)))
                .collect::<Vec<_>>();
            return Ok(heap.alloc_vector(maps));
        }
        Value::Nil => {
            let target = if args.len() > 1 {
                match arg(args, 1) {
                    Value::Pid { node, id } if crate::dist::is_local(node) => id,
                    other => {
                        return Err(LispError::wrong_type(
                            heap,
                            "proc/system-monitor",
                            "local pid to clear",
                            other,
                        ));
                    }
                }
            } else {
                sysmon_caller()?
            };
            sysmon::clear(target)
        }
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            let mut m = SysMon {
                pid: id,
                gc: true,
                gc_min_pause_us: 0,
                spawn: true,
                exit: true,
                exit_abnormal: true,
                deopt: true,
                long_schedule_us: sysmon::DEFAULT_LONG_SCHEDULE_US,
            };
            if args.len() > 1 {
                match arg(args, 1) {
                    // An explicit opts map selects exactly its truthy keys.
                    Value::Map(opts) => {
                        let sel = |heap: &Heap, name: &str| {
                            heap.map_get(opts, value::kw(name))
                                .is_some_and(crate::eval::truthy)
                        };
                        m.gc = sel(heap, "gc");
                        m.spawn = sel(heap, "spawn");
                        m.exit = sel(heap, "exit");
                        m.exit_abnormal = m.exit || sel(heap, "exit-abnormal");
                        m.deopt = sel(heap, "deopt");
                        if let Some(Value::Int(n)) =
                            heap.map_get(opts, value::kw("gc-min-pause-us"))
                            && n > 0
                        {
                            m.gc_min_pause_us = n as u64;
                        }
                        // Selected by its threshold: `:long-schedule-us n`, n > 0.
                        m.long_schedule_us = match heap.map_get(opts, value::kw("long-schedule-us"))
                        {
                            Some(Value::Int(n)) if n > 0 => n as u64,
                            _ => 0,
                        };
                    }
                    Value::Nil => {}
                    other => {
                        return Err(LispError::wrong_type(
                            heap,
                            "proc/system-monitor",
                            "options map or nil",
                            other,
                        ));
                    }
                }
            }
            sysmon::install(m).map_err(|sysmon::DeadSubscriber| {
                LispError::runtime(format!(
                    "proc/system-monitor: pid {id} is not a live process — a subscription \
                     on it would never be delivered or removed"
                ))
            })?
        }
        other => {
            return Err(LispError::wrong_type(
                heap,
                "proc/system-monitor",
                "local pid, :all, or nil",
                other,
            ));
        }
    };
    Ok(sysmon_config_map(heap, prev))
}

/// `(%steal-count)` — how many fresh processes the scheduler work-stole across
/// worker threads since program start. A diagnostic of how much the pool had to
/// rebalance; 0 means placement-at-spawn kept it even.
pub(super) fn steal_count(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::process::steal_count() as i64))
}

/// `(list-processes)` — every currently-live local pid as a `Pid` value
/// (carrying this runtime's node identity, so the list is `send`-routable as
/// returned). Order is unspecified; sort by `.id` if you need stability.
/// Used by agents / the `nest mcp` `processes` tool to enumerate what's been
/// spawned in the session.
pub(super) fn list_processes(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let items: Vec<Value> = crate::process::list_local_pids()
        .into_iter()
        .map(crate::process::pid_value)
        .collect();
    Ok(heap.list(items))
}

// ----- process introspection (ADR-051) ---------------------------------------
//
// Kernel-internal per-process state an observer needs but Brood can't reach:
// `mailbox-size` and the `process-info` snapshot, assembled here from the
// registry / scheduler / name / monitor tables. `std/tool/observer.blsp` builds
// everything else on top. (The frontends live in terminal.rs and gui.rs.)

pub(super) fn mailbox_size(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            Ok(crate::process::mailbox_len(id)
                .map(|n| Value::int(n as i64))
                .unwrap_or(Value::nil()))
        }
        Value::Pid { .. } => Ok(Value::nil()),
        other => Err(LispError::wrong_type(
            heap,
            "proc/mailbox-size",
            "pid",
            other,
        )),
    }
}

/// `(%process-info pid)` — a snapshot map of a **live local** process, or `nil`
/// for a remote/dead pid (a non-pid is a type error). The fields are all
/// kernel-internal, so the map is assembled here from the registry / scheduler /
/// name / monitor tables (ADR-051):
///
///   `{:id <int> :node <kw> :name <kw|nil> :status <kw> :mailbox <int>
///     :monitored-by <int> :parent <int|nil>}`
///
/// `:status` is `:running` / `:waiting` (parked in `receive`). `:name` is the
/// registered name or nil. `:parent` is the spawner's id (nil for the root).
/// `:memory` (per-process bytes) joins once the kernel tracks it, and `:status`
/// sharpens when an explicit state enum lands (the observer tolerates the gap).
/// Each accessor takes one lock independently, so no two are held at once.
pub(super) fn process_info(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            // Dead/unknown pid → nil (matches `mailbox-size`).
            if !crate::process::is_alive(id) {
                return Ok(Value::nil());
            }
            let name = crate::dist::name_for_pid(id)
                .map(Value::Keyword)
                .unwrap_or(Value::nil());
            let status = crate::process::process_status(id)
                .map(value::kw)
                .unwrap_or(Value::nil());
            let mailbox = Value::int(crate::process::mailbox_len(id).unwrap_or(0) as i64);
            let monitored = Value::int(crate::process::monitored_by(id) as i64);
            // `:parent` is the spawner's id, or nil for the root.
            let parent = crate::process::parent_of(id)
                .map(|p| Value::int(p as i64))
                .unwrap_or(Value::nil());
            // `:memory` — the process's LOCAL heap footprint (bytes), published on
            // its last `receive`; 0 for a process that has never received.
            let memory = Value::int(crate::process::process_mem(id).unwrap_or(0) as i64);
            // `:collections` — the process's cumulative GC count, republished on
            // its last `receive` (0 for one that has never received). The signal
            // for "is this process churning memory?" in the observer.
            let collections = Value::int(crate::process::process_gc_runs(id).unwrap_or(0) as i64);
            // `:reductions` — the process's cumulative reduction count (the
            // scheduling unit), updated every scheduling quantum. The observer's
            // "is this process doing work / busy?" signal. Exact for spawned
            // processes; coarse (whole-budget increments) for the root.
            let reductions = Value::int(crate::process::process_reductions(id).unwrap_or(0) as i64);
            // `:cpu-us` — the CPU time the process has actually consumed, in µs. Set beside
            // a wall-clock measurement it says whether a slow step did a lot of work or
            // spent its time WAITING to run (`process/cputime.rs`). 0 where the platform has
            // no per-thread CPU clock. A process asking about ITSELF also counts the
            // quantum it is in, which is booked only when that quantum ends.
            // The first read arms quantum accounting (it costs ~0.3 µs a quantum, so it is
            // off until something looks — see `process/cputime.rs`).
            crate::process::arm_cpu_accounting();
            let in_flight = if id == crate::process::self_pid() {
                crate::process::quantum_cpu_so_far()
            } else {
                0
            };
            let cpu_ns = crate::process::process_cpu_ns(id).unwrap_or(0) + in_flight;
            let cpu_us = Value::int((cpu_ns / 1_000) as i64);
            let pairs = vec![
                (value::kw("id"), Value::int(id as i64)),
                // The process's actual pid value (not just its numeric id), so a
                // caller — e.g. the observer's kill command — can act on the
                // process directly with `exit`/`send`/`monitor`.
                (value::kw("pid"), Value::pid(node, id)),
                (value::kw("node"), Value::keyword(node)),
                (value::kw("name"), name),
                (value::kw("status"), status),
                (value::kw("mailbox"), mailbox),
                (value::kw("monitored-by"), monitored),
                (value::kw("parent"), parent),
                (value::kw("memory"), memory),
                (value::kw("collections"), collections),
                (value::kw("reductions"), reductions),
                (value::kw("cpu-us"), cpu_us),
            ];
            Ok(heap.map_from_pairs(pairs))
        }
        Value::Pid { .. } => Ok(Value::nil()),
        other => Err(LispError::wrong_type(heap, "proc/info", "pid", other)),
    }
}
