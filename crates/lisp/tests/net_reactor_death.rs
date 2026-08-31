//! The net reactor's death must be LOUD, not a silent hang.
//!
//! Every socket in the runtime is multiplexed on one reactor thread. Before the death
//! hook (`reactor_died`) existed, that thread dying — a panic anywhere in the event
//! loop, or a fatal `poll` error — was swallowed whole: `Reactor::cmd` discarded the
//! channel error, so `tcp-send`/`tcp-listen` kept reporting success into a dead
//! channel, no `[:tcp-closed]` was ever emitted again, and every socket-owning process
//! parked in `receive` forever with zero diagnostics. This test induces the death (a
//! debug-only command that panics the reactor exactly as a real event-loop bug would)
//! and asserts the two observable halves of the fix:
//!
//! 1. operations that would silently queue into the dead reactor now ERROR;
//! 2. a socket that was live at death is failed at its owner (registry drained), so
//!    a later op on its id reports the reactor death instead of succeeding.
//!
//! Own test binary on purpose: the reactor is a process-global singleton and this
//! kills it for the life of the process — under nextest each test binary is its own
//! process, so nothing else shares the corpse.
//!
//! **Sabotage-verified** (2026-08-31): with the death hook's `catch_unwind` +
//! `REACTOR_DOWN` sweep removed (the pre-fix tree), `send` after death returns
//! `Ok(())` and both asserts below fail.

#![cfg(debug_assertions)]

use std::time::{Duration, Instant};

#[test]
fn reactor_death_errors_instead_of_hanging() {
    // A live listener, so the death sweep has a registered socket to fail.
    let lid = brood::net::listen("127.0.0.1", 0, 999_999).expect("listen before death");
    assert!(brood::net::local_port(lid).is_some(), "listener registered");

    brood::net::die_for_test();

    // The death is asynchronous (the reactor drains the command on its own thread);
    // wait for the flag to land rather than sleeping a fixed amount.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if brood::net::listen("127.0.0.1", 0, 999_999).is_err() {
            break; // the gate is up: the reactor is known-dead
        }
        assert!(
            Instant::now() < deadline,
            "reactor death was never observed: listen still succeeds"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // 1. Every entry point errors rather than reporting success into a dead channel.
    assert!(
        brood::net::connect("127.0.0.1", 1, 999_999).is_err(),
        "connect must error once the reactor is dead"
    );
    assert!(
        brood::net::send(lid, b"x").is_err(),
        "send must error once the reactor is dead"
    );

    // 2. The pre-death socket was swept out of the registry (failed at its owner),
    //    so its id no longer resolves.
    assert!(
        brood::net::local_port(lid).is_none(),
        "the death sweep must drain the registry"
    );
}
