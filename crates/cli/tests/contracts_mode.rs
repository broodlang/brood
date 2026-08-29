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
(report \"sig-good: \" (fn () (good 7)))\n\
(report \"sig-bad: \" (fn () (bad-result 1)))\n\
(report \"op-good: \" (fn () (size 7)))\n\
(report \"op-bad: \" (fn () (size \"x\")))\n\
(report \"op-undeclared: \" (fn () (tag \"x\")))\n";

/// Run the program with a **fresh** cache dir, so the prelude is expanded from source rather
/// than replayed — the only configuration in which either KI-81 cause is reachable.
fn run(contracts: bool) -> (String, bool) {
    let dir = temp_dir(if contracts { "contracts-on" } else { "contracts-off" });
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
