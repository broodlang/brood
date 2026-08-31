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
             (let (got (reduce (range 32) (list) (fn (a _) (receive ([:r v] (cons v a))))))\
               [(count got) (count (filter got (fn (x) (= x true))))\
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

/// A waiter that stops waiting must not be left holding the loader's completion.
///
/// The waiter registers, *then* re-checks. If the re-check wins — the loader provided while
/// we were joining the queue — we stop waiting, but the loader may already have taken the
/// waiter list and will still `send` the completion. Leaving the queue does not un-send it,
/// and `demonitor` is best-effort, so an ordinary process that merely called an autoloaded
/// function could be left holding a stray `[:load-done …]`. A later catch-all `receive` — a
/// `gen` server's unknown-message clause — would then pick it up as application traffic.
///
/// That interleaving is not reproducible on demand (racing an autoload 16 ways does not hit
/// it), so this stages it directly through the protocol's own helpers instead: enqueue a
/// waiter, give up the way `%require-unwatch!` does, and only then let the loader drain and
/// send. The guarantee is the one OTP 24 reply aliases provide — the completion is addressed
/// to the waiter's ref, and a deactivated ref's message is dropped at DELIVERY.
#[test]
fn a_completion_for_a_waiter_that_gave_up_is_dropped_at_delivery() {
    let mut interp = Interp::new();
    let v = interp
        .eval_str(
            r#"
            (let (k "zz-not-a-real-module" r (ref))
              ;; 1. join the queue, as `%require-await` does
              (%require-enqueue-waiter! k (self) r)
              ;; 2. give up, as every non-notified exit path does
              (%require-unwatch! k (self) r nil)
              ;; 3. the loader finishes and drains a list it took BEFORE we left. Staged by
              ;;    re-adding us: the loader cannot tell the difference, and this is exactly
              ;;    the state the race produces.
              (%require-enqueue-waiter! k (self) r)
              (%require-release! k)
              ;; Nothing may have queued: the ref is deactivated.
              (%mailbox-size (self)))
            "#,
        )
        .expect("staging the give-up window");
    assert_eq!(
        interp.print(v),
        "0",
        "the completion for a waiter that had already given up was queued in its mailbox — \
         the reply alias was not deactivated, so require-protocol traffic leaks into an \
         application process"
    );
}

/// A loader KILLED mid-load must hand the load back, not poison the module.
///
/// Nothing clears `*features-loading*` when a process dies, so the dead claimant's marker
/// outlives it. A waiter that retries without clearing it re-enters `%require-await`,
/// monitors the same dead pid, and `add_monitor` answers a dead target with an *immediate*
/// synthetic `[:down … :noproc]` — a hot loop with no delay in it, spinning at 100% CPU and
/// poisoning that module for every process. The `:down` handler clears the stale claim first.
///
/// The assertion is that the require completes at all: before the fix this test does not
/// fail, it hangs.
#[test]
fn a_loader_killed_mid_load_hands_the_load_back() {
    let mut interp = Interp::new();
    let v = interp
        .eval_str(
            "(def root (self))\
             ;; Claim `string`'s load, then die holding the claim — the marker stays set.
             (def victim (spawn (do (%registry-update! '*features-loading* :assoc-new\
                                      [\"string\"] (self))\
                                  (send root [:claimed])\
                                  (receive ([:never] nil)))))\
             (receive ([:claimed] nil) (after 2000 nil))\
             (exit victim :kill)\
             ;; Now require it. The claim is held by a corpse.
             (require-one \"string\")\
             [(string/blank? \"\") (%registry-member? '*features* \"string\")]",
        )
        .expect("requiring a module whose claimant was killed mid-load");
    assert_eq!(
        interp.print(v),
        "[true true]",
        "a dead claimant's stale marker was not cleared — the module stays unloadable"
    );
}

/// A load that FAILS must not be reported to its waiters as a success.
///
/// The loader releases its waiters on the error path too — it has to, or they would block on
/// a load that will never provide. So `[:load-done]` means "the claimant is finished", not
/// "the module is loaded", and a waiter that trusts it returns as if the require succeeded;
/// the caller then meets an unbound name instead of the loader's actual error. `%require-await`
/// re-checks `*features*` rather than trusting the notification.
#[test]
fn a_failed_load_does_not_report_success_to_racing_requirers() {
    let mut interp = Interp::new();
    // Every one of these must see an ERROR, not a silent success: the module does not exist,
    // so whichever process wins the claim fails, and the waiters must fail the same way
    // rather than being told the load is done.
    let v = interp
        .eval_str(
            "(def root (self))\
             (dotimes (_ 8)\
               (spawn (send root [:r (try (do (require-one \"zzz-no-such-module\") :ok)\
                                       (catch _ :err))])))\
             (let (got (reduce (range 8) (list) (fn (a _) (receive ([:r v] (cons v a))))))\
               [(count got) (count (filter got (fn (x) (= x :err))))])",
        )
        .expect("8 processes racing a require of a module that does not exist");
    assert_eq!(
        interp.print(v),
        "[8 8]",
        "a failed load was reported to a racing requirer as success — `[:load-done]` means \
         the claimant finished, not that the module loaded"
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
             (count (reduce (range 24) (list) (fn (acc _) (receive ([:r v] (cons v acc))))))",
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
             (count (reduce (range 40) (list)\
                      (fn (acc _) (try (receive ([:r v] (cons v acc))) (catch _ acc)))))",
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
             (def victim (spawn (reduce (range 1) (list)\
                                  (fn (acc _) (receive ([:never v] (cons v acc)))))))\
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
