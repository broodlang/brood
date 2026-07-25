//! `nest new` must produce a project that is immediately healthy.
//!
//! This is the check nothing else makes: unit tests cover the template *strings*,
//! but nobody was scaffolding a project and then running the toolchain over it. As
//! a result every template shipped code that `nest format --check` rejected on the
//! first run — so a brand-new project failed its own CI format gate, and the
//! starter code modelled non-canonical style to every new user.
//!
//! For each template, scaffold into a temp dir and assert the result is
//! format-clean, check-clean, and its bundled tests pass.
//!
//! Deliberately excluded: `hatch` and `web-api`. Those scaffold `:path`
//! dependencies on sibling checkouts (`../hatch`) that do not exist in a temp dir,
//! so every command correctly fails on the unresolved dep — nothing to do with
//! template quality. See `scaffolded_hatch_template_explains_its_missing_dep`.

use std::path::Path;
use std::process::Command;

/// Templates that scaffold a self-contained project (no external path deps).
const SELF_CONTAINED: &[&str] = &["default", "tui-loop", "gen", "editor", "gui"];

struct TempDir {
    path: std::path::PathBuf,
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn tempdir(tag: &str) -> TempDir {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("brood-scaf-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    TempDir { path }
}

struct Run {
    out: String,
    ok: bool,
}

fn nest(dir: &Path, args: &[&str]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .args(args)
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

/// Scaffold `template` into a fresh temp dir; returns (dir, project root).
fn scaffold(template: &str) -> (TempDir, std::path::PathBuf) {
    let tmp = tempdir(template);
    let name = format!("p_{}", template.replace('-', "_"));
    let made = nest(&tmp.path, &["new", &name, "--template", template]);
    assert!(made.ok, "nest new {template} failed: {}", made.out);
    let root = tmp.path.join(&name);
    (tmp, root)
}

#[test]
fn every_self_contained_template_scaffolds_format_clean() {
    for template in SELF_CONTAINED {
        let (_tmp, root) = scaffold(template);
        let checked = nest(&root, &["format", "--check"]);
        assert!(
            checked.ok,
            "`nest new --template {template}` produces source that `nest format --check` \
             rejects, so a brand-new project fails its own format gate:\n{}",
            checked.out
        );
    }
}

#[test]
fn every_self_contained_template_scaffolds_check_clean() {
    for template in SELF_CONTAINED {
        let (_tmp, root) = scaffold(template);
        let checked = nest(&root, &["check"]);
        assert!(
            checked.ok,
            "`nest new --template {template}` produces source with checker warnings:\n{}",
            checked.out
        );
    }
}

#[test]
fn every_self_contained_template_ships_passing_tests() {
    for template in SELF_CONTAINED {
        let (_tmp, root) = scaffold(template);
        let tested = nest(&root, &["test"]);
        assert!(
            tested.ok,
            "`nest new --template {template}` ships failing tests:\n{}",
            tested.out
        );
        // A template with no tests at all would pass the line above vacuously.
        assert!(
            tested.out.contains(" passed,"),
            "{template}: expected a test summary, got:\n{}",
            tested.out
        );
    }
}

/// Regression: a comment trailing a form *inside* a list is re-emitted by the
/// formatter on its own line, which hoisted `; the app framework …` notes out of
/// their `defmodule` and stranded them above the whole form — describing the wrong
/// thing. Templates must put such comments above the clause to begin with.
#[test]
fn no_template_leaks_a_hoisted_comment_above_its_first_form() {
    for template in SELF_CONTAINED {
        let (_tmp, root) = scaffold(template);
        let main = std::fs::read_to_string(root.join("src/main.blsp")).unwrap();
        let first = main.lines().next().unwrap_or("");
        assert!(
            !first.trim_start().starts_with(';'),
            "{template}: src/main.blsp starts with a comment, which means the formatter \
             hoisted one out of a form — put it above the clause in the template.\nGot: {first}"
        );
        assert!(
            first.starts_with("(defmodule"),
            "{template}: src/main.blsp should open with its defmodule, got: {first}"
        );
    }
}

/// The hatch templates depend on sibling checkouts. That is by design, but the
/// failure a user meets when they aren't present should name the missing dep — it
/// is the first thing that happens after `nest new`, so it has to be legible.
#[test]
fn scaffolded_hatch_template_explains_its_missing_dep() {
    let (_tmp, root) = scaffold("web-api");
    let checked = nest(&root, &["check"]);
    assert!(
        !checked.ok,
        "expected the unresolved sibling dep to fail; if hatch is now bundled, \
         move `web-api` into SELF_CONTAINED"
    );
    assert!(
        checked.out.contains("hatch"),
        "the error should name the missing dependency:\n{}",
        checked.out
    );
}

/// The scaffolder must not print a "Next:" step that cannot work. The hatch
/// templates depend on sibling checkouts, so the generic
/// `Next: cd x && nest test && nest run` was a promise the project could not keep —
/// it was the first thing a user saw after `nest new`, and running it failed.
#[test]
fn next_steps_are_accurate_per_template() {
    // Self-contained: the generic next step really works.
    let tmp = tempdir("next-default");
    let made = nest(&tmp.path, &["new", "nx", "--template", "default"]);
    assert!(made.ok, "{}", made.out);
    assert!(
        made.out.contains("nest test && nest run"),
        "a self-contained template should suggest test+run:\n{}",
        made.out
    );

    // Sibling-dep templates: name the prerequisite instead of promising test+run.
    for (template, expect_deps) in [
        ("hatch", vec!["../hatch", "../store-postgres"]),
        ("web-api", vec!["../hatch"]),
    ] {
        let tmp = tempdir(&format!("next-{template}"));
        let made = nest(&tmp.path, &["new", "nx", "--template", template]);
        assert!(made.ok, "{}", made.out);
        for dep in &expect_deps {
            assert!(
                made.out.contains(dep),
                "{template} should name its sibling dep {dep}:\n{}",
                made.out
            );
        }
        assert!(
            !made.out.contains("nest test && nest run"),
            "{template} must not promise test+run, which cannot work without the siblings:\n{}",
            made.out
        );
    }
}

/// Singular vs plural in that message — one dep must not read "as sibling path deps".
#[test]
fn the_sibling_dep_message_agrees_in_number() {
    let tmp = tempdir("plural");
    let made = nest(&tmp.path, &["new", "nx", "--template", "web-api"]);
    assert!(made.ok, "{}", made.out);
    assert!(
        made.out.contains("as a sibling path dep,") && made.out.contains("until it is"),
        "a single dep should read in the singular:\n{}",
        made.out
    );
}
