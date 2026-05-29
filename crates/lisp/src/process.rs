//! Processes: share-nothing green processes communicating by message passing
//! (`spawn`/`send`/`receive`/`self`).
//!
//! **Step 4b** (see `docs/scheduler.md`, ADR-018): processes are lightweight
//! *green* threads, not OS threads. Each runs inside a [`corosensei`] stackful
//! coroutine — its own parkable stack — so the native recursive evaluator runs
//! unchanged and `receive` on an empty mailbox **suspends** the coroutine instead
//! of blocking a thread. A small pool of worker OS threads (≈ `nproc`, a setting)
//! runs ready processes off a shared run queue; `send` wakes a parked process.
//!
//! **Code is shared, data is not.** A spawned process shares the runtime's code +
//! global table (the `Arc`s in its `Heap`), so a `def` reaches it; but its data
//! lives in its own LOCAL heap, so messages cross as a self-contained, `Send`
//! [`Message`] (a deep copy), rebuilt into the receiver's heap. Symbols travel as
//! their global interned id (the interner is process-wide).
//!
//! The thread that started the program (the REPL / file runner) is a *root*
//! process: it is not a coroutine, so its `receive` **blocks** on its mailbox
//! rather than yielding. Everything `spawn`ed is a green process that yields.
//!
//! ## Module map
//!
//! - [`message`] — `Message`/`ClosureMsg` types + `to_message`/`from_message`
//!   (the deep-copy machinery that moves a `Value` between heaps).
//! - [`mailbox`] — `Mailbox`, `REGISTRY`, `deliver`, `send`, `receive_match`,
//!   `wait_for_message`, `wake_for_timeout`, `list_local_pids`.
//! - [`scheduler`] — coroutine plumbing (`Process`, `Ctx`, `Suspend`), the
//!   run queue + worker pool, `spawn`, `tick`/`preempt`, `GcBlockGuard`,
//!   `self_pid`, `pid_value`, `deregister`.
//! - [`monitor`] — Erlang-style monitors (`Watcher`, `MONITORS`,
//!   `PENDING_REMOTE`, the full `monitor`/`demonitor`/`add_monitor`/
//!   `drop_monitor`/`handle_node_down`/`fire_noconnection` surface).
//! - [`timer`] — `(after ms expr)` deadlines: the min-heap + one OS
//!   thread that calls back into the mailbox's `wake_for_timeout`.

mod mailbox;
mod message;
mod monitor;
mod scheduler;
mod timer;

pub use mailbox::{
    list_local_pids, mailbox_len, process_gc_runs, process_mem, process_status, receive_match, send,
};
pub use message::{from_message, to_message, ClosureArmMsg, ClosureMsg, Message};
pub use monitor::{demonitor, monitor, monitored_by, next_ref};
pub use scheduler::{
    gc_block_depth, in_green_process, parent_of, peak_threads, pid_value, self_pid,
    set_max_parallel, spawn, spawn_count, stack_budget, stack_overflow_check, tick, worker_threads,
    GcBlockGuard, GcBlockReset, CORO_STACK_BYTES,
};

pub(crate) use mailbox::{deliver, is_alive, read_name_address};
pub(crate) use monitor::{
    add_monitor, demonitor_remote_fanout, drop_monitor, drop_pending_remote, fire_noconnection,
    handle_node_down, record_pending_remote, Watcher,
};
