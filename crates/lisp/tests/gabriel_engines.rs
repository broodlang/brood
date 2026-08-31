//! The Gabriel/Larceny benchmarks, run through **both engines** against upstream's own
//! expected outputs (ROADMAP "External conformance corpora", suite 13).
//!
//! `tests/conformance_gabriel_test.blsp` already checks every ported program against its
//! vendored oracle — but only on whichever engine the in-language runner happens to use,
//! and that is **always the VM**. Measured 2026-07-26: with `BROOD_VM=0` set, a test body
//! run by `nest test` still JIT-compiles (`BROOD_JIT_DUMP_IR=1` lists its arms) and shows
//! no tree-walker slowdown, because the env var gates how a *top-level form* is run while
//! the framework invokes each test as an already-compiled closure. So the env var cannot
//! give these programs tree-walker coverage; only `set_forced_ceiling` can, which is why
//! this file is Rust and lives beside `differential.rs`.
//!
//! What it buys over `differential.rs`: that file's corpus is single expressions probing
//! one feature each. These are whole programs — a rewrite-rule theorem prover, a maze
//! generator, a CPS-transformed recursion, exact big-integer arithmetic — with answers
//! that took the Scheme community years to settle. Agreement between engines is checked
//! *and* both are held to upstream's number, so a shared bug in both engines still fails.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use brood::eval::compile::{set_forced_ceiling, Tier};
use brood::Interp;

static MEM_GUARD: LazyLock<()> = LazyLock::new(|| {
    brood::core::alloc::init_limits_with_default(
        brood::core::alloc::TEST_DEFAULT_HARD,
        brood::core::alloc::TEST_DEFAULT_SOFT,
    );
});

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonicalize repo root")
}

/// A self-contained program: put `tests/support` on the load path, load the ported
/// benchmark, bind `f` to the forms of its vendored `.input` file (upstream's `(read)`
/// order — count, arguments, expected result), and evaluate `expr`, which every case
/// writes so that the answer is `true` exactly when the port matches upstream.
fn driver(module: &str, input: &str, expr: &str) -> String {
    let root = repo_root();
    let support = root.join("tests/support");
    let input_path = root.join("tests/corpus/gabriel/data").join(input);
    format!(
        "(def *load-path* (cons {support:?} *load-path*)) \
         (require-one '{module}) \
         (let (f (reflect/read-all (file/slurp {input_path:?}))) {expr})",
        support = support.to_str().expect("utf-8 path"),
        input_path = input_path.to_str().expect("utf-8 path"),
    )
}

/// Evaluate in a fresh interpreter pinned to one engine.
fn eval_on(src: &str, ceiling: Tier) -> Result<String, String> {
    LazyLock::force(&MEM_GUARD);
    set_forced_ceiling(Some(ceiling));
    let mut interp = Interp::new();
    let out = match interp.eval_str(src) {
        Ok(v) => Ok(interp.print(v)),
        Err(e) => Err(e.message.clone()),
    };
    set_forced_ceiling(None);
    out
}

/// Assert the program agrees with upstream at **every** ceiling. Iterates [`Tier::ALL`], so a new
/// tier inherits the whole Gabriel corpus as conformance coverage. Affordable here in a way it is
/// not in `differential.rs`: this corpus is already sized for what tier 0 can carry in a debug
/// build, so the tier-1 and tier-2 passes are the cheap ones.
fn agrees_with_upstream(module: &str, input: &str, expr: &str) {
    let src = driver(module, input, expr);
    for &ceiling in Tier::ALL {
        assert_eq!(
            eval_on(&src, ceiling),
            Ok("true".to_string()),
            "{module}: did not match upstream's expected output at ceiling {ceiling:?}",
        );
    }
}

/// (module, `.input` file, an expression that is `true` iff the port matches upstream)
///
/// Sizes are the ones the tree-walker can carry in a **debug** build, which is the
/// binding constraint here — it is ~10x slower than the VM and spends ~12.6 kB of native
/// stack per frame, so the deep non-tail programs need care. Measured debug tree-walker
/// costs: mazefun 2.1 s, cpstak 1.8 s, nqueens-8 1.6 s, chudnovsky 0.5 s, primes-100
/// 15 ms, deriv 1 ms. `nboyer` and `takl` are an order of magnitude past that and get
/// their own ignored test below.
const PROGRAMS: &[(&str, &str, &str)] = &[
    // The full derivative tree, ~90 nodes.
    (
        "gabriel/deriv",
        "deriv.input",
        "(= (gabriel/deriv/deriv (nth f 1)) (nth f 2))",
    ),
    // Upstream's oracle is the sieve up to 1000 — 999 levels of non-tail `interval-list`,
    // which exceeds the debug tree-walker's stack budget (it raises a clean E0043 there,
    // correctly). The primes up to 100 are the first 25 entries of that same vendored
    // list, so this stays upstream's bytes at a depth both engines can carry.
    (
        "gabriel/primes",
        "primes.input",
        "(= (gabriel/primes/primes<= 100) (take (nth f 2) 25))",
    ),
    // All 121 cells of the generated maze.
    (
        "gabriel/mazefun",
        "mazefun.input",
        "(= (gabriel/mazefun/make-maze (nth f 1) (nth f 2)) (nth f 3))",
    ),
    // Closure capture + tail calls: upstream's third (smallest) stanza, 18/12/6 -> 7.
    (
        "gabriel/cpstak",
        "cpstak.input",
        "(= (gabriel/cpstak/cpstak (nth f 11) (nth f 12) (nth f 13)) (nth f 14))",
    ),
    // Ten exact big integers, 50 to 500 digits.
    (
        "gabriel/chudnovsky",
        "chudnovsky.input",
        "(= (gabriel/chudnovsky/pies (nth f 1) (nth f 2) (nth f 3)) (nth f 4))",
    ),
    // Upstream's own stanza is n=13; 8 is the largest that fits the debug tree-walker
    // budget. 92 is OEIS A000170's eighth term.
    (
        "gabriel/nqueens",
        "nqueens.input",
        "(= (gabriel/nqueens/nqueens 8) 92)",
    ),
];

#[test]
fn gabriel_programs_agree_with_upstream_on_both_engines() {
    for (module, input, expr) in PROGRAMS {
        agrees_with_upstream(module, input, expr);
    }
}

/// The two programs whose tree-walker leg is too slow to run by default: measured in a
/// debug build, `nboyer` at n=0 takes **38 s** on the tree-walker (0.25 s on the VM) and
/// `takl` at 18/12/6 takes **13 s** (30 ms on the VM). Both run on the VM in
/// `tests/conformance_gabriel_test.blsp` every suite run; this is their tree-walker
/// coverage, on demand:
///
/// ```text
/// cargo nextest run -p brood --test gabriel_engines --run-ignored all
/// ```
#[test]
#[ignore = "~51s on the tree-walker in a debug build; VM coverage is in the .blsp suite"]
fn gabriel_deep_programs_agree_with_upstream_on_both_engines() {
    // The rewrite count is upstream's own sanity check, "because it is too easy for a
    // buggy version to return the correct boolean result".
    agrees_with_upstream(
        "gabriel/nboyer",
        "nboyer.input",
        "(= (gabriel/nboyer/test-boyer gabriel/nboyer/nboyer-alist gabriel/nboyer/nboyer-term \
              0 (gabriel/nboyer/setup-boyer)) \
            95024)",
    );
    // Upstream's oldest documented stanza: lists of 18/12/6, result of length 7.
    agrees_with_upstream(
        "gabriel/takl",
        "takl.input",
        "(= (count (gabriel/takl/mas (reverse (into () (range 1 19))) \
                                    (reverse (into () (range 1 13))) \
                                    (reverse (into () (range 1 7))))) \
            7)",
    );
}
