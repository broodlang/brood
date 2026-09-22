//! **Unarmed, the kernel does not consult the contract policy at all (KI-182).**
//!
//! `contract_apply` runs at every `def` of a name with a declared arrow and used to apply
//! the Brood hook `%contract-wrap` unconditionally, leaving the decision to the hook — which,
//! unarmed and not `sig!`-forced, always declines. That is a Brood call per declared
//! definition at load time: while `io` materialised, the hook's `not` crossed the JIT's tier
//! threshold and every short `brood file` run instantiated Cranelift at boot to compile it —
//! `startup` +6% at the 422c92a5 benchmark refresh. The kernel now hoists the hook's own first
//! test: unarmed and not forced, the policy is not called.
//!
//! The hook is a reserved prelude name, so the program observes it through
//! `%load-module-source` — the loader that holds the reserved-name exemption — wrapping the
//! original in a closure that records each name it is offered. Three names: two plain `sig`s
//! (one above its `defn`, one below — both go through the same offer) and one `sig!`.
//!
//! Unarmed: only the forced name is offered, and only it enforces. Armed (`BROOD_CONTRACTS=1`,
//! the `nest run`/`nest test` default): all three are offered and all three enforce — the
//! same program, so a guard that reads `false` for a hook that was never wired would fail
//! the armed half.
//!
//! The second caller had the same shape: `impl` wrapped every op body under a declared
//! return in `%contract-check-op-result`, a Brood function whose first test was
//! `(not (%contracts-armed?))` — a call plus a `not` per ability-op result, armed or not.
//! The emission now asks the cached-bool primitive inline and enters the checking function
//! only armed. Same probe: the checker is rebound to record the op it is asked about.

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

const PROGRAM: &str = "\
(def %kw-calls (%table))\n\
(def %kw-orig %contract-wrap)\n\
(%load-module-source \"(def %contract-wrap (fn (name orig type) (%table-put %kw-calls name true) (%kw-orig name orig type)))\" \"offer-probe\")\n\
(defn f (n) n)\n\
(sig f (int -> int))\n\
(sig above (int -> int))\n\
(defn above (n) n)\n\
(defn g (n) n)\n\
(sig! g (int -> int))\n\
(io/puts (str \"offered f=\" (%table-has? %kw-calls 'f) \" above=\" (%table-has? %kw-calls 'above) \" g=\" (%table-has? %kw-calls 'g)))\n\
(io/puts (str \"g-bad: \" (try (g \"x\") (catch e (str \"RAISED \" (get e :kind))))))\n\
(io/puts (str \"f-bad: \" (try (f \"x\") (catch e (str \"RAISED \" (get e :kind))))))\n\
(def %op-calls (%table))\n\
(def %op-orig %contract-check-op-result)\n\
(%load-module-source \"(def %contract-check-op-result (fn (a op ret v) (%table-put %op-calls op true) (%op-orig a op ret v)))\" \"op-probe\")\n\
(defability Sz (size [self] :-> int))\n\
(impl Sz :int (size [n] n))\n\
(impl Sz :string (size [s] \"not an int\"))\n\
(io/puts (str \"size-int=\" (size 7) \" op-checked=\" (%table-has? %op-calls 'size)))\n\
(io/puts (str \"size-bad: \" (try (size \"x\") (catch e (str \"RAISED \" (get e :kind))))))\n";

fn run(contracts: bool) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = TempDir {
        path: std::env::temp_dir().join(format!(
            "brood-contract-offer-{}-{nanos}",
            std::process::id()
        )),
    };
    std::fs::create_dir_all(&dir.path).expect("create temp dir");
    let program = dir.path.join("program.blsp");
    std::fs::write(&program, PROGRAM).expect("write program");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg("program.blsp")
        .current_dir(&dir.path)
        .env("BROOD_NO_CHECK", "1");
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
    assert!(
        out.status.success(),
        "the program should run to completion:\n{text}"
    );
    text
}

#[test]
fn unarmed_only_a_forced_name_reaches_the_policy() {
    let text = run(false);
    assert!(
        text.contains("offered f=false above=false g=true"),
        "unarmed, a plain `sig` (above or below its defn) must not call the hook; `sig!` must:\n{text}"
    );
    assert!(
        text.contains("g-bad: RAISED :contract") && text.contains("f-bad: x"),
        "the forced name still enforces unarmed and the plain one still flows through:\n{text}"
    );
    assert!(
        text.contains("size-int=7 op-checked=false") && text.contains("size-bad: not an int"),
        "unarmed, an ability op's result never enters the checking function:\n{text}"
    );
}

#[test]
fn armed_every_declared_name_reaches_the_policy() {
    let text = run(true);
    assert!(
        text.contains("offered f=true above=true g=true"),
        "armed, every declared def is offered — the probe's hook is wired:\n{text}"
    );
    assert!(
        text.contains("g-bad: RAISED :contract") && text.contains("f-bad: RAISED :contract"),
        "armed, both enforce:\n{text}"
    );
    assert!(
        text.contains("size-int=7 op-checked=true") && text.contains("size-bad: RAISED :contract"),
        "armed, the op result is checked and a wrong one raises:\n{text}"
    );
}
