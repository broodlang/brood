//! A stale binary cannot check silently (B7, 2026-09-17).
//!
//! `nest check` resolves every `:use`d std module from the binary's baked-in `std/`, so a
//! `nest` built before a `sig` edit checks every caller against the OLD declaration — and a
//! verdict from the wrong std is wrong in both directions while reading exactly like a right
//! one. So inside a checkout whose `std/**/*.blsp` hashes differently from what the binary
//! baked in, the checker declines with the reason and exit 2; outside any checkout (an
//! installed binary in a user's project) nothing is stale and it runs.
//!
//! A checkout, to `cli_support::repo_root`, is the nearest ancestor holding a `Cargo.toml`
//! file and a `std/` directory — so a temp dir with both, and one `.blsp` under `std/` that
//! the binary certainly did not bake in, IS a stale checkout. That is the whole fixture.

use std::path::Path;
use std::process::Command;

struct TempDir {
    path: std::path::PathBuf,
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn tempdir(tag: &str) -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "brood-stale-binary-{tag}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    TempDir { path }
}

/// One clean project file to check, so a run that is NOT refused has something to say.
fn project_with_a_clean_file(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("project.blsp"),
        "(project :name \"fresh\" :version \"0.1.0\" :source-paths [\"src\"])\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/fresh.blsp"),
        "(defmodule fresh)\n\n(defn live (x) (+ x 1))\n",
    )
    .unwrap();
}

/// Turn `root` into what `repo_root` recognises as a brood checkout, with a `std/` the
/// binary never saw.
fn make_a_stale_checkout(root: &Path) {
    std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    std::fs::create_dir_all(root.join("std")).unwrap();
    std::fs::write(
        root.join("std/not-baked-in.blsp"),
        "(defmodule not-baked-in)\n(defn nope () nil)\n",
    )
    .unwrap();
}

struct Out {
    text: String,
    code: Option<i32>,
}

fn run(binary: &str, dir: &Path, args: &[&str]) -> Out {
    let out = Command::new(binary)
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run the binary");
    Out {
        text: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        code: out.status.code(),
    }
}

fn assert_refused(out: &Out, command: &str) {
    assert_eq!(
        out.code,
        Some(2),
        "{command} should exit 2 when stale:\n{}",
        out.text
    );
    assert!(
        out.text.contains("baked-in std/ is OLDER than") && out.text.contains("refused"),
        "{command} must name the staleness and the refusal:\n{}",
        out.text
    );
}

#[test]
fn nest_check_refuses_inside_a_checkout_whose_std_it_did_not_bake_in() {
    let dir = tempdir("nest");
    project_with_a_clean_file(&dir.path);
    // Control first: outside any checkout the same project checks clean.
    let fresh = run(env!("CARGO_BIN_EXE_nest"), &dir.path, &["check"]);
    assert_eq!(
        fresh.code,
        Some(0),
        "the control must check clean:\n{}",
        fresh.text
    );
    assert!(!fresh.text.contains("OLDER than"), "{}", fresh.text);

    make_a_stale_checkout(&dir.path);
    let stale = run(env!("CARGO_BIN_EXE_nest"), &dir.path, &["check"]);
    assert_refused(&stale, "nest check");
}

#[test]
fn a_stale_checkout_is_found_from_a_subdirectory_too() {
    // `repo_root` walks up, the way a developer runs `nest check` from anywhere in a tree.
    let dir = tempdir("nested");
    make_a_stale_checkout(&dir.path);
    let inner = dir.path.join("examples/deep");
    std::fs::create_dir_all(&inner).unwrap();
    project_with_a_clean_file(&inner);
    let stale = run(env!("CARGO_BIN_EXE_nest"), &inner, &["check"]);
    assert_refused(&stale, "nest check");
}

#[test]
fn brood_check_refuses_the_same_way() {
    let dir = tempdir("brood");
    project_with_a_clean_file(&dir.path);
    let brood = Path::new(env!("CARGO_BIN_EXE_nest"))
        .with_file_name("brood")
        .to_string_lossy()
        .into_owned();
    if !Path::new(&brood).is_file() {
        eprintln!("no `brood` beside `nest` — skipping the brood --check half");
        return;
    }
    let fresh = run(&brood, &dir.path, &["--check", "src/fresh.blsp"]);
    assert_eq!(
        fresh.code,
        Some(0),
        "the control must check clean:\n{}",
        fresh.text
    );

    make_a_stale_checkout(&dir.path);
    let stale = run(&brood, &dir.path, &["--check", "src/fresh.blsp"]);
    assert_refused(&stale, "brood --check");
}

#[test]
fn nest_test_still_warns_rather_than_refuses() {
    // The test runner's result from an older binary is usually still right, so it keeps
    // the warning (and its exit code); only the checker's verdict is refused. Pinned so the
    // two policies cannot silently become one.
    let dir = tempdir("test");
    project_with_a_clean_file(&dir.path);
    std::fs::create_dir_all(dir.path.join("tests")).unwrap();
    std::fs::write(
        dir.path.join("tests/fresh_test.blsp"),
        "(defmodule fresh-test (:use test) (:use fresh))\n\n\
         (describe \"fresh\"\n  (test \"live\" (assert= (live 1) 2)))\n",
    )
    .unwrap();
    std::fs::write(
        dir.path.join("project.blsp"),
        "(project :name \"fresh\" :version \"0.1.0\" \
         :source-paths [\"src\"] :test-paths [\"tests\"])\n",
    )
    .unwrap();
    make_a_stale_checkout(&dir.path);
    let out = run(env!("CARGO_BIN_EXE_nest"), &dir.path, &["test"]);
    assert_eq!(out.code, Some(0), "{}", out.text);
    assert!(
        out.text.contains("baked-in std/ is OLDER than"),
        "{}",
        out.text
    );
    assert!(!out.text.contains("refused"), "{}", out.text);
}
