//! **`BROOD_CONTRACTS=1` must boot, and must actually enforce.**
//!
//! The mode had no end-to-end coverage at all, and it had rotted into unusable: on a COLD
//! boot cache it aborted the interpreter before running a line (KI-81). Two independent
//! causes, both invisible warm, because a warm cache replays an already-expanded prelude and
//! never runs the macro bodies below.
//!
//! 1. `sig!`'s expansion-time code called `take`/`nth`/`map`/`range`/`count`, which the
//!    prelude does not have that early — `take` had left the bare namespace entirely
//!    (ADR-290/291) and nothing noticed, because nothing expanded that path.
//! 2. The contract shim was `(let (orig name) (fn …))`, a closure over a **let-bound local**,
//!    and the prelude's freeze step rejects exactly that ("shared closures must capture the
//!    global env"). So arming contracts over the prelude's own sigs aborted the boot.
//!
//! Both are cold-cache-only, so this test **cold-caches deliberately** by pointing
//! `XDG_CACHE_HOME` at a fresh temp dir. Without that it passes on a broken build: the
//! warm-cache path was green throughout the entire period the mode was unusable.

use std::path::PathBuf;
use std::process::Command;

mod support;

struct TempDir {
    path: PathBuf,
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn temp_dir(tag: &str) -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("brood-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create temp dir");
    TempDir { path }
}

/// The program under test: a `sig`-declared function and an ability op, each with a body
/// that contradicts its declaration, plus the well-behaved calls beside them.
const PROGRAM: &str = "\
(defn bad-result (n) \"not an int\")\n\
(sig bad-result (int -> int))\n\
(defn good (n) n)\n\
(sig good (int -> int))\n\
(defability Size (size [self] :-> int) (tag [self]))\n\
(impl Size :string (size [s] \"not an int\") (tag [s] :anything))\n\
(impl Size :int (size [n] n) (tag [n] :fine))\n\
(defn- report (label thunk)\n\
\x20\x20(io/puts (str label (try (thunk) (catch e (str \"RAISED \" (error-message e)))))))\n\
(defn padded (s w &optional (fill \"-\")) (string/pad-left s w fill))\n\
(sig padded (string int &optional string -> string))\n\
(report \"sig-good: \" (fn () (good 7)))\n\
(report \"sig-bad: \" (fn () (bad-result 1)))\n\
(report \"op-good: \" (fn () (size 7)))\n\
(report \"op-bad: \" (fn () (size \"x\")))\n\
(report \"op-undeclared: \" (fn () (tag \"x\")))\n\
(report \"opt-absent: \" (fn () (padded \"x\" 3)))\n\
(report \"opt-given: \" (fn () (padded \"x\" 3 \"*\")))\n\
(report \"opt-bad: \" (fn () (padded \"x\" \"three\")))\n";

/// Run the program with a **fresh** cache dir, so the prelude is expanded from source rather
/// than replayed — the only configuration in which either KI-81 cause is reachable.
fn run(contracts: bool) -> (String, bool) {
    let dir = temp_dir(if contracts {
        "contracts-on"
    } else {
        "contracts-off"
    });
    let program = dir.path.join("program.blsp");
    std::fs::write(&program, PROGRAM).expect("write program");
    let cache = dir.path.join("cache");
    std::fs::create_dir_all(&cache).expect("create cache dir");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg("program.blsp")
        .current_dir(&dir.path)
        .env("XDG_CACHE_HOME", &cache);
    if contracts {
        cmd.env("BROOD_CONTRACTS", "1");
    } else {
        cmd.env_remove("BROOD_CONTRACTS");
    }
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run brood");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (text, out.status.success())
}

#[test]
fn contracts_mode_boots_on_a_cold_cache_and_enforces_both_kinds() {
    let (text, ok) = run(true);

    // The boot itself. A panic here is the KI-81 shape, and it names its own cause, so the
    // message is worth surfacing rather than just asserting a bool.
    assert!(
        !text.contains("panicked"),
        "BROOD_CONTRACTS=1 aborted on a cold boot cache — this is KI-81's shape:\n{text}"
    );
    assert!(ok, "the program should run to completion:\n{text}");

    // A `sig` contract fires on the result, and leaves a correct call alone.
    assert!(
        text.contains("sig-good: 7"),
        "a call matching its sig must pass through untouched:\n{text}"
    );
    assert!(
        text.contains("sig-bad: RAISED") && text.contains("result expected int"),
        "a body contradicting its sig must raise under contracts:\n{text}"
    );

    // An ability op's declared `:-> RET` is enforced the same way (ADR-180 deferred item c).
    assert!(
        text.contains("op-good: 7"),
        "an impl matching its declared return must pass through:\n{text}"
    );
    assert!(
        text.contains("op-bad: RAISED") && text.contains("Size/size: result expected int"),
        "an impl contradicting the ability's declared return must raise:\n{text}"
    );

    // An op the ability declares WITHOUT a return type is not constrained by this.
    assert!(
        text.contains("op-undeclared: :anything"),
        "an op with no declared return must be left alone:\n{text}"
    );

    // `&optional`. The shim has to check the optionals that were SUPPLIED and pass the
    // rest through untouched — read as fixed parameters, the `&optional` marker counts as
    // one, so the shim took four arguments and every ordinary call became an arity error.
    // The callee's own default is the part a wrapper is most likely to destroy: passing an
    // explicit `nil` for an absent optional type-checks fine and silently changes the answer.
    assert!(
        text.contains("opt-absent: --x"),
        "an omitted optional must keep the callee's own default:\n{text}"
    );
    assert!(
        text.contains("opt-given: **x"),
        "a supplied optional must reach the callee:\n{text}"
    );
    assert!(
        text.contains("opt-bad: RAISED") && text.contains("argument 2 expected int"),
        "a contract must still fire on a fixed argument of an &optional signature:\n{text}"
    );
}

#[test]
fn without_the_flag_nothing_is_enforced() {
    // The default build must be untouched: the shim is decided at expansion time, so with
    // the flag unset it is never emitted and every wrong value flows through as before.
    let (text, ok) = run(false);
    assert!(ok, "the program should run to completion:\n{text}");
    assert!(
        text.contains("sig-bad: not an int") && text.contains("op-bad: not an int"),
        "with contracts off, a declaration is advisory and nothing raises:\n{text}"
    );
}

/// Every baked-in module must LOAD under `BROOD_CONTRACTS=1` — from SOURCE.
///
/// The case above boots a small program and enforces two kinds of contract, and it passed
/// while eleven std modules could not load at all in that mode: with the stdlib image
/// present a materialised module never evaluates its `(sig …)` forms, so the only runs that
/// reached them were ones with no current image, which nothing in CI is. Two shapes were
/// hiding there (2026-09-06): a `(sig *name* int)` on a VALUE — a bare type, not an arrow —
/// which `sig!`'s position helper answered by calling `first` on the symbol; and a `sig`
/// indented inside a `(check-allow …)` above its own `defn-`, whose deferred contract lands
/// at `provide`, after the loader's reserved-name exemption ends, and is refused. A fresh
/// `XDG_CACHE_HOME` plus `BROOD_NO_STDIMAGE=1` is the configuration in which both are
/// reachable; the program requires every module the binary bakes and names each that fails.
#[test]
fn every_baked_in_module_loads_under_contracts_from_source() {
    let dir = temp_dir("contracts-all-modules");
    let program = dir.path.join("all.blsp");
    std::fs::write(
        &program,
        "(doseq (m (reflect/builtin-modules))\n\
           (let (r (try (do (require-one (symbol m)) nil) (catch e (error-message e))))\n\
             (when r (io/puts \"FAIL \" m \" :: \" r))))\n\
         (io/puts \"ALL-MODULES-DONE\")\n",
    )
    .expect("write program");
    let cache = dir.path.join("cache");
    std::fs::create_dir_all(&cache).expect("create cache dir");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg("all.blsp")
        .current_dir(&dir.path)
        .env("XDG_CACHE_HOME", &cache)
        .env("BROOD_NO_STDIMAGE", "1")
        .env("BROOD_CONTRACTS", "1")
        // Pin the ENGINE, as this child already pins its cache, its image and its contract
        // mode. The question here is "does every module load under contracts from source?",
        // and both causes it exists for (KI-81) are `sig!`'s expansion-time calls and the
        // prelude freeze rejecting a closure over a let-bound local — neither has anything to
        // do with which engine runs the code.
        //
        // Without this the child inherits the ambient `BROOD_VM=0` from CI's
        // `differential (tree-walker)` job and re-asks the same engine-independent question at
        // the tree-walker's ~10x: a cold source boot plus every baked-in module, measured at
        // 15.5s under the VM and 80.7s under the tree-walker on an idle 12-core box, and
        // TIMING OUT at 480s on the 2-core shared runner. Contract shims still get
        // tree-walker coverage in that job through `brood_suite_passes`, which runs
        // `tests/contract_test.blsp`'s 74 cases under whatever engine is ambient.
        //
        // `BROOD_TIER=2` rather than unsetting `BROOD_VM`: it is the documented ceiling knob
        // (ADR-222) and it WINS over the `BROOD_VM` alias, so this holds whichever spelling
        // the caller's environment happens to use.
        .env("BROOD_TIER", "2");
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run brood");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("ALL-MODULES-DONE"),
        "the walk must run to completion:\n{text}"
    );
    let failed: Vec<&str> = text.lines().filter(|l| l.starts_with("FAIL ")).collect();
    assert!(
        failed.is_empty(),
        "modules that do not load under BROOD_CONTRACTS=1 from source:\n{}",
        failed.join("\n")
    );
}
