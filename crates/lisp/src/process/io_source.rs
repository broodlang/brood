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
        // Acquire pairs with `retarget`'s Release store so a source thread always sees
        // the latest subscriber after a `tcp-controlling-process` handoff.
        deliver(self.subscriber.load(Ordering::Acquire), msg);
    }
}

/// A connected (sink, retarget-cell) pair over a fresh subscriber cell, without
/// spawning a thread. Reactor-style sources use this (the net reactor
/// multiplexes many sockets on one thread): they own the sink and hand the
/// cell to the control plane, whose `tcp-controlling-process` retargets by
/// storing a new pid into it (Release, pairing with `emit`'s Acquire).
pub fn sink_pair(subscriber: u64) -> (MailboxSink, Arc<AtomicU64>) {
    let cell = Arc::new(AtomicU64::new(subscriber));
    (
        MailboxSink {
            subscriber: cell.clone(),
        },
        cell,
    )
}

/// Run `body` on a fresh non-worker OS thread named `name`; it reads some blocking
/// resource and `emit`s messages to `subscriber`'s mailbox until it returns. The
/// spawned thread owns whatever it blocks on.
///
/// This is the thread-per-resource shape of the ADR-059 pattern (subprocess
/// pipes); sockets ride the net reactor's single thread instead (ADR-143) via
/// [`sink_pair`].
pub fn spawn_io_source<F>(subscriber: u64, name: &str, body: F)
where
    F: FnOnce(&MailboxSink) + Send + 'static,
{
    let (sink, _cell) = sink_pair(subscriber);
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || body(&sink))
        .expect("spawn blocking-io source thread");
}
