//! Pins the **clock domain** of a `receive` deadline: it is computed on the scheduler
//! clock ([`process::timer::sched_now`]), so every gate that decides "has it elapsed?"
//! must read that same clock — never `Instant::now()`.
//!
//! Why a source-text guard rather than a behavioural test: **on native the two are the
//! same clock**. `sched_now()` is literally `Instant::now()` off wasm, so a native run
//! cannot tell a correct gate from a broken one, and no `cargo test` on this box would
//! ever go red for the mismatch. The divergence exists only under
//! `wasm32-unknown-unknown`, where there is no timer thread and `sched_now()` is a
//! FROZEN logical clock that only `timer::fire_next_timer` advances.
//!
//! What the mismatch costs, when it is there: `receive_match_timed` stores
//! `recv_deadline = sched_now() + ms`. If `park_on_receive` then compares that against
//! real time, any snippet that computes for longer than `ms` before reaching the
//! `receive` reads "deadline passed" and re-queues, while the re-scan's own
//! `sched_now() >= d` gate reads "not yet" and suspends again. suspend → park →
//! re-queue, forever. `pump_until_quiescent` sees `ran_any = true` on every sweep, so
//! it never falls through to `fire_next_timer` — the one thing that could advance the
//! logical clock and break the cycle. 100% CPU, frozen browser tab.
//!
//! Verified against a real wasm32 build (2026-08-25, `crates/playground` via
//! `wasm-bindgen --target nodejs`, run under node): with the gate on `Instant::now()`
//! the snippet
//!
//! ```text
//! (let (a (receive (after 1 :first))
//!       b (burn 50000000 0)          ; ~2 s of real time, no logical time
//!       c (receive (after 1000 :second)))
//!   (list a c))
//! ```
//!
//! spun at 101% CPU and was still running when killed at 90 s; with the gate on
//! `sched_now()` it returned `(:first :second)` in ~3 s. Shortening the burn below the
//! timeout made even the broken build terminate, which is what identifies the clock
//! mismatch (not the build) as the cause.
//!
//! Limits, stated so a green run is not over-read: this is a **text** check over three
//! function bodies. It cannot prove the gates are correct — only that reintroducing the
//! real clock at either of them is a deliberate act that has to edit this file too.
//!
//! `mailbox::wait_for_message` — the *blocking condvar* path a top-level capture receive
//! never reaches — was originally left out of this guard on the argument that it "cannot
//! work on wasm anyway". That argument is right about the conclusion and wrong about the
//! reachability, so the exemption has been withdrawn and the function fixed instead.
//!
//! Measured 2026-08-25 on the same wasm32 build: `wait_for_message` **is** reached in the
//! browser. A bare top-level `(receive (after 1 :fired))` captures its continuation and
//! answers `:fired`, but `(do (sleep 5) :slept)` traps — `sleep`'s `receive` sits inside a
//! called function, so it takes the native-nested carve-out and lands here, where `std`'s
//! `no_threads` `Condvar` panics (`RuntimeError: unreachable`). That is a separate live
//! defect — wasm needs a non-blocking park for the nested case — but it means a reader
//! cannot dismiss this function as dead code, and a gate mixing clock domains in
//! reachable code is not something to leave documented-as-fine. Off wasm the two clocks
//! are the same function, so the fix costs nothing and the invariant now holds
//! everywhere, which is what makes it worth pinning.

use std::path::Path;

/// Read a repo file relative to the workspace root.
fn source(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// The body of `fn <name>(` in `src`, by brace matching from its opening `{`.
fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
    let needle = format!("fn {name}(");
    let at = src
        .find(&needle)
        .unwrap_or_else(|| panic!("`fn {name}(` not found — was it renamed?"));
    let open = at + src[at..].find('{').expect("fn has no body");
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    for i in open..bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[open..=i];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces in `fn {name}`");
}

/// The body with `//` comment tails stripped, so prose *about* `Instant::now()` (there is
/// a paragraph of it at the fixed gate) doesn't read as a use of it.
fn code_only(body: &str) -> String {
    body.lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn receive_deadlines_are_compared_on_the_clock_they_were_computed_on() {
    // The producer: `recv_deadline` (and the non-capture local deadline) are minted from
    // the scheduler clock. If this ever moves to `Instant::now()`, the consumers below
    // are pinned to the wrong domain instead of the right one.
    let mailbox = source("crates/lisp/src/process/mailbox.rs");
    let produce = code_only(fn_body(&mailbox, "receive_match_timed"));
    let mints = produce.matches("Duration::from_millis(ms").count();
    assert!(
        mints >= 2,
        "expected the capture and non-capture deadline mints in `receive_match_timed`; \
         found {mints}. If the shape changed, re-derive this guard."
    );
    for line in produce
        .lines()
        .filter(|l| l.contains("Duration::from_millis(ms"))
    {
        // The mint is `sched_now() + Duration::from_millis(ms …)`, possibly wrapped over
        // two lines by rustfmt — so accept `sched_now` on the line or the one above.
        assert!(
            produce.contains("sched_now() + Duration::from_millis(ms"),
            "a receive deadline must be minted from `timer::sched_now()`, not real time \
             (offending line: {})",
            line.trim()
        );
    }
    assert!(
        !produce.contains("Instant::now()"),
        "`receive_match_timed` must not read the real clock — a deadline minted or \
         re-checked against `Instant::now()` diverges from `sched_now()` on wasm, where \
         the scheduler clock is frozen logical time. See this file's header."
    );

    // Consumer 1: the re-scan gate ("has my timeout fired?").
    assert!(
        produce.contains("sched_now() >= d"),
        "the re-scan timeout gate in `receive_match_timed` must read `sched_now()`"
    );

    // Consumer 2: the park gate ("did the deadline elapse inside the suspend→park
    // window?"). This is the one that regressed; it is the half of the pair that, read on
    // the real clock, disagrees with consumer 1 and spins the wasm pump forever.
    let pool = source("crates/lisp/src/process/scheduler/pool.rs");
    let park = code_only(fn_body(&pool, "park_on_receive"));
    assert!(
        park.contains("recv_deadline"),
        "`park_on_receive` no longer inspects `recv_deadline` — re-derive this guard"
    );
    assert!(
        park.contains("timer::sched_now()"),
        "`park_on_receive`'s elapsed-deadline branch must read the SCHEDULER clock \
         (`process::timer::sched_now()`), the same one `receive_match_timed` minted the \
         deadline on."
    );
    assert!(
        !park.contains("Instant::now()"),
        "`park_on_receive` must not compare a receive deadline against real time. On \
         wasm that re-queues a process whose re-scan gate then says `not yet` — suspend \
         → park → re-queue forever, at 100% CPU, with `fire_next_timer` (the only thing \
         that advances the logical clock) never reached. See this file's header."
    );

    // Consumer 3: the blocking-condvar park. Not a top-level capture path — but it is
    // reachable on wasm (`sleep`, see the header), so it is held to the same rule rather
    // than exempted.
    let wait = code_only(fn_body(&mailbox, "wait_for_message"));
    assert!(
        wait.contains("deadline"),
        "`wait_for_message` no longer takes a deadline — re-derive this guard"
    );
    assert!(
        wait.contains("sched_now()"),
        "`wait_for_message`'s elapsed check must read the SCHEDULER clock — the one the \
         deadline it is handed was minted on."
    );
    assert!(
        !wait.contains("Instant::now()"),
        "`wait_for_message` must not compare a receive deadline against real time. It is \
         the blocking path, so the symptom differs from `park_on_receive`'s spin — but it \
         is reachable (a `receive` inside a called function, e.g. `sleep`), so it is not \
         exempt. See this file's header."
    );
}
