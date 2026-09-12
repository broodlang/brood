//! The test framework's `(:load …)` header names its WHOLE dependency closure (ADR-335).
//!
//! Since ADR-335 a qualified reference loads its module on first use. That is right for
//! programs and wrong for one process in particular: the test runner's driver runs beside
//! every `:isolated` unit, and `%isolate` is sound only while nothing else mutates globals.
//! A std module the driver first needed while a unit's snapshot was open was loaded INSIDE
//! that snapshot — by whichever process got there first — and rolled back with it, so the
//! driver died on `unbound symbol: math/max` (`test/collect-loop`) in roughly one run in
//! three of any file with an `:isolated` unit. The invariant that used to hold by accident
//! — everything the runner touches is loaded before the first isolate opens — is now
//! declared: `std/tool/test.blsp`'s header `(:load …)`s its closure.
//!
//! This test pins the list to reality. The closure is measured the one way that cannot
//! miss a body reference: a SOURCE load under the EAGER policy (`BROOD_NO_STDIMAGE=1
//! BROOD_NO_LAZY_LOAD=1`), which loads every module any body of `test` or its dependencies
//! names, transitively. A module in that closure but missing from the header is exactly a
//! module the driver can first need mid-isolate. Sabotage: drop `math` from the `(:load …)`
//! clause and this fails naming it.

use std::collections::BTreeSet;
use std::process::Command;

fn load_clause_modules(source: &str) -> BTreeSet<String> {
    let start = source
        .find("(:load ")
        .expect("std/tool/test.blsp declares a (:load …) clause");
    let rest = &source[start + "(:load ".len()..];
    let end = rest.find(')').expect("the (:load …) clause closes");
    rest[..end]
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn the_test_frameworks_load_clause_is_its_eager_closure() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../std/tool/test.blsp"
    ))
    .expect("read std/tool/test.blsp");
    let declared = load_clause_modules(&source);

    let dir = std::env::temp_dir().join(format!(
        "brood-test-closure-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let program = dir.join("closure.blsp");
    // `fs` is captured BEFORE the `io/puts` form compiles (forms compile one at a time), so
    // the probe's own `io` reference cannot add to the measurement — `io` is in the closure
    // regardless, but the measurement must not depend on that.
    std::fs::write(
        &program,
        "(require-one 'test)\n\
         (def fs (sort (map (keys *features*) ->string)))\n\
         (io/puts (string/join fs \" \"))\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        .arg(&program)
        .env("BROOD_NO_STDIMAGE", "1")
        .env("BROOD_NO_LAZY_LOAD", "1")
        .env("XDG_CACHE_HOME", &dir)
        .output()
        .expect("run brood");
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("cannot find module 'test'") {
        eprintln!("skipped: this binary has no dev-tools (`test` is not embedded)");
        return;
    }
    assert!(
        out.status.success(),
        "the probe failed: stdout={stdout:?} stderr={stderr}"
    );
    let measured: BTreeSet<String> = stdout
        .lines()
        .last()
        .unwrap_or_default()
        .split_whitespace()
        .filter(|m| *m != "test")
        .map(|s| s.to_string())
        .collect();

    let missing: Vec<&String> = measured.difference(&declared).collect();
    assert!(
        missing.is_empty(),
        "std/tool/test.blsp's (:load …) clause is missing {missing:?} — modules a source load \
         of the test framework pulls in, which the runner's driver could otherwise first need \
         while an :isolated unit has the globals snapshotted (ADR-335). Add them to the clause."
    );
}
