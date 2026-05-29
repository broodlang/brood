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

use super::mailbox::deliver;
use super::message::Message;

/// Where a blocking source emits messages: one process's mailbox. Each [`emit`]
/// injects a [`Message`] into the subscriber and wakes it (a no-op if the
/// subscriber has exited).
///
/// [`emit`]: MailboxSink::emit
#[derive(Clone)]
pub struct MailboxSink {
    subscriber: u64,
}

impl MailboxSink {
    /// Deliver `msg` to the subscriber's mailbox (and wake it).
    pub fn emit(&self, msg: Message) {
        deliver(self.subscriber, msg);
    }

    /// The local pid of the subscriber this sink feeds — e.g. so an accept loop
    /// can give each accepted connection the same subscriber.
    pub fn subscriber(&self) -> u64 {
        self.subscriber
    }
}

/// Run `body` on a fresh non-worker OS thread named `name`; it reads some blocking
/// resource and `emit`s messages to `subscriber`'s mailbox until it returns. The
/// caller returns immediately. The spawned thread owns whatever it blocks on.
///
/// This is the single place the thread-plus-`deliver` pattern lives — see ADR-059
/// and `docs/handoff-blocking-io.md`.
pub fn spawn_io_source<F>(subscriber: u64, name: &str, body: F)
where
    F: FnOnce(&MailboxSink) + Send + 'static,
{
    let sink = MailboxSink { subscriber };
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || body(&sink))
        .expect("spawn blocking-io source thread");
}
