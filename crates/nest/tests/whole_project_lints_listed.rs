//! The whole-project lints (unused-private, duplicate-defs) run for `nest check FILE…` too,
//! reported for the listed files, and both forms count them (KI-149).
//!
//! The lints were whole-project passes that only the bare `nest check` ran — CI's
//! explicit-list invocation over the same files ran neither — and the bare form threw
//! their counts away, so a dead private had never failed a gate in either form. One
//! project, one dead `defn-`: the bare check, the listed check naming its file, and the
//! listed check naming ANOTHER file must agree on what they report, and the first two
//! must exit nonzero.

use std::path::Path;
use std::process::Command;

struct Run {
    out: String,
    ok: bool,
}

fn nest(dir: &Path, args: &[&str]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .args(args)
        .env("BROOD_NO_CHECK_CACHE", "1")
        .output()
        .expect("run nest");
    Run {
        out: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        ok: out.status.success(),
    }
}

#[test]
fn a_dead_private_is_reported_by_the_bare_and_the_listed_check_alike() {
    let tmp = std::env::temp_dir().join(format!("nest-ki149-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let made = nest(&tmp, &["new", "p_ki149"]);
    assert!(made.ok, "scaffold failed:\n{}", made.out);
    let root = tmp.join("p_ki149");
    let main = root.join("src").join("main.blsp");
    let mut src = std::fs::read_to_string(&main).unwrap();
    src.push_str("\n(defn- ki149-dead () 1)\n");
    std::fs::write(&main, src).unwrap();
    let other = root.join("src").join("other.blsp");
    std::fs::write(&other, "(defmodule p-ki149/other)\n(defn live () 2)\n").unwrap();

    let bare = nest(&root, &["check"]);
    assert!(
        !bare.ok,
        "the bare check must fail on a dead private:\n{}",
        bare.out
    );
    assert!(
        bare.out.contains("unused private function: ki149-dead"),
        "{}",
        bare.out
    );

    let listed = nest(&root, &["check", "src/main.blsp"]);
    assert!(
        !listed.ok,
        "the listed check naming the file must fail too (KI-149):\n{}",
        listed.out
    );
    assert!(
        listed.out.contains("unused private function: ki149-dead"),
        "{}",
        listed.out
    );

    // Listing only ANOTHER file reports nothing about this one — the verdict is computed
    // over the whole project but reported for the files asked about.
    let elsewhere = nest(&root, &["check", "src/other.blsp"]);
    assert!(elsewhere.ok, "{}", elsewhere.out);
    assert!(!elsewhere.out.contains("ki149-dead"), "{}", elsewhere.out);
    let _ = std::fs::remove_dir_all(&tmp);
}
