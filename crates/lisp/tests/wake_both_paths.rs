//! A receiver must be woken however it happens to be waiting.
//!
//! `deliver` used to choose: re-queue the parked green process, **or else** notify the
//! mailbox condvar. That reads as exhaustive and is not. A green process that enters a
//! **native-nested** `receive` — a `receive` inside a HOF running in a Rust builtin, e.g.
//! `(reduce (fn (acc _) (receive …)) …)`, the §7.4 dirty-scheduler carve-out — blocks its
//! worker on the condvar without clearing a `waiter` an earlier park left behind, so it is
//! reachable by *both* paths. Whenever a `waiter` was present the `else` never ran and the
//! condvar was never notified. A condvar notify does not latch: delivered to no one it is
//! discarded and nothing recovers it. `mailbox::wake_both` now signals both, every time.
//!
//! `wake_for_timeout` was the clearest case — alone among the wake sites it had **no**
//! condvar notify at all, survivable only because a cv-blocked receiver with a deadline sits
//! in `wait_timeout` and self-wakes at the same instant.
//!
//! **These are path-coverage tests, not a regression test for KI-72.** They assert the wake
//! paths deliver; they do **not** reproduce the either/or race, which needs a `waiter` and a
//! cv-blocked receiver to coexist. Verified by sabotage: restoring the old `if parked {…}
//! else {…}` leaves all three passing. KI-72 itself remains open and is NOT closed by this
//! change — see `docs/known-issues.md`. Do not read a green run here as evidence that the
//! either/or cannot come back; that would need a test that can stage both states at once.

use brood::Interp;

/// The load-completion protocol (`%require-await`, the OTP `code_server` model): many
/// processes racing the first call into an autoloaded module must ALL get the right answer,
/// and the waiters must be released by the loader's completion rather than by a poll's
/// timeout. A dropped notification shows up here as a hang, not a wrong answer.
#[test]
fn racing_requirers_are_all_released_by_the_loader() {
    let mut interp = Interp::new();
    let v = interp
        .eval_str(
            "(def root (self))\
             (dotimes (_ 32) (spawn (send root [:r (string/blank? \"\")])))\
             (let (got (reduce (fn (a _) (receive ([:r v] (cons v a)))) (list) (range 32)))\
               [(count got) (count (filter (fn (x) (= x true)) got))\
                (%registry-member? '*features* \"string\")])",
        )
        .expect("32 processes racing the first call into `string`");
    assert_eq!(
        interp.print(v),
        "[32 32 true]",
        "a racing requirer was not released by the loader's completion — every one of the 32 \
         must get `true`, and `string` must read as loaded"
    );
}

/// The exact failing shape: a `receive` nested inside the native `reduce` builtin, with **no
/// `after`** — so nothing can self-wake it and a lost notify is a permanent hang rather than
/// a slow test. The harness timeout is the only backstop, which is why this is a test and not
/// a benchmark: if the wake regresses, this hangs.
#[test]
fn a_receive_nested_in_a_native_hof_is_woken() {
    let mut interp = Interp::new();
    let v = interp
        .eval_str(
            "(def root (self))\
             (dotimes (_ 24) (spawn (send root [:r 7])))\
             (count (reduce (fn (acc _) (receive ([:r v] (cons v acc)))) (list) (range 24)))",
        )
        .expect("racing 24 senders into a native-nested receive");
    assert_eq!(
        interp.print(v),
        "24",
        "a `receive` inside the native `reduce` builtin missed a wakeup — every one of the 24 \
         sends must reach it, however the receiver happens to be parked"
    );
}

/// The same, one runtime layer deeper: the nested `receive` runs inside a `try`, which is the
/// other way onto the native-nested path. Also drains in a second `reduce`, so the receiver
/// parks and unparks repeatedly rather than once.
#[test]
fn repeated_parks_in_a_native_hof_are_all_woken() {
    let mut interp = Interp::new();
    let v = interp
        .eval_str(
            "(def root (self))\
             (dotimes (_ 40) (spawn (send root [:r 1])))\
             (count (reduce (fn (acc _) (try (receive ([:r v] (cons v acc))) (catch _ acc)))\
                      (list) (range 40)))",
        )
        .expect("40 sends into a try-wrapped native-nested receive");
    assert_eq!(
        interp.print(v),
        "40",
        "a repeated native-nested park lost a wakeup"
    );
}

/// `exit` must reach a receiver blocked on the condvar even when a `waiter` is present — the
/// kill path had the same either/or. A process parked on a `receive` nothing will ever send
/// to is exactly the case where "it will wake on the next message" is never.
#[test]
fn exit_reaches_a_receiver_blocked_in_a_native_hof() {
    let mut interp = Interp::new();
    let v = interp
        .eval_str(
            "(def root (self))\
             ;; the child blocks in a native-nested receive that nothing ever sends to
             (def victim (spawn (reduce (fn (acc _) (receive ([:never v] (cons v acc))))\
                                  (list) (range 1))))\
             (sleep 50)\
             (exit victim :kill)\
             (defn settle (n)\
               (if (or (= n 0) (not (proc/alive? victim))) (proc/alive? victim)\
                 (do (sleep 20) (settle (dec n)))))\
             (settle 100)",
        )
        .expect("killing a process blocked in a native-nested receive");
    assert_eq!(
        interp.print(v),
        "false",
        "`exit` did not reach a process blocked on the mailbox condvar — the kill sat waiting \
         for a message that was never going to arrive"
    );
}
