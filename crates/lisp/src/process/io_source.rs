//! The core **blocking-work-delivers-to-a-mailbox** mechanism (ADR-059).
//!
//! The green scheduler has a small worker pool; a process that makes a native
//! blocking call (a socket `read`, a device wait, a synchronous FFI call) would
//! pin its worker for the whole call, starving the pool. The rule (ADR-059):
//! anything that blocks runs on a **non-worker thread** and **delivers a message
//! to the owning process's mailbox**; the process parks in `(receive)` holding no
//! worker until woken.
//!
//! This module is the one reusable seam for that pattern. A blocking source
//! (`crate::net` sockets today; `gui`/`dist`/terminal input are slated to migrate
//! onto it) calls [`spawn_io_source`] with the subscriber process and a body that
//! reads its resource and `emit`s [`Message`]s. `Message` is a plain enum and
//! symbols are a global interner, so the body builds messages off-heap without
//! touching any process's `Heap`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::mailbox::deliver;
use super::message::Message;

/// Where a blocking source emits messages: one process's mailbox. The subscriber
/// is held in a shared atomic so it can be **retargeted** at runtime (see
/// [`SubscriberHandle`] — the socket "controlling process" handoff). Each
/// [`emit`] injects a [`Message`] into the current subscriber and wakes it (a
/// no-op if it has exited).
///
/// [`emit`]: MailboxSink::emit
#[derive(Clone)]
pub struct MailboxSink {
    subscriber: Arc<AtomicU64>,
}

impl MailboxSink {
    /// Deliver `msg` to the current subscriber's mailbox (and wake it).
    pub fn emit(&self, msg: Message) {
        deliver(self.subscriber.load(Ordering::Relaxed), msg);
    }
}

/// A handle to retarget a running source's subscriber — e.g. to hand an accepted
/// socket from the acceptor to a freshly `spawn`ed per-connection process. Cheap
/// to keep; the source reads the current value on every `emit`.
pub struct SubscriberHandle {
    subscriber: Arc<AtomicU64>,
}

impl SubscriberHandle {
    /// Redirect all future deliveries to process `pid`.
    pub fn retarget(&self, pid: u64) {
        self.subscriber.store(pid, Ordering::Relaxed);
    }
}

/// Run `body` on a fresh non-worker OS thread named `name`; it reads some blocking
/// resource and `emit`s messages to `subscriber`'s mailbox until it returns. The
/// caller returns immediately with a [`SubscriberHandle`] it can use to retarget
/// delivery later. The spawned thread owns whatever it blocks on.
///
/// This is the single place the thread-plus-`deliver` pattern lives — see ADR-059
/// and `docs/handoff-blocking-io.md`.
pub fn spawn_io_source<F>(subscriber: u64, name: &str, body: F) -> SubscriberHandle
where
    F: FnOnce(&MailboxSink) + Send + 'static,
{
    let cell = Arc::new(AtomicU64::new(subscriber));
    let sink = MailboxSink {
        subscriber: cell.clone(),
    };
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || body(&sink))
        .expect("spawn blocking-io source thread");
    SubscriberHandle { subscriber: cell }
}
