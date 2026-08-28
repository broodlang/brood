//! A root context must not outlive the `Interp` that minted it.
//!
//! `ensure_ctx` caches the root `Ctx` in a thread-local keyed to the THREAD, not the
//! runtime, and nothing used to clear it — so a second `Interp` on the same thread inherited
//! the first's pid **and its mailbox**. Six sequential `Interp`s all reported
//! `#<pid nonode/1>`.
//!
//! The sharp edge is not the repeated pid. A `Payload::Local { slot, .. }` left queued in
//! that inherited mailbox is an index into the heap of the runtime that took delivery;
//! popping it after the swap reads the NEW runtime's `msg_roots` at the OLD runtime's index.
//! That is a wrong-heap read, and a silent one, because the index is in range more often
//! than not. `Interp::drop` now calls `deregister_root_ctx`.

use brood::Interp;

fn root_pid(i: &mut Interp) -> String {
    let v = i.eval_str("(self)").expect("(self)");
    i.print(v)
}

#[test]
fn sequential_interps_on_one_thread_get_distinct_root_pids() {
    let mut seen = Vec::new();
    for _ in 0..4 {
        let mut i = Interp::new();
        seen.push(root_pid(&mut i));
    }
    let mut uniq = seen.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(
        uniq.len(),
        seen.len(),
        "sequential `Interp`s on one thread reused a root pid — the second runtime's root is \
         the first's mailbox. Saw: {seen:?}"
    );
}

/// The consequence that actually corrupts: a message left unconsumed in one runtime's root
/// mailbox must not be visible to the next runtime's root. Before the fix the `receive`
/// below popped the previous runtime's envelope.
#[test]
fn an_unconsumed_message_does_not_cross_into_the_next_runtime() {
    {
        let mut a = Interp::new();
        // Send to self and never receive it: the envelope stays queued in the root mailbox.
        a.eval_str("(send (self) [:stale 1]) (send (self) [:stale 2])")
            .expect("queueing two messages nobody receives");
    } // `a` dropped — its root ctx must be retired with it.

    let mut b = Interp::new();
    let v = b
        .eval_str("(receive ([:stale n] [:LEAKED n]) (after 200 :clean))")
        .expect("a fresh runtime's root receive");
    assert_eq!(
        b.print(v),
        ":clean",
        "the previous runtime's queued message was visible to the next runtime's root — the \
         root mailbox was inherited across the `Interp` swap"
    );
}
