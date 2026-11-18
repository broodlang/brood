//! Which warnings fail `nest check` is decided by each warning's CATEGORY, under the
//! project's `:check` key and a module's `(:check-allow …)` header clause (ADR-698) — never
//! by matching the message text, which ADR-673 leaves free to change.
//!
//! The probe file has one warning the defaults call advisory (a use of a deprecated name) and
//! one they call gating (a type mismatch at a call). Every scenario runs both
//! with the check cache on — where a replayed verdict must notice that project.blsp's
//! policy moved — and off, the parallel fresh path whose workers print for themselves.

mod support;
use std::path::Path;
use std::process::Command;
use support::{TempDir, temp_dir};

const PROBE: &str = "(defn caller () (oldlib/old-way))\n\
                     (defn bad () (%string-length 3))\n";

/// The module the probe's deprecated name lives in: a deprecation is read from the loaded
/// image, so it is declared in the module the probe calls into, as deprecations are.
const OLDLIB: &str = "(defmodule oldlib)\n\
                      (defn old-way () 1)\n\
                      (meta old-way :deprecated \"0.1.0\" :use 'new-way)\n\
                      (defn new-way () 2)\n";

fn project(tag: &str) -> TempDir {
    let temp = temp_dir(tag);
    std::fs::create_dir_all(temp.path.join("src")).unwrap();
    write_main(&temp.path, "");
    temp
}

fn write_manifest(root: &Path, check: &str) {
    std::fs::write(
        root.join("project.blsp"),
        format!("(project :name \"policy\" :version \"0.1.0\" :source-paths [\"src\"] {check})\n"),
    )
    .unwrap();
}

fn write_main(root: &Path, header_clause: &str) {
    std::fs::write(root.join("src/oldlib.blsp"), OLDLIB).unwrap();
    std::fs::write(
        root.join("src/main.blsp"),
        format!("(defmodule main \"d\" {header_clause})\n{PROBE}"),
    )
    .unwrap();
}

/// `nest check` in `root`: its output and whether it passed.
fn check(root: &Path, cached: bool) -> (String, bool) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nest"));
    command.arg("check").current_dir(root);
    if cached {
        command.env_remove("BROOD_NO_CHECK_CACHE");
    } else {
        command.env("BROOD_NO_CHECK_CACHE", "1");
    }
    let output = command.output().expect("run nest check");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        output.status.success(),
    )
}

/// The printed line naming `needle`, or a panic showing the whole output.
fn line_with<'a>(output: &'a str, needle: &str) -> &'a str {
    output
        .lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no line naming {needle:?} in:\n{output}"))
}

fn scenarios(cached: bool) {
    let proj = project(if cached {
        "policy-cached"
    } else {
        "policy-fresh"
    });
    let root = &proj.path;

    // The defaults: the mismatch gates, the recursion is advisory, and each line names its
    // category — the name a `check-allow` or the `:check` key takes.
    write_manifest(root, "");
    let (output, passed) = check(root, cached);
    assert!(!passed, "a type mismatch gates by default:\n{output}");
    assert!(
        line_with(&output, "%string-length").ends_with("[type-mismatch]"),
        "{output}"
    );
    assert!(
        line_with(&output, "is deprecated").ends_with("[deprecated, advisory — not gating]"),
        "{output}"
    );

    // Demoted by the project: printed, marked, and the check passes.
    write_manifest(root, ":check {:advisory [:type-mismatch]}");
    let (output, passed) = check(root, cached);
    assert!(passed, ":advisory must stop a category gating:\n{output}");
    assert!(
        line_with(&output, "%string-length").ends_with("[type-mismatch, advisory — not gating]"),
        "{output}"
    );

    // Silenced, and the other way round: promoted.
    write_manifest(
        root,
        ":check {:allow [:type-mismatch] :gating [:deprecated]}",
    );
    let (output, passed) = check(root, cached);
    assert!(
        !output.contains("%string-length"),
        ":allow must silence:\n{output}"
    );
    assert!(
        !passed,
        ":gating must make an advisory category fail:\n{output}"
    );
    assert!(
        line_with(&output, "is deprecated").ends_with("[deprecated]"),
        "{output}"
    );

    // A typo is reported, not silently ignored.
    write_manifest(root, ":check {:allow [:type-mismatch :no-such-category]}");
    let (output, passed) = check(root, cached);
    assert!(
        !passed && line_with(&output, ":no-such-category").contains("not a warning category"),
        "{output}"
    );

    // The module's own header clause silences for that file, whatever the project says.
    write_manifest(root, "");
    write_main(root, "(:check-allow :type-mismatch)");
    let (output, passed) = check(root, cached);
    assert!(
        passed && !output.contains("%string-length"),
        "(:check-allow :type-mismatch) in the header must silence the mismatch:\n{output}"
    );
}

#[test]
fn the_category_and_the_project_policy_decide_what_gates_with_the_cache_on() {
    scenarios(true);
}

#[test]
fn the_category_and_the_project_policy_decide_what_gates_with_the_cache_off() {
    scenarios(false);
}

/// A module that calls a name it deprecates itself is not told (KI-400): a deprecation is a
/// message to its callers. The answer must not depend on the path — the cached check read the
/// deprecation from the loaded file and warned, the fresh one never loads the file and did
/// not. A caller in another module is still told, on both paths.
fn own_deprecation(cached: bool) {
    let proj = project(if cached {
        "own-deprecation-cached"
    } else {
        "own-deprecation-fresh"
    });
    let root = &proj.path;
    write_manifest(root, "");
    std::fs::write(
        root.join("src/main.blsp"),
        "(defmodule main \"d\")\n\
         (defn early () (old-way))\n\
         (defn old-way () 1)\n\
         (meta old-way :deprecated \"0.1.0\" :use 'new-way)\n\
         (defn new-way () 2)\n\
         (defn late () (old-way))\n\
         (defn named () old-way)\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/other.blsp"),
        "(defmodule other \"d\")\n(defn far () (main/old-way))\n",
    )
    .unwrap();
    let (output, passed) = check(root, cached);
    assert!(passed, "an advisory deprecation does not gate:\n{output}");
    let reported: Vec<&str> = output
        .lines()
        .filter(|line| line.contains("is deprecated"))
        .collect();
    assert_eq!(
        reported.len(),
        1,
        "exactly the other module's use is reported:\n{output}"
    );
    assert!(reported[0].contains("other.blsp"), "{output}");
}

#[test]
fn a_module_is_not_told_about_its_own_deprecation_with_the_cache_on() {
    own_deprecation(true);
}

#[test]
fn a_module_is_not_told_about_its_own_deprecation_with_the_cache_off() {
    own_deprecation(false);
}
