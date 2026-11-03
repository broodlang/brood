//! When `tcp/connect` returns, the stream must already be installed on the reactor: a
//! `tcp/send` issued at once has to reach the peer.
//!
//! The dial runs on a helper thread. It used to announce `[:tcp-connected]` itself and
//! only then queue the stream for the reactor, so the woken owner could queue its first
//! send AHEAD of the stream — the reactor met a send for an id it did not know and dropped
//! it. Every request/response client (http, the package registry, sse) then waited for an
//! answer to a request never sent. The reactor now emits the announcement when it installs
//! the stream, so any later command is queued behind it.
//!
//! Deterministic construction: the whole runtime is pinned to ONE cpu and one worker, so
//! the dial thread is routinely preempted between announcing and handing off — the first
//! round lost its send every time with the old order (and 84 of 300 in a plain script).
//!
//! Own test binary: the cpu affinity and `set_max_parallel` are process-wide.

#![cfg(target_os = "linux")]

use brood::{Interp, process};

/// Pin the calling thread — and so every thread it later creates — to cpu 0.
fn pin_to_one_cpu() {
    // SAFETY: `cpu_set_t` is plain data; `sched_setaffinity(0, …)` affects this thread.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(0, &mut set);
        let pinned = libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
        assert_eq!(pinned, 0, "sched_setaffinity failed");
    }
}

#[test]
fn a_send_right_after_connect_reaches_the_peer() {
    pin_to_one_cpu();
    process::set_max_parallel(1);
    let mut interp = Interp::new();
    let program = r#"
        (defn round (listener port)
          (let (client (tcp/connect "127.0.0.1" port)
                _ (tcp/send client "x")
                server (receive ([:tcp-accept ^listener accepted] accepted) (after 5000 nil)))
            (tcp/controlling-process server (self))
            (let (got (receive ([:tcp ^server data] data) (after 2000 :lost)))
              (tcp/close client)
              (tcp/close server)
              got)))
        (defn first-lost (listener port remaining)
          (cond (= remaining 0) :none
                (= (round listener port) :lost) remaining
                else (first-lost listener port (- remaining 1))))
        (let (listener (tcp/listen "127.0.0.1" 0)
              lost (first-lost listener (tcp/local-port listener) 100))
          (tcp/close listener)
          lost)
        "#;
    let result = interp.eval_str(program).expect("program errored");
    assert_eq!(
        interp.print(result),
        ":none",
        "a send issued right after tcp/connect returned never reached the peer — the \
         connect was announced before the reactor held the stream"
    );
}
