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
//! - [`monitor`] — Erlang-style monitors (`Watcher`, `MONITORS`,
//!   `PENDING_REMOTE`, the full `monitor`/`demonitor`/`add_monitor`/
//!   `drop_monitor`/`handle_node_down`/`fire_noconnection` surface).
//! - [`timer`] — `(after ms expr)` deadlines: the min-heap + one OS
//!   thread that calls back into the mailbox's `wake_for_timeout`.

mod io_source;
pub(crate) mod keywords;
mod links;
mod mailbox;
mod message;
mod monitor;
mod scheduler;
mod timer;

pub use mailbox::{
    list_local_pids, mailbox_len, process_gc_runs, process_mem, process_reductions, process_status,
    receive_match, send,
};
pub use message::{from_message, to_message, ClosureArmMsg, ClosureMsg, Message};
// The wire codec (`dist::wire`) defines its decode-depth cap in terms of this so
// the two can't diverge; crate-internal, hence `pub(crate)`.
pub(crate) use message::MAX_MESSAGE_DEPTH;
// The reusable blocking-IO → mailbox seam (ADR-059): any subsystem that must
// block runs it on a non-worker thread and delivers to a process mailbox.
pub(crate) use io_source::{spawn_io_source, MailboxSink, SubscriberHandle};
pub use monitor::{demonitor, monitor, monitored_by, next_ref};
// Erlang-style links (ADR-067): symmetric failure coupling + `trap_exit`.
pub use links::{link_count, link_self, set_trap_exit, unlink_self};
// Cross-node link machinery, used by `dist` (the senders + inbound handlers).
pub(crate) use links::{
    deliver_remote_link_exit, drop_remote_link, handle_node_down as handle_link_node_down,
    record_remote_link,
};
pub use scheduler::{
    begin_capture, capture_append, deadline_exceeded, exit, gc_block_depth, in_green_process,
    macro_block_active, parent_of, peak_threads, pid_value, self_pid, set_deadline,
    migrate_count, set_max_parallel, spawn, spawn_count, stack_budget, stack_overflow_check,
    steal_count, take_capture, tick, worker_threads, yield_now, GcBlockGuard, MacroBlockGuard,
    WORKER_STACK_BYTES,
};
// State-capture driver helpers (ADR-100 §8): read by the bytecode VM driver to decide
// when to capture a continuation (vs. yield the coroutine / block the root), and by the
// `receive` gate to tell a capturable top-level receive from a native-nested one.
pub(crate) use scheduler::{
    capture_hard_kill_pending, capture_top_level, dirty_block, in_capture_run,
    set_capture_top_level, tick_capture,
};
// Test-only: the JIT preempt unit test (`compile.rs`) drives a tiered arm as if it
// were a capture-mode green process. Non-test callers reach it via `scheduler::` or
// the local fn directly, so the re-export is test-gated to avoid an unused warning.
#[cfg(test)]
pub(crate) use scheduler::set_capture_run;

pub(crate) use mailbox::{deliver, is_alive, read_name_address};
pub(crate) use monitor::{
    add_monitor, demonitor_remote_fanout, drop_monitor, drop_pending_remote, fire_noconnection,
    handle_node_down, record_pending_remote, Watcher,
};
