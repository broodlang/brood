//! Interrupting a running computation (ADR-491): an endless loop inside an interrupt window
//! (`proc/interruptible`) raises a catchable `:interrupted` at its next safepoint, at every
//! execution tier; the interrupt is sticky, so a catch-all inside the work cannot keep it;
//! and a long builtin that iterates natively is stopped from INSIDE.
//!
//! Each program interrupts ITSELF through its window's ceiling (`proc/interruptible 0 …`):
//! the ceiling is due the moment the window opens, so the first safepoint or builtin poll
//! the work reaches raises it. That is the same probe a quit key or `proc/interrupt` sets,
//! minus the second process — no timing, no threads to race.

use brood::Interp;
use brood::eval::compile::{Tier, set_forced_ceiling};

/// Evaluate `src` at `tier` in a fresh interpreter on a worker-sized stack, printed.
fn run_at(tier: Tier, src: String) -> String {
    std::thread::Builder::new()
        .stack_size(brood::process::WORKER_STACK_BYTES)
        .spawn(move || {
            set_forced_ceiling(Some(tier));
            let mut interp = Interp::new();
            match interp.eval_str(&src) {
                Ok(value) => interp.print(value),
                Err(error) => format!("error: {}", error.message),
            }
        })
        .expect("spawn interrupt test thread")
        .join()
        .expect("interrupt test thread panicked")
}

/// A self-tail loop that never ends unless `limit` is reached, warmed past the JIT's tiering
/// threshold with a finite run first so the endless one runs natively at the Native ceiling.
const ENDLESS_LOOP: &str = r#"
    (defn spin (n limit) (if (= n limit) n (spin (inc n) limit)))
    (defn warm (k) (if (= k 0) :warm (do (spin 0 1000) (warm (dec k)))))
    (warm 300)
    (let (r (proc/interruptible 0 (fn () (spin 0 -1))))
      [(first r) (:kind (second r)) (:code (second r))])
"#;

const INTERRUPTED: &str = "[:interrupted :interrupted \"E0047\"]";

#[test]
fn an_endless_loop_is_interrupted_on_the_tree_walker() {
    assert_eq!(run_at(Tier::TreeWalk, ENDLESS_LOOP.into()), INTERRUPTED);
}

#[test]
fn an_endless_loop_is_interrupted_on_the_bytecode_vm() {
    assert_eq!(run_at(Tier::Bytecode, ENDLESS_LOOP.into()), INTERRUPTED);
}

#[cfg(feature = "jit")]
#[test]
fn an_endless_loop_is_interrupted_in_native_code() {
    assert_eq!(run_at(Tier::Native, ENDLESS_LOOP.into()), INTERRUPTED);
}

#[test]
fn a_loop_inside_a_builtin_callback_is_interrupted() {
    // `fold` calls the reducer from a native frame: the nested run's own safepoint raises
    // it, and it unwinds through the builtin.
    let src = r#"
        (defn spin (n) (spin (inc n)))
        (first (proc/interruptible 0 (fn () (fold [1 2 3] 0 (fn (acc x) (spin x))))))
    "#;
    assert_eq!(run_at(Tier::Bytecode, src.into()), ":interrupted");
}

#[test]
fn a_catch_all_inside_the_window_cannot_swallow_the_interrupt() {
    // The work catches everything and goes on: the interrupt is sticky, raised again at the
    // first safepoint after each handler, so it escapes. Afterwards the window is closed and a
    // loop runs to its end.
    let src = r#"
        (defn count-to (n limit) (if (= n limit) n (count-to (inc n) limit)))
        (defn stubborn (swallowed)
          (do (try (count-to 0 -1) (catch e nil)) (stubborn (inc swallowed))))
        [(first (proc/interruptible 0 (fn () (stubborn 0)))) (count-to 0 100000)]
    "#;
    for tier in [Tier::TreeWalk, Tier::Bytecode] {
        assert_eq!(
            run_at(tier, src.into()),
            "[:interrupted 100000]",
            "{tier:?}"
        );
    }
}

#[test]
fn a_window_that_finishes_answers_its_value_and_closes() {
    let src = r#"
        (defn count-to (n limit) (if (= n limit) n (count-to (inc n) limit)))
        [(proc/interruptible nil (fn () (count-to 0 100000))) (proc/interrupt (self))]
    "#;
    assert_eq!(run_at(Tier::Bytecode, src.into()), "[[:ok 100000] false]");
}

/// `call` (a form over `big`) run in a window whose ceiling is already due, after `setup`
/// built `big`: `[:interrupted :stopped-inside]` when the builtin's own poll stopped it. Had
/// it run to its end, the `send` right behind it — no safepoint between two native calls —
/// would leave `:finished` in the mailbox. The `count-to` before the window passes a quantum
/// boundary, so the budget the builtin starts on is fresh and the window's first safepoint
/// is not reached before the builtin is.
fn builtin_is_stopped_inside(setup: &str, call: &str) {
    let src = format!(
        r#"
        (defn count-to (n limit) (if (= n limit) n (count-to (inc n) limit)))
        (def big {setup})
        (count-to 0 5000)
        (let (r (proc/interruptible 0 (fn () (do {call} (send (self) :finished)))))
          [(first r) (receive (:finished :finished) (after 0 :stopped-inside))])
    "#
    );
    assert_eq!(
        run_at(Tier::Bytecode, src),
        "[:interrupted :stopped-inside]",
        "{call}"
    );
}

#[test]
fn a_sort_without_a_callback_is_interrupted_inside() {
    builtin_is_stopped_inside(
        "(vec (map (range 300000) (fn (i) (math/mod (* i 7919) 300007))))",
        "(%sort-asc big)",
    );
    builtin_is_stopped_inside(
        "(vec (map (range 300000) (fn (i) [(math/mod (* i 7919) 300007)])))",
        "(%sort-cmp big)",
    );
}

#[test]
fn string_natives_are_interrupted_inside() {
    builtin_is_stopped_inside("(string/repeat \"ab,\" 100000)", "(string/split big \",\")");
    builtin_is_stopped_inside("(string/repeat \"ab\" 100000)", "(string/->codepoints big)");
    builtin_is_stopped_inside("(vec (range 100000))", "(%string-join \",\" big)");
}

#[test]
fn a_regex_scan_is_interrupted_between_searches() {
    builtin_is_stopped_inside(
        "(string/repeat \"a\" 100000)",
        "(%regex-find-all \"a\" big)",
    );
}

#[test]
fn a_rope_walk_is_interrupted_inside() {
    builtin_is_stopped_inside(
        "(%string->rope (string/repeat \" \" 100000))",
        "(%rope-skip big 0 true \" \" true)",
    );
}

#[test]
fn a_json_encode_is_interrupted_inside() {
    builtin_is_stopped_inside("(vec (range 100000))", "(%json-encode-plain big)");
}

#[test]
fn a_line_diff_is_interrupted_inside() {
    builtin_is_stopped_inside(
        "[(string/join (map (range 20000) (fn (i) (str \"a\" i))) \"\\n\") (string/join (map (range 20000) (fn (i) (str \"b\" i))) \"\\n\")]",
        "(%diff-line-hunks (first big) (second big))",
    );
}

#[test]
fn a_ui_harvest_is_interrupted_inside() {
    builtin_is_stopped_inside(
        "(vec (map (range 100000) (fn (i) [:clear])))",
        "(%ui-harvest big false)",
    );
}

// Compiled only with a Ruby grammar linked (`--features treesit-ruby`, or `treesit-grammars`):
// a plain `cargo test --test interrupt` runs one test fewer, and that is this one.
#[cfg(feature = "treesit-ruby")]
#[test]
fn a_tree_sitter_parse_is_cancelled_from_inside() {
    // ~4 MB of Ruby: a full parse takes far longer than the first progress callback, which
    // sees the ceiling due and cancels.
    builtin_is_stopped_inside(
        "(string/repeat \"def f(x)\\n  x + 1\\nend\\n\" 200000)",
        "(%tree-sitter-parse big :ruby)",
    );
}

#[test]
fn case_maps_and_normalisation_are_interrupted_inside() {
    builtin_is_stopped_inside("(string/repeat \"ab \" 400000)", "(string/upper big)");
    builtin_is_stopped_inside("(string/repeat \"AB \" 400000)", "(string/lower big)");
    // A `Σ` makes lower-casing cut only after whitespace; it still polls.
    builtin_is_stopped_inside("(string/repeat \"ΑΣ \" 400000)", "(string/lower big)");
    builtin_is_stopped_inside(
        "(string/repeat \"é \" 400000)",
        "(string/normalize big :nfd)",
    );
}

#[test]
fn a_piecewise_case_map_answers_what_the_whole_string_would() {
    // `Σ` lower-cases to `ς` at a word's end and `σ` elsewhere, by its neighbours: pieces cut
    // anywhere else would answer differently near a cut.
    let text = "ΑΣ ΣΑ ΣΣ.Σ x".repeat(2000);
    let src = format!("(string/lower \"{text}\")");
    assert_eq!(
        run_at(Tier::Bytecode, src),
        format!("\"{}\"", text.to_lowercase())
    );
    let src = format!("(string/upper \"{text}ß\")");
    assert_eq!(
        run_at(Tier::Bytecode, src),
        format!("\"{}\"", format!("{text}ß").to_uppercase())
    );
}

#[test]
fn a_string_search_is_interrupted_inside() {
    builtin_is_stopped_inside("(string/repeat \"a\" 1000000)", "(%str-index-of big \"z\")");
    builtin_is_stopped_inside(
        "(string/repeat \"a\" 1000000)",
        "(%str-last-index-of big \"z\" nil)",
    );
}

#[test]
fn a_windowed_string_search_finds_a_match_across_a_window_edge() {
    // The search runs 16 KB windows; a needle straddling an edge, after multi-byte text,
    // is found at the char index a whole-string search gives, forwards and backwards.
    for pad in [16_382usize, 16_383, 16_384, 32_767] {
        let text = format!(
            "{}é{}bcd{}",
            "a".repeat(pad),
            "a".repeat(5),
            "a".repeat(40_000)
        );
        let expected = text.chars().count() - 40_000 - 3;
        let src = format!(
            "[(%str-index-of \"{text}\" \"bcd\") (%str-last-index-of \"{text}\" \"bcd\" nil) (%str-index-of \"{text}\" \"zz\") (%str-last-index-of \"{text}\" \"zz\" nil)]"
        );
        assert_eq!(
            run_at(Tier::Bytecode, src),
            format!("[{expected} {expected} -1 -1]"),
            "pad {pad}"
        );
    }
}

#[test]
fn compression_is_interrupted_inside() {
    let big = "(%string->utf8-bytes (string/repeat \"abcdefgh\" 400000))";
    builtin_is_stopped_inside(big, "(%gzip big 9)");
    builtin_is_stopped_inside(big, "(%zlib-compress big)");
    builtin_is_stopped_inside(big, "(%deflate big)");
    builtin_is_stopped_inside(big, "(%brotli big)");
    builtin_is_stopped_inside(
        "(%gzip (%string->utf8-bytes (string/repeat \"abcdefgh\" 400000)))",
        "(%gunzip big)",
    );
}

#[test]
fn hashing_is_interrupted_inside() {
    let big = "(%string->utf8-bytes (string/repeat \"abcdefgh\" 400000))";
    builtin_is_stopped_inside(big, "(%digest :sha256 big)");
    builtin_is_stopped_inside(big, "(%digest :blake3 big)");
    builtin_is_stopped_inside(big, "(%hmac :sha256 (%string->utf8-bytes \"k\") big)");
    builtin_is_stopped_inside(
        "(%string->utf8-bytes \"salt\")",
        "(%pbkdf2-sha256-bytes big big 100000000 32)",
    );
}

#[test]
fn a_region_in_tail_position_answers_and_closes() {
    // The driver runs `proc/interruptible` over a VM thunk as a region on its own frame
    // stack; in tail position its answer is the caller's return value, both ways it ends.
    let src = r#"
        (defn spin (n) (spin (inc n)))
        (defn finishes () (proc/interruptible nil (fn () 7)))
        (defn stopped () (proc/interruptible 0 (fn () (spin 0))))
        [(finishes) (first (stopped)) (proc/interrupt (self))]
    "#;
    for tier in [Tier::Bytecode, Tier::Native] {
        assert_eq!(
            run_at(tier, src.into()),
            "[[:ok 7] :interrupted false]",
            "{tier:?}"
        );
    }
}

#[test]
fn an_error_passes_through_a_region_and_closes_its_window() {
    let src = r#"
        (defn count-to (n limit) (if (= n limit) n (count-to (inc n) limit)))
        [(try (proc/interruptible nil (fn () (raise :boom))) (catch e :caught))
         (proc/interrupt (self))
         (count-to 0 100000)]
    "#;
    assert_eq!(run_at(Tier::Bytecode, src.into()), "[:caught false 100000]");
}

/// Windows nest (ADR-493) on every path that opens one: the bytecode driver's REGION, the tier-0
/// machine's FRAME (ADR-494), and the builtin under a native frame.
const NESTING_TIERS: [Tier; 2] = [Tier::Bytecode, Tier::TreeWalk];

#[test]
fn an_outer_ceiling_passes_through_an_inner_window_to_the_outer() {
    // The outer window's ceiling is the outer window's: the inner one, with no ceiling of its
    // own, lets it pass — and the code after the inner call never runs.
    let src = r#"
        (defn spin (n) (spin (inc n)))
        (let (r (proc/interruptible 0
                  (fn () (do (proc/interruptible nil (fn () (spin 0)))
                             (send (self) :inner-answered)))))
          [(first r) (receive (:inner-answered :inner-answered) (after 0 :passed-through))])
    "#;
    for tier in NESTING_TIERS {
        assert_eq!(
            run_at(tier, src.into()),
            "[:interrupted :passed-through]",
            "{tier:?}"
        );
    }
}

#[test]
fn an_inner_ceiling_is_answered_by_the_inner_window_and_the_outer_runs_on() {
    let src = r#"
        (defn spin (n) (spin (inc n)))
        (defn count-to (n limit) (if (= n limit) n (count-to (inc n) limit)))
        (proc/interruptible nil
          (fn () [(first (proc/interruptible 0 (fn () (spin 0)))) (count-to 0 100000)]))
    "#;
    for tier in NESTING_TIERS {
        assert_eq!(
            run_at(tier, src.into()),
            "[:ok [:interrupted 100000]]",
            "{tier:?}"
        );
    }
}

#[test]
fn a_request_lands_in_the_innermost_window_and_the_outer_needs_its_own() {
    // `proc/interrupt` from another process while the worker is in BOTH windows: the inner
    // answers it, and the outer window — the ui-run turn around a command's own region —
    // runs on, consumed interrupt and all, until a second request stops it.
    let src = r#"
        (def root (self))
        (defn spin (n) (spin (inc n)))
        (defn interrupt-until (pid tries)
          (cond (proc/interrupt pid) :landed
                (= tries 0) :never
                else (do (proc/sleep 10) (interrupt-until pid (dec tries)))))
        (def worker
          (spawn (send root [:outer (proc/interruptible nil
                                      (fn () (do (send root [:inner (first (proc/interruptible nil
                                                                             (fn () (do (send root :in-inner)
                                                                                    (spin 0)))))])
                                                 (spin 0))))])))
        (receive (:in-inner nil) (after 10000 nil))
        (def first-landed (interrupt-until worker 500))
        (def inner (receive ([:inner r] r) (after 10000 :inner-lost)))
        (def outer-early (receive ([:outer r] :outer-stopped-too) (after 100 :outer-runs-on)))
        [first-landed inner outer-early
         (interrupt-until worker 500)
         (receive ([:outer r] (first r)) (after 10000 :outer-lost))]
    "#;
    let mut interp = brood::Interp::new();
    let printed = match interp.eval_str(src) {
        Ok(value) => interp.print(value),
        Err(error) => format!("error: {}", error.message),
    };
    assert_eq!(
        printed,
        "[:landed :interrupted :outer-runs-on :landed :interrupted]"
    );
}

#[test]
fn windows_nest_across_the_frame_and_the_builtin() {
    // The outer window is the engine's own (the VM's region, the tier-0 machine's frame); the
    // inner one is opened in a `reduce` callback, under a native frame, so the builtin opens
    // it. Depths are one count whoever opens them: the inner ceiling is the inner's, and the
    // outer's passes through the inner to the outer.
    let src = r#"
        (defn spin (n) (spin (inc n)))
        (defn count-to (n limit) (if (= n limit) n (count-to (inc n) limit)))
        (defn inner (ceiling) (reduce [ceiling] nil (fn (_ c) (proc/interruptible c (fn () (spin 0))))))
        [(proc/interruptible nil (fn () [(first (inner 0)) (count-to 0 100000)]))
         (first (proc/interruptible 0 (fn () (do (inner nil) :inner-answered))))]
    "#;
    for tier in NESTING_TIERS {
        assert_eq!(
            run_at(tier, src.into()),
            "[[:ok [:interrupted 100000]] :interrupted]",
            "{tier:?}"
        );
    }
}
#[test]
fn a_catch_all_in_an_inner_window_cannot_keep_an_outer_ceiling() {
    // The outer ceiling's interrupt is sticky through a `try` inside the INNER window too: the
    // catch-all only delays it, the inner window lets it pass, the outer answers.
    let src = r#"
        (defn stubborn-spin (n) (stubborn-spin (inc n)))
        (defn stubborn (n) (do (try (stubborn-spin n) (catch e nil)) (stubborn (inc n))))
        (first (proc/interruptible 0 (fn () (proc/interruptible nil (fn () (stubborn 0))))))
    "#;
    for tier in NESTING_TIERS {
        assert_eq!(run_at(tier, src.into()), ":interrupted", "{tier:?}");
    }
}

#[test]
fn a_receive_inside_a_region_parks_and_an_interrupt_lands_after_it_resumes() {
    // More processes than any machine has workers, each parked in a `receive` inside an
    // interrupt region. Run nested under the builtin, each would block its worker thread —
    // the dirty count would climb and, with every worker held, nothing else would run. As a
    // region on the driver's own stack, each parks like any receive.
    let dirty = brood::process::dirty_receive_block_count();
    let src = r#"
        (def root (self))
        (defn spin (n) (spin (inc n)))
        (defn gather (k acc)
          (if (= k 0) acc
              (gather (dec k) (conj acc (receive ([:done r] r) (after 10000 :lost))))))
        (defn start (k acc)
          (if (= k 0) acc
              (start (dec k)
                     (conj acc (spawn (send root [:done (proc/interruptible nil
                                                           (fn () (receive (:go :went))))]))))))
        (defn release (ws) (when (not (empty? ws)) (do (send (first ws) :go) (release (rest ws)))))
        (def waiters (start 256 []))
        (proc/sleep 200)
        (spawn (send root :ran))
        (def ran (receive (:ran :ran) (after 10000 :starved)))
        (release waiters)
        (def answers (gather 256 []))
        ;; Parked inside a region past its ceiling, then woken into an endless loop: the
        ;; ceiling is checked once it runs, and the interrupt lands.
        (def late (spawn (send root [:late (first (proc/interruptible 50
                                                   (fn () (do (receive (:go nil)) (spin 0)))))])))
        (proc/sleep 150)
        (send late :go)
        ;; Woken from a region with no ceiling, then interrupted from outside.
        (def woken (spawn (send root [:woken (first (proc/interruptible nil
                                                     (fn () (do (receive (:go nil)) (spin 0)))))])))
        (proc/sleep 100)
        (defn interrupt-until (pid tries)
          (cond (proc/interrupt pid) :landed
                (= tries 0) :never
                else (do (proc/sleep 10) (interrupt-until pid (dec tries)))))
        (send woken :go)
        [ran
         (= answers (vec (seq/repeat 256 [:ok :went])))
         (receive ([:late r] r) (after 10000 :late-lost))
         (interrupt-until woken 500)
         (receive ([:woken r] r) (after 10000 :woken-lost))]
    "#;
    let mut interp = brood::Interp::new();
    let printed = match interp.eval_str(src) {
        Ok(value) => interp.print(value),
        Err(error) => format!("error: {}", error.message),
    };
    assert_eq!(printed, "[:ran true :interrupted :landed :interrupted]");
    assert_eq!(
        brood::process::dirty_receive_block_count(),
        dirty,
        "a receive inside a region blocked its worker"
    );
}

#[test]
fn a_bounded_regex_search_is_interrupted_inside() {
    // One search with no match over a megabyte: a pattern of bounded length is searched a
    // window at a time, with a poll between.
    builtin_is_stopped_inside(
        "(string/repeat \"a\" 1000000)",
        "(%regex-match? \"zq\" big)",
    );
    builtin_is_stopped_inside(
        "(string/repeat \"a\" 1000000)",
        "(%regex-find \"zq\" big 0)",
    );
}

#[test]
fn an_unbounded_regex_search_is_interrupted_inside() {
    // A pattern with no bound on its match length cannot be cut into windows; it is stepped
    // (ADR-493): its lazy DFA driven a byte at a time, a poll every window. Each of these is
    // one search over a megabyte — no match at all (`a+z`), a match the whole text long
    // (`a+`), a literal prefix the prefilter finds nowhere (`zq.*`), a lexer whose rules
    // never match, and an anchored longest match the whole text long.
    builtin_is_stopped_inside(
        "(string/repeat \"a\" 1000000)",
        "(%regex-match? \"a+z\" big)",
    );
    builtin_is_stopped_inside(
        "(string/repeat \"a\" 1000000)",
        "(%regex-find \"a+\" big 0)",
    );
    builtin_is_stopped_inside(
        "(string/repeat \"a\" 1000000)",
        "(%regex-find \"zq.*\" big 0)",
    );
    builtin_is_stopped_inside(
        "(string/repeat \"a\" 1000000)",
        "(%regex-tokens [\"b[^z]*z\"] [:never] big)",
    );
    builtin_is_stopped_inside(
        "(string/repeat \"a\" 1000000)",
        "(%regex-tokens [\"a+\"] [:run] big)",
    );
    // A Unicode word boundary over non-ASCII text: the lazy DFA quits, our Pike VM polls.
    builtin_is_stopped_inside(
        "(string/repeat \"é\" 500000)",
        "(%regex-find \"\\\\bz\\\\w*\" big 0)",
    );
    builtin_is_stopped_inside(
        "(string/repeat \"é\" 500000)",
        "(%regex-tokens [\"\\\\bz\\\\w*\"] [:word] big)",
    );
}

#[test]
fn a_stepped_regex_search_finds_what_one_search_finds() {
    // Over a text past a window, after multi-byte text, with a literal the prefilter finds
    // straddling a window edge: the match, its groups, an empty match, and a word boundary
    // the stepped DFA quits on (é is not ASCII), so `meta` answers it.
    for pad in [65_534usize, 65_535, 65_536, 131_070] {
        let text = format!("{}é zqyyy xbcd {}", "a".repeat(pad), "a".repeat(70_000));
        let zq = pad + 2;
        let x = zq + 6;
        let src = format!(
            r#"(let (text "{text}")
                 [(:start (%regex-find "zq\\w*" text 0))
                  (:text (%regex-find "zq(y+)" text 0))
                  (:text (first (:groups (%regex-find "zq(y+)" text 0))))
                  (:start (%regex-find "x(b|bc)d+" text 0))
                  (:start (%regex-find "q*" text 5))
                  (:start (%regex-find "\\bx[a-d]+" text 0))
                  (%regex-match? "[^a-z ]+\\d" text)])"#
        );
        assert_eq!(
            run_at(Tier::Bytecode, src),
            format!("[{zq} \"zqyyy\" \"yyy\" {x} 5 {x} false]"),
            "pad {pad}"
        );
    }
}

#[test]
fn a_pike_vm_search_answers_what_one_search_does() {
    // A match longer than a window has its groups found by our polling Pike VM, and a
    // Unicode word boundary over non-ASCII text is searched by it from the start (ADR-493).
    let text = format!("zz é{}b é{}x yy", "a".repeat(100_000), "ü".repeat(70_000));
    let src = format!(
        r#"(let (text "{text}" m (%regex-find "é(a+)(b)" text 0))
             [(:start m) (:end m) (:start (first (:groups m))) (:text (second (:groups m)))
              (:start (%regex-find "\\béü+x\\b" text 0))
              (:start (%regex-find "\\byy" text 0))
              (count (%regex-tokens ["\\b[a-zé]+\\b"] [:word] text))])"#
    );
    let ü = 3 + 1 + 100_000 + 1 + 1 + 1;
    assert_eq!(
        run_at(Tier::Bytecode, src),
        format!(
            "[3 {} 4 \"b\" {} {} 3]",
            3 + 1 + 100_000 + 1,
            ü - 1,
            ü + 70_000 + 2
        )
    );
}

#[test]
fn a_windowed_regex_search_finds_what_one_search_finds() {
    // A match straddling a 64 KB window edge, after multi-byte text, is the one a single
    // search gives — its start, its groups, and a word boundary that looks past the window.
    for pad in [65_534usize, 65_535, 65_536, 131_070] {
        let text = format!(
            "{}é{} xbcd {}",
            "a".repeat(pad),
            "a".repeat(5),
            "a".repeat(70_000)
        );
        let at = pad + 1 + 5 + 1;
        let src = format!(
            "(let (m (%regex-find \"\\\\bx(b|bc)d\" \"{text}\" 0)) [(:start m) (%regex-match? \"q\" \"{text}\")])"
        );
        assert_eq!(
            run_at(Tier::Bytecode, src),
            format!("[{at} false]"),
            "pad {pad}"
        );
    }
}

#[test]
fn a_hard_kill_pending_beside_an_interrupt_lands_first() {
    // Both arrive while the process is inside a native call with no safepoint (a child
    // process it waits on), so its next quantum boundary finds both pending. The kill is
    // untrappable and must win: raised first, the interrupt closed the window and ran the
    // code after it — here the `send` — before the kill landed.
    let src = r#"
        (def root (self))
        (defn spin (n) (spin (inc n)))
        (let ([p watch] (spawn-monitor
                          (do (proc/interruptible nil
                                (fn () (do (%run-process "sleep" ["0.3"]) (spin 0))))
                              (send root :ran-on))))
          (do (proc/sleep 100)
              (proc/interrupt p)
              (exit p :kill)
              [(receive ([:down ^watch _ reason] reason) (after 5000 :survived))
               (receive (:ran-on :ran-on) (after 100 :stopped))]))
    "#;
    assert_eq!(run_at(Tier::Bytecode, src.into()), "[:killed :stopped]");
}
