//! End-to-end tests for `nest complete` — the candidate engine behind
//! `nest completions <shell>`.
//!
//! The contract these pin, in priority order:
//!
//!   1. **It never fails.** Completion runs on a keypress, so whatever it is
//!      handed — no project, a corrupt manifest, hostile argument text — it must
//!      exit 0 and write nothing to stderr. A stack trace pasted into a
//!      half-typed command line is far worse than no suggestion.
//!   2. **Subcommands and flags come from clap's own model.** They are not
//!      re-listed anywhere, so these tests assert on real flags to catch the
//!      derivation breaking, not to restate the flag list.
//!   3. **Silence means "fall back".** When there is no useful candidate, printing
//!      nothing lets the shell resume filename completion, which beats a
//!      confidently wrong list.
//!
//! Runs the real `nest` binary in a child process.

use std::path::Path;
use std::process::Command;

struct Completion {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

impl Completion {
    fn lines(&self) -> Vec<&str> {
        self.stdout.lines().filter(|l| !l.is_empty()).collect()
    }
    /// Every completion invocation must satisfy this, without exception.
    fn assert_never_fails(&self, what: &str) {
        assert_eq!(self.code, Some(0), "{what}: expected exit 0");
        assert!(
            self.stderr.is_empty(),
            "{what}: expected empty stderr, got {:?}",
            self.stderr
        );
    }
}

fn complete_in(dir: &Path, words: &[&str]) -> Completion {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .arg("complete")
        .arg("--")
        .args(words)
        .output()
        .expect("run nest complete");
    Completion {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code(),
    }
}

// ── fixtures ────────────────────────────────────────────────────────────────

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
    let path = std::env::temp_dir().join(format!("brood-cmp-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    TempDir { path }
}

/// A project with two tagged test files, a source module, and one dependency.
fn project() -> TempDir {
    let tmp = tempdir("proj");
    let root = tmp.path.clone();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("project.blsp"),
        "(project :name \"cmp\" :version \"0.1.0\" :source-paths [\"src\"] \
         :test-paths [\"tests\"] :dependencies [[shared :path \"../shared\"]])\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.blsp"),
        "(defmodule main \"d\")\n\n(defn main () nil)\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/alpha_test.blsp"),
        "(defmodule alpha-test (:use test))\n\n(describe \"g\" :tags [:fast :unit]\n  \
         (test \"t\" :tags [:slow] (is true)))\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/beta_test.blsp"),
        "(defmodule beta-test (:use test))\n\n(describe \"h\" :tags [:db]\n  \
         (test \"t\" (is true)))\n",
    )
    .unwrap();
    tmp
}

// ── the never-fail contract ─────────────────────────────────────────────────

#[test]
fn completion_never_fails_however_it_is_called() {
    let proj = project();
    let bare = tempdir("bare");

    // A manifest that does not parse: every other nest command errors loudly on
    // this, and completion must not.
    let broken = tempdir("broken");
    std::fs::write(broken.path.join("project.blsp"), "(project :name \n").unwrap();

    // Argument text designed to break interpolation, paths, and the reader.
    let hostile = [
        "",
        " ",
        "-",
        "--",
        "..",
        "/etc/passwd",
        "\"",
        "\\",
        "(",
        ")",
        "#{",
        "$(id)",
        ";id",
        "x\") (print \"PWNED\") (\"",
        "日本語",
        "🔥",
    ];
    // The full matrix runs against the healthy project (where dynamic lookups
    // actually execute); the degraded contexts get a smaller sweep. Every
    // invocation is a process spawn, so the shape is chosen to keep this well
    // inside the per-test time cap while still covering each context.
    for sub in ["test", "check", "doc", "remove", "grammar", "nope", ""] {
        complete_in(&proj.path, &[sub]).assert_never_fails(sub);
        complete_in(&proj.path, &[sub, ""]).assert_never_fails(sub);
        for value in hostile {
            complete_in(&proj.path, &[sub, value]).assert_never_fails(value);
            complete_in(&proj.path, &[sub, "--only", value]).assert_never_fails(value);
        }
    }
    for dir in [&bare, &broken] {
        for sub in ["test", "remove", "doc", ""] {
            complete_in(&dir.path, &[sub, ""]).assert_never_fails(sub);
            for value in ["", "\"", "(", "$(id)", "x\") (print \"PWNED\") (\""] {
                complete_in(&dir.path, &[sub, value]).assert_never_fails(value);
                complete_in(&dir.path, &[sub, "--only", value]).assert_never_fails(value);
            }
        }
        complete_in(&dir.path, &[]).assert_never_fails("no words");
    }
}

#[test]
fn hostile_input_is_never_evaluated() {
    // The payload prints 24690 only if it is EVALUATED; an error message merely
    // echoing it cannot be mistaken for execution.
    let proj = project();
    let payload = "x\") (print (* 12345 2)) (\"";
    for words in [
        vec!["doc", payload],
        vec!["test", payload],
        vec!["remove", payload],
        vec!["test", "--only", payload],
    ] {
        let got = complete_in(&proj.path, &words);
        got.assert_never_fails("injection probe");
        assert!(
            !got.stdout.contains("24690"),
            "payload was evaluated for {words:?}"
        );
    }
}

// ── static candidates, derived from clap ────────────────────────────────────

#[test]
fn subcommands_are_offered_and_prefix_filtered() {
    let proj = project();
    let all = complete_in(&proj.path, &[""]);
    all.assert_never_fails("subcommands");
    for expected in ["test", "check", "run", "format", "doc", "completions"] {
        assert!(all.lines().contains(&expected), "missing {expected}");
    }
    // `complete` itself is hidden, so it must not be suggested.
    assert!(!all.lines().contains(&"complete"));

    let filtered = complete_in(&proj.path, &["te"]);
    assert_eq!(filtered.lines(), vec!["test"]);
}

#[test]
fn flags_come_from_the_argument_model() {
    let proj = project();
    let got = complete_in(&proj.path, &["test", "--co"]);
    got.assert_never_fails("flags");
    let lines = got.lines();
    assert!(lines.contains(&"--cover"), "got {lines:?}");
    assert!(lines.contains(&"--cover-min"), "got {lines:?}");
    // A flag of a DIFFERENT subcommand must not leak in.
    assert!(!lines.contains(&"--changed"), "got {lines:?}");
}

#[test]
fn value_enum_positionals_come_from_the_enum() {
    let proj = project();
    let grammar = complete_in(&proj.path, &["grammar", ""]);
    grammar.assert_never_fails("grammar");
    assert!(grammar.lines().contains(&"emacs"), "{:?}", grammar.lines());

    let shells = complete_in(&proj.path, &["completions", ""]);
    let lines = shells.lines();
    for shell in ["bash", "zsh", "fish"] {
        assert!(lines.contains(&shell), "missing {shell} in {lines:?}");
    }
}

// ── dynamic, project-aware candidates ──────────────────────────────────────

#[test]
fn selectors_offer_every_declared_tag_plus_the_pseudo_prefixes() {
    let proj = project();
    let got = complete_in(&proj.path, &["test", "--only", ""]);
    got.assert_never_fails("selectors");
    let lines = got.lines();
    // Tags from BOTH describe-level and test-level, across BOTH files.
    for tag in ["fast", "unit", "slow", "db"] {
        assert!(lines.contains(&tag), "missing tag {tag} in {lines:?}");
    }
    assert!(lines.contains(&"test:"), "{lines:?}");
    assert!(lines.contains(&"describe:"), "{lines:?}");
}

#[test]
fn selector_candidates_are_prefix_filtered() {
    let proj = project();
    let got = complete_in(&proj.path, &["test", "--only", "sl"]);
    assert_eq!(got.lines(), vec!["slow"]);
}

#[test]
fn exclude_and_include_share_the_selector_source() {
    let proj = project();
    for flag in ["--exclude", "--include"] {
        let got = complete_in(&proj.path, &["test", flag, "db"]);
        got.assert_never_fails(flag);
        assert_eq!(got.lines(), vec!["db"], "{flag} should offer tags too");
    }
}

#[test]
fn test_positional_offers_test_files() {
    let proj = project();
    let got = complete_in(&proj.path, &["test", ""]);
    got.assert_never_fails("test files");
    let lines = got.lines();
    assert!(lines.contains(&"tests/alpha_test.blsp"), "{lines:?}");
    assert!(lines.contains(&"tests/beta_test.blsp"), "{lines:?}");
    // Not a test file, so not a candidate here.
    assert!(!lines.contains(&"src/main.blsp"), "{lines:?}");
}

#[test]
fn check_positional_offers_every_blsp_including_sources() {
    let proj = project();
    let got = complete_in(&proj.path, &["check", ""]);
    got.assert_never_fails("blsp files");
    assert!(got.lines().contains(&"src/main.blsp"), "{:?}", got.lines());
}

#[test]
fn remove_offers_declared_dependencies() {
    let proj = project();
    let got = complete_in(&proj.path, &["remove", ""]);
    got.assert_never_fails("deps");
    // Declared in the manifest but NOT fetched — completion reads the manifest as
    // data, so an unfetched dep is still offered.
    assert_eq!(got.lines(), vec!["shared"]);
}

#[test]
fn doc_offers_baked_in_modules() {
    let proj = project();
    let got = complete_in(&proj.path, &["doc", "js"]);
    got.assert_never_fails("modules");
    assert!(got.lines().contains(&"json"), "{:?}", got.lines());
}

// ── fallback behaviour ─────────────────────────────────────────────────────

#[test]
fn outside_a_project_dynamic_values_are_silent_not_wrong() {
    let bare = tempdir("bare2");
    for words in [
        vec!["test", ""],
        vec!["remove", ""],
        vec!["test", "--only", ""],
    ] {
        let got = complete_in(&bare.path, &words);
        got.assert_never_fails("outside a project");
        // Silence is the contract: the shell then falls back to filenames.
        assert!(
            got.lines().is_empty() || !got.lines().contains(&"tests/alpha_test.blsp"),
            "{words:?} leaked candidates from nowhere: {:?}",
            got.lines()
        );
    }
    // Static candidates still work with no project at all.
    assert!(complete_in(&bare.path, &["te"]).lines().contains(&"test"));
}

#[test]
fn a_broken_manifest_still_completes_flags() {
    let broken = tempdir("broken2");
    std::fs::write(broken.path.join("project.blsp"), "(project :name \n").unwrap();
    // The manifest is unparseable — every other command errors on it. Flags are
    // derived from clap, so they must be unaffected.
    let got = complete_in(&broken.path, &["test", "--cov"]);
    got.assert_never_fails("broken manifest");
    assert!(got.lines().contains(&"--cover"), "{:?}", got.lines());
}

#[test]
fn an_argument_with_no_known_value_kind_stays_silent() {
    let proj = project();
    // `--seed` takes a number: there is nothing to suggest, so printing nothing
    // lets the shell fall back rather than offering something wrong.
    let got = complete_in(&proj.path, &["test", "--seed", ""]);
    got.assert_never_fails("--seed");
    assert!(got.lines().is_empty(), "{:?}", got.lines());
}

// ── the emitted shell scripts ──────────────────────────────────────────────

fn completions_for(shell: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .args(["completions", shell])
        .output()
        .expect("run nest completions");
    assert!(out.status.success(), "nest completions {shell} failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn every_shell_script_delegates_to_nest_complete() {
    for shell in ["bash", "zsh", "fish"] {
        let script = completions_for(shell);
        assert!(
            script.contains("nest complete"),
            "{shell} script should call `nest complete`"
        );
        assert!(
            script.contains("2>/dev/null"),
            "{shell} script should discard stderr so a diagnostic can't corrupt the candidate list"
        );
    }
}

#[test]
fn the_zsh_script_does_not_shadow_the_words_array() {
    // Regression: `local -a words` blanked zsh's own completion-context `$words`
    // before it could be read, so every completion saw an empty command line.
    let script = completions_for("zsh");
    assert!(
        !script.contains("local -a words"),
        "zsh script must not declare a local named `words`"
    );
    assert!(script.contains("$CURRENT"), "zsh script should use $CURRENT");
}

#[test]
fn the_bash_script_keeps_filename_fallback() {
    let script = completions_for("bash");
    assert!(
        script.contains("-o default"),
        "bash script should fall back to filename completion"
    );
}
