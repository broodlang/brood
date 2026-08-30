//! Concurrent `nest add` / `nest remove` must not lose an edit.
//!
//! Before `%file-swap`, editing `project.blsp` was a plain read-modify-write: both
//! processes read the original, both spliced their entry in, and the second write
//! erased the first — while *both* printed `package: added …`. Measured at the time:
//! three concurrent adds landed between one and three of them.
//!
//! The fix is a locked compare-and-swap (`package-edit-manifest`): the write only
//! lands if the file still holds exactly what was read, and otherwise the edit is
//! recomputed against the new content. These tests are the regression, and they are
//! written to *fail* on the old behaviour rather than merely exercise the new one —
//! the assertion is that every command reporting success actually landed.

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
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("brood-race-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    TempDir { path }
}

/// A minimal dependency project, so `add` can actually resolve it.
fn write_dep(root: &Path, name: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("project.blsp"),
        format!("(project :name \"{name}\" :version \"0.1.0\")\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join(format!("src/{name}.blsp")),
        format!("(defmodule {name} \"lib\")\n\n(defn {name}-f () 1)\n"),
    )
    .unwrap();
}

/// A host project with `count` sibling deps available to add.
fn workspace(tag: &str, deps: &[&str]) -> (TempDir, std::path::PathBuf) {
    let tmp = tempdir(tag);
    for d in deps {
        write_dep(&tmp.path, d);
    }
    let host = tmp.path.join("host");
    std::fs::create_dir_all(host.join("src")).unwrap();
    std::fs::create_dir_all(host.join("tests")).unwrap();
    std::fs::write(
        host.join("project.blsp"),
        "(project :name \"host\" :version \"0.1.0\" :source-paths [\"src\"] \
         :test-paths [\"tests\"])\n",
    )
    .unwrap();
    std::fs::write(
        host.join("src/main.blsp"),
        "(defmodule main \"d\")\n\n(defn main () nil)\n",
    )
    .unwrap();
    (tmp, host)
}

/// Run `nest add`/`remove` concurrently; returns each command's combined output.
fn concurrently(host: &Path, commands: &[Vec<String>]) -> Vec<String> {
    let handles: Vec<_> = commands
        .iter()
        .map(|args| {
            let host = host.to_path_buf();
            let args = args.clone();
            std::thread::spawn(move || {
                let out = Command::new(env!("CARGO_BIN_EXE_nest"))
                    .current_dir(&host)
                    .args(&args)
                    .output()
                    .expect("run nest");
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                )
            })
        })
        .collect();
    handles.into_iter().map(|h| h.join().unwrap()).collect()
}

fn manifest(host: &Path) -> String {
    std::fs::read_to_string(host.join("project.blsp")).unwrap()
}

/// Is `dep` present as a `:dependencies` entry (`[dep :path …]`)?
fn has_dep(host: &Path, dep: &str) -> bool {
    manifest(host).contains(&format!("[{dep} "))
}

fn nest(host: &Path, args: &[&str]) -> bool {
    Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(host)
        .args(args)
        .output()
        .expect("run nest")
        .status
        .success()
}

#[test]
fn concurrent_adds_do_not_lose_an_edit() {
    let deps = ["d1", "d2", "d3", "d4"];
    let (_tmp, host) = workspace("adds", &deps);

    let commands: Vec<Vec<String>> = deps
        .iter()
        .map(|d| vec!["add".into(), (*d).into(), ":path".into(), format!("../{d}")])
        .collect();
    let outputs = concurrently(&host, &commands);

    // The property that was broken: every command that CLAIMED success must have
    // landed. (Not "all four succeed" — a legitimate failure is fine, a false
    // success is not.)
    for (dep, out) in deps.iter().zip(&outputs) {
        if out.contains("package: added") {
            assert!(
                has_dep(&host, dep),
                "`nest add {dep}` reported success but is absent from the manifest \
                 — an edit was lost.\nits output: {out}\nmanifest:\n{}",
                manifest(&host)
            );
        }
    }
    // And the manifest must still be usable afterwards.
    assert!(
        nest(&host, &["tree"]),
        "the manifest no longer parses after concurrent adds:\n{}",
        manifest(&host)
    );
}

#[test]
fn concurrent_adds_all_succeed_in_practice() {
    // Distinct deps with no reason to conflict: with the CAS retry, all of them
    // should land. This is the stronger, empirical form of the test above — it is
    // what actually caught the regression (1-3 of 3 landing before the fix).
    let deps = ["d1", "d2", "d3"];
    let (_tmp, host) = workspace("all", &deps);
    let commands: Vec<Vec<String>> = deps
        .iter()
        .map(|d| vec!["add".into(), (*d).into(), ":path".into(), format!("../{d}")])
        .collect();
    let outputs = concurrently(&host, &commands);
    for (dep, out) in deps.iter().zip(&outputs) {
        assert!(
            has_dep(&host, dep),
            "{dep} did not land under concurrency.\nits output: {out}\nmanifest:\n{}",
            manifest(&host)
        );
    }
}

#[test]
fn a_concurrent_remove_does_not_resurrect_or_erase_an_add() {
    let deps = ["d1", "d2", "d3"];
    let (_tmp, host) = workspace("mixed", &deps);
    // Start with d1 present, then remove it while adding d2 and d3 alongside.
    assert!(nest(&host, &["add", "d1", ":path", "../d1"]));

    concurrently(
        &host,
        &[
            vec!["remove".into(), "d1".into()],
            vec!["add".into(), "d2".into(), ":path".into(), "../d2".into()],
            vec!["add".into(), "d3".into(), ":path".into(), "../d3".into()],
        ],
    );

    let text = manifest(&host);
    assert!(!has_dep(&host, "d1"), "the removal was lost:\n{text}");
    assert!(
        has_dep(&host, "d2"),
        "an add was lost by the removal:\n{text}"
    );
    assert!(
        has_dep(&host, "d3"),
        "an add was lost by the removal:\n{text}"
    );
    assert!(nest(&host, &["tree"]), "manifest unusable:\n{text}");
}

#[test]
fn the_lock_lives_outside_the_project_tree() {
    // The lock file is derived state, so it belongs with the caches, not next to the
    // user's source — nothing new should appear in the project after an edit.
    let (_tmp, host) = workspace("lockloc", &["d1"]);
    let before: std::collections::BTreeSet<String> = std::fs::read_dir(&host)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(nest(&host, &["add", "d1", ":path", "../d1"]));
    let after: std::collections::BTreeSet<String> = std::fs::read_dir(&host)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let added: Vec<_> = after.difference(&before).collect();
    // `project.lock.blsp` is the expected new file; a `.lock` or `.swap.` temp is not.
    for name in &added {
        assert!(
            name.as_str() == "project.lock.blsp",
            "unexpected file left in the project by `nest add`: {name}"
        );
    }
}
