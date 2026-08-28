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

/// Dropping a short-lived `Interp` must not retire a *different*, still-live `Interp`'s root
/// context on the same thread.
///
/// `deregister_root_ctx` takes whatever context the thread holds, with no check that the
/// caller minted it — so the ownership check has to come from the runtime tag. A host with a
/// long-lived interpreter that builds a scratch one beside it (an LSP-style server doing a
/// throwaway check) would otherwise find, on dropping the scratch one, that its main
/// interpreter's pid changed, its queued mailbox was discarded, and its monitors and links
/// had fired as a death.
#[test]
fn dropping_a_nested_interp_leaves_the_outer_ones_root_ctx_alone() {
    let mut outer = Interp::new();
    let before = root_pid(&mut outer);
    outer
        .eval_str("(send (self) [:mine 1])")
        .expect("queue a message on the outer root");

    {
        let mut scratch = Interp::new();
        scratch.eval_str("(+ 1 2)").expect("scratch work");
    } // dropped here

    let after = root_pid(&mut outer);
    assert_eq!(
        before, after,
        "dropping a nested `Interp` retired the outer interpreter's root context — its pid \
         changed from {before} to {after}"
    );
    let v = outer
        .eval_str("(receive ([:mine n] [:kept n]) (after 200 :LOST))")
        .expect("the outer root's own message");
    assert_eq!(
        outer.print(v),
        "[:kept 1]",
        "dropping a nested `Interp` discarded the outer interpreter's queued mailbox"
    );
}
