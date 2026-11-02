//! Processes: share-nothing green processes communicating by message passing
//! (`spawn`/`send`/`receive`/`self`).
//!
//! Processes are lightweight *green* threads, not OS threads. Each runs its 0-arg
//! body's bytecode directly on a worker thread (ADR-100 §8.4 — state capture,
//! corosensei removed); `receive` on an empty mailbox **captures** the process's
//! continuation as relocatable heap data (`Suspended`) instead of blocking the
//! thread. A small pool of worker OS threads (≈ `nproc`, a setting) runs ready
//! processes off per-worker run queues with work-stealing; `send` wakes a parked
//! process — which, carrying no native stack, may resume on any worker (live
//! migration, §7).
//!
//! **Code is shared, data is not.** A spawned process shares the runtime's code +
//! global table (the `Arc`s in its `Heap`), so a `def` reaches it; but its data
//! lives in its own LOCAL heap, so messages cross as a self-contained, `Send`
//! [`Message`] (a deep copy), rebuilt into the receiver's heap. Symbols travel as
//! their global interned id (the interner is process-wide).
//!
//! The thread that started the program (the REPL / file runner) is a *root*
//! process: it never enters the scheduler, so its `receive` **blocks** on its
//! mailbox rather than capturing. Everything `spawn`ed is a green process.
//!
//! ## Module map
//!
//! - [`message`] — `Message`/`ClosureMsg` types + `to_message`/`from_message`
//!   (the deep-copy machinery that moves a `Value` between heaps).
//! - [`mailbox`] — `Mailbox`, `REGISTRY`, `deliver`, `send`, `receive_match`,
//!   `wait_for_message`, `wake_for_timeout`, `list_local_pids`.
//! - [`scheduler`] — the state-capture driver (`Process`, `Ctx`), the run queue +
//!   worker pool, `spawn`, `tick`/`preempt`, `GcBlockGuard`, `self_pid`,
//!   `pid_value`, `deregister`.
//! - [`monitor`] — process monitors (`Watcher`, `MONITORS`,
//!   `PENDING_REMOTE`, the full `monitor`/`demonitor`/`add_monitor`/
//!   `drop_monitor`/`handle_node_down`/`fire_noconnection` surface).
//! - [`timer`] — `(after ms expr)` deadlines: the min-heap + one OS
//!   thread that calls back into the mailbox's `wake_for_timeout`.
//!
//! ## Lock order
//!
//! The process-wide locks, outer before inner. Each row is a nesting the code takes; a
//! nesting not listed here is a new order, and belongs in this table before it lands.
//!
//! | outer | inner | where |
//! |---|---|---|
//! | `dist::NAMES` | `REGISTRY` shard | `spawn_or_get`, `register`, `unregister_dead_pid_while` |
//! | `monitor::MONITORS` | `REGISTRY` shard | `add_monitor`'s liveness check |
//! | `monitor::MONITORS` | mailbox `state`, run queue | the death path's local `[:down]` pushes (KI-213) |
//! | `links::LINKS` | `REGISTRY` shard | `link`'s liveness check |
//! | `links::LINKS` | `REGISTRY` shard, mailbox `state`, run queue | `notify_peers`' local `[:EXIT]`s (KI-262) |
//! | `monitor::PENDING_REMOTE` | mailbox `state`, run queue | a remote monitor's `[:down]`, delivered by whoever removed its entry |
//! | `timer::TIMERS` | `REGISTRY` shard | `compact` |
//! | `dist::NODE_MONITORS` (write) | `dist::NODES` (read) | `monitor_node` |
//! | `dist::NODES` | `dist::DIAL_ROUTES` | `linked_peer_for` |
//! | net `SocketOwner` | mailbox `state` | socket delivery |
//! | mailbox `state` | subprocess `OutputBudget` | an output refund |
//!
//! Two rules follow, and every row keeps them:
//!
//! - **A `REGISTRY` shard is innermost.** Never hold one while taking `NAMES`, `MONITORS`,
//!   `LINKS`, `TIMERS` or the sysmon table, and never hold two shards at once.
//!   `deregister` takes its tables one after another, never nested, for this reason.
//! - **Delivery takes only the target's mailbox `state` and a run queue**, so a table may
//!   deliver while held (`MONITORS`, `LINKS`, `PENDING_REMOTE`) as long as nothing takes a
//!   mailbox lock and then that table. Holding it across the delivery is what makes
//!   "removed from the table" and "message queued" one step to a concurrent `demonitor` or
//!   `unlink`. `REMOTE_LINKS` is taken alone, and no wire send is made with a table held.
//!
//! The code region keeps its own order, documented where its locks live: `registry_lock`
//! → the globals lock, the load journal or `promote_lock` (`core/heap/env_globals.rs`,
//! `runtime_code.rs`), and `promote_lock` → the published table's lock (`promote.rs`).

pub(crate) mod cputime;
pub(crate) mod interrupt;
mod io_source;
pub(crate) mod keywords;
mod links;
mod mailbox;
/// Diagnostic counters for the L1 local-send fast path (`BROOD_L1_STATS=1`).
pub use mailbox::l1_stats;
pub(crate) mod message;
pub(crate) mod monitor;
mod pressure;
mod receive_spec;
pub(crate) use pressure::{near_soft_limit, soft_limit_crossed};
pub(crate) use receive_spec::spec_key as receive_spec_key;
mod scheduler;
/// The system monitor (kernel runtime events → one subscriber's mailbox).
/// `pub` so the GC (`core/heap.rs`) and VM (`eval/compile`) emit sites and the
/// `(proc/system-monitor …)` builtin reach it without per-fn re-exports.
pub mod sysmon;
mod timer;
/// A `receive` deadline on the scheduler clock — carried through the VM's suspend path
/// (`Control::Suspend`). Opaque outside this module: nothing else can mint or read one.
pub use timer::SchedDeadline;
/// Pending `(after ms …)` deadline entries, runtime-wide — the leak/compaction test hook
/// for `timer`'s lazy cancellation.
pub use timer::pending_timer_count;

// A window's busy cursor, and `proc/busy?` for one across a link (ADR-491).
pub(crate) use mailbox::interrupt_busy;
pub use mailbox::{close_interrupt_window, interrupt_process, open_interrupt_window};
pub(crate) use mailbox::{
    current_interrupt_due, interrupt_caught, interrupt_raised_for, set_quit_key,
    take_current_interrupt,
};
pub use mailbox::{
    deactivate_alias, list_local_pids, mailbox_len, max_mailbox, process_cpu_ns, process_gc_runs,
    process_mem, process_reductions, process_status, receive_lane, receive_match, send,
    send_in_lane, set_max_mailbox, take_priority,
};
pub use message::{ClosureArmMsg, ClosureMsg, Message, from_message, to_message};
pub(crate) use message::{chunk_flush, chunk_payload, error_reason};
pub(crate) use scheduler::exit_reason_phrase;
pub(crate) use scheduler::take_current_mailbox_overflow;
pub(crate) use scheduler::{arm_cpu_accounting, quantum_cpu_so_far};
// The wire codec (`dist::wire`) defines its decode-depth cap in terms of this so
// the two can't diverge; crate-internal, hence `pub(crate)`.
pub(crate) use message::{MAX_MESSAGE_DEPTH, grow};
// The reusable blocking-IO → mailbox seam (ADR-059): any subsystem that must
// block runs it on a non-worker thread and delivers to a process mailbox.
pub(crate) use io_source::{IoThread, MailboxSink, reserve_io_thread, sink_pair};

/// What to do when the OS refuses a thread the runtime cannot work without.
pub(crate) const THREAD_LIMIT_HINT: &str = "the operating system refused a thread: raise the \
     thread limit (ulimit -u, a container's pids limit)";

pub use monitor::{demonitor, monitor, monitored_by, next_ref};
// Process links (ADR-067): symmetric failure coupling + `trap_exit`.
pub use links::{link_count, link_self, set_trap_exit, unlink_self};
// Cross-node link machinery, used by `dist` (the senders + inbound handlers).
pub(crate) use links::{
    deliver_remote_link_exit, drop_remote_link, handle_node_down as handle_link_node_down,
    record_remote_link,
};
pub use scheduler::{
    GcBlockGuard, MacroBlockGuard, NATIVE_STACK_MARGIN_BYTES, WORKER_STACK_BYTES, begin_capture,
    capture_append, current_pid, deregister_root_ctx, deregister_root_ctx_of,
    dirty_receive_block_count, exit, exit_count, free_drained_gen, gc_block_depth,
    in_green_process, isolate_owner_of, live_pids, macro_block_active, migrate_count,
    native_stack_headroom_ok, next_isolate_token, old_gen_drained, parent_of, peak_threads,
    pending_kill_phrase, pid_value, preempt_count, report_drain_liveness, self_isolate_scope,
    self_mailbox_seq, self_pid, set_max_parallel, set_self_isolate_scope, set_test_no_workers,
    shutdown_runtime_parked, spawn, spawn_count, spawn_linked, spawn_monitored, spawn_root_program,
    stack_budget, stack_overflow_check, steal_count, take_capture, test_drive_quanta, tick,
    with_minting_runtime_tag, worker_threads, yield_now,
};
#[cfg(target_arch = "wasm32")]
pub(crate) use scheduler::{pump_due, pump_until_quiescent};
/// Currently-parked process count, runtime-wide — teardown/leak test hook.
pub fn parked_process_count() -> usize {
    mailbox::parked_count()
}
// State-capture driver helpers (ADR-100 §8): read by the bytecode VM driver to decide
// when to capture a continuation (vs. yield the coroutine / block the root), and by the
// `receive` gate to tell a capturable top-level receive from a native-nested one.
pub(crate) use scheduler::{
    Safepoint, TreeWalkGuard, capture_hard_kill_pending, capture_kill_pending, capture_top_level,
    charge as charge_reductions, charge_native, dirty_block, driver_safepoint, expire_quantum,
    in_capture_run, set_capture_top_level, stop_between_elements, tick_nested,
};
// A native activation's poll is called only from the JIT's runtime callback (`crate::jit`),
// so its re-export is feature-gated to keep the no-`jit` build warning-free.
#[cfg(feature = "jit")]
pub(crate) use scheduler::{NativePoll, current_mailbox_overflow_pending, native_poll};
// Test-only: the JIT preempt unit test (`compile.rs`) drives a tiered arm as if it
// were a capture-mode green process. That test is itself `#[cfg(feature = "jit")]`,
// so the re-export must match — gating on bare `test` leaves it unused (and warned)
// in a no-`jit` test build. Non-test callers reach it via `scheduler::` or the local
// fn directly.
#[cfg(all(test, feature = "jit"))]
pub(crate) use scheduler::set_capture_run;
#[cfg(all(test, feature = "jit"))]
pub(crate) use scheduler::set_reduction_budget_for_test;
#[cfg(all(test, feature = "jit"))]
pub(crate) use scheduler::test_work_waiting;

pub(crate) use mailbox::{
    Lane, deliver, deliver_in_lane, deliver_priority, is_alive, read_name_address,
};
// Inbound flow control (socket backpressure): a credit rides in the envelope and is
// returned to its source when the message leaves the mailbox. Used by `host::net` and
// `host::subprocess`.
pub(crate) use mailbox::{CreditSource, DeliveryCredit, deliver_credited};
// Only a window delivers a credited message on the priority lane (its key repeats).
#[cfg(feature = "gui")]
pub(crate) use mailbox::deliver_priority_credited;
pub(crate) use monitor::{
    Watcher, add_monitor, deliver_remote_down, demonitor_remote_fanout, drop_monitor,
    drop_pending_remote, fire_noconnection_if_pending, handle_node_down, record_pending_remote,
};
