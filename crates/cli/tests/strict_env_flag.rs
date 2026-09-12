//! `BROOD_CHECK_STRICT=1` is the strict switch for EVERY checker entry point, not only
//! `nest check`.
//!
//! ADR-298 and the flag catalogue both name the env spelling as the way to turn strict
//! checking on, and until 2026-09-12 the only reader of it was `std/tool/nest.blsp`'s
//! `run-check` — so `brood --check`, the REPL's advisory check, the LSP and
//! `reflect/check-string-here` all ran plain with it set, and reported the plain verdict.
//! The kernel flag now reads the env itself when nothing has set it explicitly; the flag
//! catalogue's description was true and the binary made it false.
//!
//! The probe is a strict-ONLY finding: `(first xs)` over a `(vector int)` is `nil | int`,
//! which plain mode reads by overlap (silent) and strict by inclusion (a warning). A sig-typed
//! parameter would warn in both modes and prove nothing about the switch.

use std::process::Command;

const PROBE: &str = "(defn s (xs) (math/quot (first xs) 2))\n(sig s ((vector int) -> int))\n";

fn check_warnings(env: &[(&str, &str)]) -> usize {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("brood-strict-env-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("probe.blsp");
    std::fs::write(&path, PROBE).expect("write probe");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg("--check")
        .arg(&path)
        .env_remove("BROOD_CHECK_STRICT");
    for (key, value) in env {
        cmd.env(key, value);
    }
    let out = cmd.output().expect("run brood --check");
    let _ = std::fs::remove_dir_all(&dir);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    text.lines()
        .filter(|line| line.contains("warning:"))
        .count()
}

#[test]
fn brood_check_honours_the_strict_env_flag() {
    assert_eq!(
        check_warnings(&[]),
        0,
        "the probe must be a strict-ONLY finding, or it proves nothing about the switch"
    );
    assert_eq!(
        check_warnings(&[("BROOD_CHECK_STRICT", "1")]),
        1,
        "BROOD_CHECK_STRICT=1 must put `brood --check` in strict mode"
    );
    assert_eq!(
        check_warnings(&[("BROOD_CHECK_STRICT", "0")]),
        0,
        "only the spelling `1` turns strict on"
    );
}
