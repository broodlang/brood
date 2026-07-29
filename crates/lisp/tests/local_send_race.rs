//! The L1 local-send fast path under contention (`process/mailbox.rs::try_deliver_local`).
//!
//! L1 lets a `send` to a **parked** local process copy the value straight from the
//! sender's heap into the *receiver's* heap, skipping the wire-format `Message` round
//! trip. That is the one place in the runtime where a thread writes into another
//! process's heap, so its soundness rests entirely on one claim:
//!
//!   taking the receiver's `Box<Process>` out of `MailboxState::waiter` under the
//!   mailbox mutex confers exclusive ownership of it — nobody else can reach that
//!   heap until we put it back or hand it to `wake_enqueue`.
//!
//! This test attacks that claim from the angle the existing TSAN suite doesn't cover:
//! **many senders racing to the same receiver**, which repeatedly parks and wakes, on a
//! small worker pool so senders, the waking receiver, and the collector all overlap.
//! Every message carries a *structured* payload (nested vectors, a map, a string), so
//! the cross-heap copier is allocating into the receiver's heap while that receiver is
//! being scheduled — the window where a missed root or a stolen `waiter` shows up.
//!
//! Correctness is checked by value, not just by "it didn't crash": each sender sends a
//! payload derived from its own id, and the receiver sums a field out of every message.
//! A dropped, duplicated, or corrupted fast-path delivery changes the total.
//!
//! Its own test binary because `set_max_parallel` is process-wide.

use brood::{process, Interp};

#[test]
fn concurrent_senders_into_one_parked_receiver() {
    // A small pool so senders and the waking receiver contend for workers.
    process::set_max_parallel(2);

    let mut interp = Interp::new();
    let setup = r#"
        (def root (self))

        ;; Structured payload: nested vector + map + string, so the cross-heap copier
        ;; walks every container kind it claims to handle rather than an inline scalar.
        (defn payload (id i)
          [:msg id i [id i [:deep id]] {:id id :i i :tag "p"} "body"])

        ;; One sender: `n` structured messages to `dst`, then a done marker.
        (defn blast (dst id i n)
          (if (>= i n)
              (send dst [:sender-done id])
              (do (send dst (payload id i)) (blast dst id (+ i 1) n))))

        ;; The receiver parks on every iteration — each `receive` is a fresh park, so
        ;; each inbound send races a receiver that may be parked, waking, or running.
        ;; Sums `id + i` out of each message: a corrupted copy moves the total.
        (defn collect (want-done acc)
          (if (= want-done 0)
              (send root [:total acc])
              (receive
                ([:msg id i _ m _] (collect want-done (+ acc (+ id i))))
                ([:sender-done _]  (collect (- want-done 1) acc)))))

        (defn spawn-senders (dst k n acc)
          (if (= k 0) acc
              (spawn-senders dst (- k 1) n (cons (spawn (blast dst k 0 n)) acc))))

        ;; `k` senders x `n` messages into one receiver. Expected total is
        ;; sum over senders of (k*n + n*(n-1)/2).
        (defn burst (k n)
          (let (dst (spawn (collect k 0)))
            (do (spawn-senders dst k n [])
                (receive ([:total t] t) (after 60000 :timeout)))))
    "#;
    interp.eval_str(setup).expect("setup errored");

    let (k, n) = (8_i64, 250_i64);
    // sum_{id=1..k} (id*n + n*(n-1)/2)
    let expected: i64 = (1..=k).map(|id| id * n + n * (n - 1) / 2).sum();

    for round in 0..12 {
        let v = interp
            .eval_str(&format!("(burst {} {})", k, n))
            .expect("burst errored");
        let got = interp.print(v);
        assert_eq!(
            got,
            expected.to_string(),
            "round {round}: fast-path delivery lost, duplicated or corrupted a message \
             (expected the per-sender id/index sum)"
        );
    }
}

/// The declined half of the path: a value the cross-heap copier does **not** handle
/// (a closure) must fall back to the wire `Message` route with identical semantics —
/// and, critically, must put the receiver's `Box<Process>` back exactly as it found it.
/// A decline that dropped the process on the floor would hang; one that left it in an
/// inconsistent state would surface here as a wrong result under the same contention.
#[test]
fn declined_values_fall_back_without_disturbing_the_receiver() {
    process::set_max_parallel(2);

    let mut interp = Interp::new();
    let setup = r#"
        (def root (self))

        ;; Alternate a copier-friendly value with a closure (which the fast path
        ;; declines), so each sender exercises both routes into the same parked peer.
        (defn blast2 (dst i n)
          (if (>= i n)
              (send dst [:done])
              (do (if (= (mod i 2) 0)
                      (send dst [:v i])
                      (send dst [:f (fn () i)]))
                  (blast2 dst (+ i 1) n))))

        (defn collect2 (want acc)
          (if (= want 0)
              (send root [:total acc])
              (receive
                ([:v i] (collect2 want (+ acc i)))
                ([:f g] (collect2 want (+ acc (g))))
                ([:done] (collect2 (- want 1) acc)))))

        (defn spawn2 (dst k n)
          (if (= k 0) nil (do (spawn (blast2 dst 0 n)) (spawn2 dst (- k 1) n))))

        (defn burst2 (k n)
          (let (dst (spawn (collect2 k 0)))
            (do (spawn2 dst k n)
                (receive ([:total t] t) (after 60000 :timeout)))))
    "#;
    interp.eval_str(setup).expect("setup errored");

    let (k, n) = (6_i64, 200_i64);
    // Every i in 0..n contributes i once per sender, whichever route carried it.
    let expected: i64 = k * (0..n).sum::<i64>();

    for round in 0..8 {
        let v = interp
            .eval_str(&format!("(burst2 {} {})", k, n))
            .expect("burst errored");
        let got = interp.print(v);
        assert_eq!(
            got,
            expected.to_string(),
            "round {round}: mixing fast-path and declined (closure) sends changed the result"
        );
    }
}
