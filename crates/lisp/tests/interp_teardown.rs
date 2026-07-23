//! Embedded-host teardown (the mailbox waiter-slot leak, fixed 2026-07-23):
//! dropping an `Interp` reaps its permanently-parked green processes — a
//! `(receive)` nothing will ever send to no longer pins its process + heap
//! for the life of the host.

use brood::Interp;

/// Poll until `cond` or ~2 s. The scheduler parks asynchronously, so tests
/// wait for the state rather than assuming it.
fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
    for _ in 0..400 {
        if cond() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    cond()
}

#[test]
fn dropping_an_interp_reaps_permanently_parked_waiters() {
    let parked_before = brood::process::parked_process_count();
    let exited_before = brood::process::exit_count();
    {
        let mut interp = Interp::new();
        // Three processes parked forever: nothing will ever send to them.
        interp
            .eval_str("(do (spawn (receive)) (spawn (receive)) (spawn (receive)) :ok)")
            .unwrap();
        assert!(
            wait_until(|| brood::process::parked_process_count() >= parked_before + 3),
            "the spawned processes never parked"
        );
    } // Interp drops here — the teardown must reap all three.
    assert!(
        wait_until(|| brood::process::parked_process_count() <= parked_before),
        "parked waiters survived the Interp drop (the embedded-host leak)"
    );
    assert!(
        brood::process::exit_count() >= exited_before + 3,
        "reaped processes must go through the normal death path (deregister)"
    );
}

#[test]
fn a_woken_process_is_not_reaped_by_another_interps_drop() {
    // Interp A parks a process; Interp B (a different runtime) drops. A's
    // parked process must survive — the drain is runtime-scoped.
    let mut a = Interp::new();
    let parked_before = brood::process::parked_process_count();
    a.eval_str("(def p (spawn (receive)))").unwrap();
    assert!(wait_until(|| {
        brood::process::parked_process_count() >= parked_before + 1
    }));
    {
        let _b = Interp::new();
    } // B drops — must not touch A's waiter.
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(
        brood::process::parked_process_count() >= parked_before + 1,
        "another runtime's drop reaped our parked process"
    );
    // A's process is still alive and wakeable: send to it and let it finish.
    a.eval_str("(send p :wake)").unwrap();
    assert!(
        wait_until(|| brood::process::parked_process_count() <= parked_before),
        "the parked process did not wake on send"
    );
}
