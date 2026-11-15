//! **A devirtualized ability call must compute exactly what the op's own dispatch computes.**
//!
//! Ability-dispatch devirtualization (ADR-182) is always on and rewrites call sites in the
//! compiler, which makes it *the* miscompile surface: a wrong devirtualization silently calls
//! the wrong impl — no crash, no error, just a different answer. The rewrite lives in the
//! VM's compiler (`inline::mono_devirtualize`), so it runs at tiers 1 and 2, and the
//! tree-walker (`BROOD_TIER=0`), which never executes it, is the reference every gate here
//! compares against. Each gate also asserts the rewrite **fired**, because a comparison in
//! which nothing was rewritten is two identical dynamic runs and proves nothing (ADR-280).

use std::process::Command;

mod support;
use support::repo_root;

/// What one run printed: stdout, and stderr (where `BROOD_DEVIRT_DBG` traces), and whether it
/// exited cleanly. Every run pins its tier rather than inheriting the CI job's.
struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

fn run_brood(arguments: &[&str], tier: u8, extra_environment: &[(&str, &str)]) -> Run {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brood"));
    command
        .args(arguments)
        .current_dir(repo_root())
        .env("BROOD_TIER", tier.to_string())
        .env("BROOD_DEVIRT_DBG", "1")
        .env_remove("BROOD_CONTRACTS");
    for (name, value) in extra_environment {
        command.env(name, value);
    }
    support::dies_with_parent(&mut command);
    let output = command.output().expect("run brood");
    Run {
        stdout: String::from_utf8_lossy_owned(output.stdout),
        stderr: String::from_utf8_lossy_owned(output.stderr),
        success: output.status.success(),
    }
}

fn trace_count(stderr: &str) -> usize {
    stderr
        .lines()
        .filter(|line| line.starts_with("[devirt] "))
        .count()
}

/// Timings differ run to run and say nothing about correctness, and neither does the boot
/// report (the tree-walker boots its prelude from source, the VM from the image).
///
/// Any line whose last token is a duration (`13.9s`, `2ms`) is timing chatter: the test
/// framework's per-test slow annotation crosses its threshold with machine load. Blank lines
/// go too — a "Slow tests" block's filtered lines leave the blank before them behind, in
/// whichever arm happened to cross a second. A real divergence that only shows in such a line
/// is theoretically maskable, but failures already fail the `success` asserts first.
fn answers_only(text: &str) -> String {
    fn ends_with_duration(line: &str) -> bool {
        let Some(token) = line.trim_end().rsplit(char::is_whitespace).next() else {
            return false;
        };
        let number = token.strip_suffix("ms").or_else(|| token.strip_suffix('s'));
        matches!(number, Some(n) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == '.'))
    }
    text.lines()
        .filter(|line| {
            !line.contains("ms wall")
                && !line.contains("Slow tests")
                && !line.trim_start().starts_with("(stdlib image:")
                && !line.trim_start().starts_with("(prelude:")
                && !ends_with_duration(line)
                && !line.trim().is_empty()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The ability suite as `brood --test` runs it. `tests/ability_test.blsp` uses `suite-helpers`,
/// which `nest test` finds through `project.blsp`'s `:source-paths`; a bare `brood --test`
/// reads no project, so the helper file is loaded first, in the same run.
const ABILITY_SUITE: &[&str] = &[
    "--test",
    "tests/support/suite-helpers.blsp",
    "tests/ability_test.blsp",
];

#[test]
fn the_ability_suite_answers_the_same_devirtualized_and_tree_walked() {
    // The ability suite is the concentrated case: ~94 tests over dispatch, `:default`
    // fallback, sealed members, retraction, and cross-module override precedence.
    let reference = run_brood(ABILITY_SUITE, 0, &[]);
    assert!(
        reference.success,
        "the tree-walked run must pass:\n{}{}",
        reference.stdout, reference.stderr
    );
    for tier in [1, 2] {
        let compiled = run_brood(ABILITY_SUITE, tier, &[]);
        assert!(
            compiled.success,
            "the tier-{tier} run must pass — a failure here is a miscompile, not a flake:\n{}{}",
            compiled.stdout, compiled.stderr
        );
        // Not vacuous: the rewrite has to have happened for the comparison to mean anything.
        let fired = trace_count(&compiled.stderr);
        assert!(
            fired >= 5,
            "only {fired} call sites were devirtualized at tier {tier} — the pass is not \
             reaching the suite, so this compares two dynamic runs:\n{}",
            compiled.stderr
        );
        assert_eq!(
            answers_only(&reference.stdout),
            answers_only(&compiled.stdout),
            "tier {tier} disagrees with the tree-walker — devirtualization changed an ANSWER"
        );
    }
}

/// One way the compile-time proof can stop holding (or never held), with what the op's own
/// dispatch prints for it, and the identity whose devirtualization must be seen to fire.
struct Case {
    name: &'static str,
    source: &'static str,
    expected: &'static str,
    fires_for: &'static str,
    environment: &'static [(&'static str, &'static str)],
}

const CASES: &[Case] = &[
    // ADR-294. Inside ONE `do` the whole body is compiled before any of it runs: at compile
    // time the only impl is `:default`, and a rewrite that baked that in would answer
    // "DEFAULT" for the rest of the program. The retraction is the same question reversed.
    Case {
        name: "an impl registered and used in the same body",
        source: "(defrecord rec (n))\n\
                 (defability Show (render (self) :-> string))\n\
                 (impl Show :default (render (x) \"DEFAULT\"))\n\
                 (do\n\
                 \x20\x20(impl Show rec (render (r) (str \"R\" (get r :n))))\n\
                 \x20\x20(io/puts (str \"same-body: \" (render (rec 7)))))\n\
                 (%unimpl Show rec)\n\
                 (io/puts (str \"after-retract: \" (render (rec 7))))\n",
        expected: "same-body: R7\nafter-retract: DEFAULT\n",
        fires_for: ":rec",
        environment: &[],
    },
    // KI-90/KI-161. `shape` is a record, then rebound to a plain fn returning a bare map; the
    // id stays registered. The constructor proof is structural (the bound closure must build
    // `{:__id__ <id> …}`) — a name comparison refused every module-qualified head and
    // accepted a rebound bare one. Both spellings, because each hid the bug from the other.
    Case {
        name: "a stale record id, in a module",
        source: "(defmodule mono-stale)\n\
                 (defrecord shape (n))\n\
                 (defability Show (render (self) :-> string))\n\
                 (impl Show :default (render (x) \"DEFAULT\"))\n\
                 (impl Show mono-stale/shape (render (r) (str \"REC\" (get r :n))))\n\
                 (io/puts (str \"record: \" (render (shape 7))))\n\
                 (defn shape (n) {:n n})\n\
                 (io/puts (str \"rebound: \" (render (shape 7))))\n",
        expected: "record: REC7\nrebound: DEFAULT\n",
        fires_for: ":mono-stale/shape",
        environment: &[],
    },
    Case {
        name: "a stale record id, module-less",
        source: "(defrecord shape (n))\n\
                 (defability Show (render (self) :-> string))\n\
                 (impl Show :default (render (x) \"DEFAULT\"))\n\
                 (impl Show shape (render (r) (str \"REC\" (get r :n))))\n\
                 (io/puts (str \"record: \" (render (shape 7))))\n\
                 (defn shape (n) {:n n})\n\
                 (io/puts (str \"rebound: \" (render (shape 7))))\n",
        expected: "record: REC7\nrebound: DEFAULT\n",
        fires_for: ":shape",
        environment: &[],
    },
    // The same rebinding AFTER the calling function was compiled: the compile-time proof was
    // true, and only the per-call guard on the constructor's binding can see it lapse.
    Case {
        name: "a constructor rebound after the caller compiled",
        source: "(defrecord shape (n))\n\
                 (defability Show (render (self)))\n\
                 (impl Show :default (render (x) \"DEFAULT\"))\n\
                 (impl Show shape (render (r) \"REC\"))\n\
                 (defn caller () (render (shape 7)))\n\
                 (io/puts (str \"record: \" (caller)))\n\
                 (check-allow :duplicate-def (defn shape (n) {:n n}))\n\
                 (io/puts (str \"rebound: \" (caller)))\n",
        expected: "record: REC\nrebound: DEFAULT\n",
        fires_for: ":shape",
        environment: &[],
    },
    // A `def` always wins (hot reload): the op name rebound after the caller compiled must
    // be what the caller calls next.
    Case {
        name: "an op rebound after the caller compiled",
        source: "(defability Show (render (self)))\n\
                 (impl Show :int (render (x) \"INT\"))\n\
                 (defn caller () (render 5))\n\
                 (io/puts (str \"op: \" (caller)))\n\
                 (check-allow :duplicate-def (defn render (x) \"REBOUND\"))\n\
                 (io/puts (str \"rebound: \" (caller)))\n",
        expected: "op: INT\nrebound: REBOUND\n",
        fires_for: ":int",
        environment: &[],
    },
    // A retracted impl with no `:default` is the op's own `%no-impl` error, not a call of nil.
    Case {
        name: "an impl retracted after the caller compiled",
        source: "(defrecord rec (n))\n\
                 (defability Show (render (self)))\n\
                 (impl Show rec (render (r) \"R\"))\n\
                 (defn caller () (render (rec 1)))\n\
                 (io/puts (str \"first: \" (caller)))\n\
                 (%unimpl Show rec)\n\
                 (io/puts (str \"retracted: \" (try (caller) (catch e (get e :message)))))\n",
        expected: "first: R\nretracted: ability Show/render: no impl for :rec\n",
        fires_for: ":rec",
        environment: &[],
    },
    // Armed contracts check the op's declared return inside the op; a rewrite past the op
    // would return the impl's wrong value unchecked, so an armed run declines every rewrite.
    Case {
        name: "an op's declared return under armed contracts",
        source: "(defability Show (render (self) :-> string))\n\
                 (impl Show :int (render (x) 42))\n\
                 (defn caller () (render 5))\n\
                 (io/puts (str \"contract: \" (try (caller) (catch e (get e :kind)))))\n",
        expected: "contract: :contract\n",
        fires_for: "",
        environment: &[("BROOD_CONTRACTS", "1")],
    },
];

#[test]
fn every_way_the_proof_lapses_answers_what_the_op_answers() {
    let directory = std::env::temp_dir().join(format!("brood-mono-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create temp dir");
    for (index, case) in CASES.iter().enumerate() {
        let file = directory.join(format!("case-{index}.blsp"));
        std::fs::write(&file, case.source).expect("write fixture");
        let path = file.to_str().expect("utf-8 temp path");
        for tier in [0, 1, 2] {
            let result = run_brood(&[path], tier, case.environment);
            assert_eq!(
                result.stdout, case.expected,
                "{} (tier {tier}): the answer must be the op's own — stderr:\n{}",
                case.name, result.stderr
            );
            // Not vacuous: at a compiled tier the rewrite must have fired for the agreement
            // to test it. The contract case is the opposite assertion: it must DECLINE.
            if tier > 0 {
                let fired = trace_count(&result.stderr);
                if case.fires_for.is_empty() {
                    assert_eq!(
                        fired, 0,
                        "{} (tier {tier}): must not devirtualize:\n{}",
                        case.name, result.stderr
                    );
                } else {
                    assert!(
                        result.stderr.contains(&format!("for {}\n", case.fires_for)),
                        "{} (tier {tier}): the call on {} must devirtualize — nothing fired, \
                         so the agreement is vacuous:\n{}",
                        case.name,
                        case.fires_for,
                        result.stderr
                    );
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn an_argument_that_registers_an_impl_keeps_the_call_dynamic() {
    // The op resolves its impl after its arguments run; a devirtualized call resolves it
    // before. An argument that can run user code could register an impl in between, so such
    // a call must not be rewritten — here the second argument registers the `:int` impl the
    // call then uses.
    let directory = std::env::temp_dir().join(format!("brood-mono-order-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create temp dir");
    let file = directory.join("order.blsp");
    std::fs::write(
        &file,
        "(defability Show (render (self y)))\n\
         (impl Show :default (render (x y) \"DEFAULT\"))\n\
         (defn caller () (render 5 (do (impl Show :int (render (x y) \"INT\")) 1)))\n\
         (io/puts (str \"order: \" (caller)))\n",
    )
    .expect("write fixture");
    for tier in [0, 1, 2] {
        let result = run_brood(&[file.to_str().expect("utf-8")], tier, &[]);
        assert_eq!(
            result.stdout, "order: INT\n",
            "tier {tier}: the impl registered by an argument must be the one called:\n{}",
            result.stderr
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_devirtualized_call_stays_correct_after_it_tiers_to_native() {
    // **The suite differential cannot catch a JIT bug**: no devirtualized call in it runs often
    // enough to tier past the bytecode VM. The emitted dispatch call once carried
    // `head = Some(%dispatch)` with `site = NO_SITE`, a pairing `compile_node` cannot produce,
    // and the JIT's site-indexed fast-link path read garbage — correct below the tiering
    // threshold, `cannot call non-function: nil` above it. So this case crosses it.
    let directory = std::env::temp_dir().join(format!("brood-mono-hot-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create temp dir");
    let file = directory.join("hot.blsp");
    std::fs::write(
        &file,
        "(defrecord cir (r))\n\
         (defability Sz (asize (self) :-> int))\n\
         (impl Sz cir (asize (x) (* (get x :r) 2)))\n\
         (impl Sz :default (asize (x) 0))\n\
         ;; a direct-constructor arg, so the devirt fires, in a loop long enough to tier\n\
         (defn hot (n acc) (if (= n 0) acc (hot (- n 1) (+ acc (asize (cir n))))))\n\
         (io/puts (str \"hot: \" (hot 300000 0)))\n",
    )
    .expect("write fixture");
    for tier in [0, 2] {
        let result = run_brood(&[file.to_str().expect("utf-8")], tier, &[]);
        assert_eq!(
            result.stdout, "hot: 90000300000\n",
            "tier {tier} must compute the loop correctly — stderr:\n{}",
            result.stderr
        );
        if tier == 2 {
            assert!(
                trace_count(&result.stderr) >= 1,
                "the loop's `(asize (cir n))` must devirtualize:\n{}",
                result.stderr
            );
        }
    }
    let _ = std::fs::remove_dir_all(&directory);
}
