//! `BROOD_FEATURES_AUDIT=1` names the write that leaves `*features*` listing a module whose
//! bindings are gone — the end state of the `[refer] … imported NOTHING` wave (KI-193).
//!
//! The probe builds that state deliberately, with KI-89's asymmetry: inside an `%isolate`, a
//! plain `def` into the module's namespace (rolled back by the restore) and a `provide` made
//! inside a load frame (journalled, so the restore replays it). The module is listed and
//! bound when the frame publishes, then listed and unbound after the restore — which is the
//! event the audit must name. Unarmed it must print nothing: the state is the same, the
//! report is opt-in.
//!
//! Sabotage-verified: with the restore's audit call removed, the armed case reports nothing.

use std::process::Command;

fn run(armed: bool) -> (String, String) {
    let dir = std::env::temp_dir().join(format!(
        "brood-features-audit-{}-{armed}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("ghost.blsp");
    std::fs::write(
        &file,
        "(%isolate (fn () (do (reflect/eval '(def ghostmod/x 1))\n\
         \x20                   (%with-load-journal (fn () (provide 'ghostmod))))))\n\
         (io/puts (str \"listed=\" (contains? *features* \"ghostmod\") \" bound=\" (bound? (symbol \"ghostmod/x\"))))\n",
    )
    .expect("write probe");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg(&file)
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env_remove("BROOD_FEATURES_AUDIT");
    if armed {
        cmd.env("BROOD_FEATURES_AUDIT", "1");
    }
    let out = cmd.output().expect("run brood");
    let _ = std::fs::remove_dir_all(&dir);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn the_audit_names_the_restore_that_left_a_module_listed_without_bindings() {
    let (stdout, stderr) = run(true);
    // The probe really built the state the audit exists for…
    assert!(
        stdout.contains("listed=true bound=false"),
        "the probe must leave `ghostmod` listed and unbound\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    // …and the audit named the event that made it, once.
    let line =
        "[features-audit] isolate restore: *features* lists `ghostmod`, whose bindings are gone";
    assert_eq!(
        stderr.matches(line).count(),
        1,
        "the armed audit must name the restore exactly once\n--- stderr ---\n{stderr}"
    );
}

#[test]
fn the_audit_is_silent_unless_armed() {
    let (stdout, stderr) = run(false);
    assert!(
        stdout.contains("listed=true bound=false"),
        "--- stdout ---\n{stdout}"
    );
    assert!(
        !stderr.contains("[features-audit]"),
        "an unarmed run must not report\n--- stderr ---\n{stderr}"
    );
}
