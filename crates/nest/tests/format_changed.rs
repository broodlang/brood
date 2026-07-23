//! End-to-end test for `nest format --changed` (the git-aware narrower scope):
//! only `.blsp` files git reports as not-committed-clean are formatted, a clean
//! tree formats nothing, and a non-git dir falls back to the whole project.
//!
//! Runs the real `nest` binary in a child process. Skips gracefully if `git`
//! is unavailable.

use std::path::Path;
use std::process::Command;

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run `git` in `dir` with the machine's system/global config neutralized
/// (no credential helper, no signing) so a commit works in CI and on dev
/// machines with a fancy git setup.
fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .args(["-c", "credential.helper=", "-c", "commit.gpgsign=false"])
        .args(args)
        .status()
        .expect("run git")
        .success();
    assert!(ok, "git {args:?} failed");
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
}

fn nest_format_changed(dir: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .args(["format", "--changed"])
        .output()
        .expect("run nest");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn format_changed_only_touches_git_changed_files() {
    if !have_git() {
        eprintln!("skipping: git not available");
        return;
    }
    let tmp = tempdir();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    write(&root.join("project.blsp"), "(project :name scratch)\n");
    // Already well-formatted, so a whole-project format would rewrite 0 either
    // way — the discriminator is the "considered" COUNT.
    write(&root.join("src/a.blsp"), "(defn a () 1)\n");
    write(&root.join("src/b.blsp"), "(defn b () 2)\n");
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "t@t"]);
    git(root, &["config", "user.name", "t"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "init"]);

    // Clean tree: 0 files considered — NOT a whole-project fallback.
    let clean = nest_format_changed(root);
    assert!(
        clean.contains("0 changed files considered"),
        "clean tree should consider 0 changed files, got:\n{clean}"
    );

    // Dirty exactly one file, badly formatted. --changed must format just it.
    write(&root.join("src/a.blsp"), "(defn   a   ()   1)\n");
    let dirty = nest_format_changed(root);
    assert!(
        dirty.contains("1 changed file considered"),
        "one dirty file should be the only one considered, got:\n{dirty}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/a.blsp")).unwrap(),
        "(defn a () 1)\n",
        "the changed file should have been reformatted"
    );
    // b.blsp (committed-clean, untouched) is never considered — proven by the
    // count above; also confirm it wasn't rewritten.
    assert_eq!(
        std::fs::read_to_string(root.join("src/b.blsp")).unwrap(),
        "(defn b () 2)\n"
    );

    // Regression: a brand-new file in a brand-new (wholly-untracked) directory
    // must be seen. Plain `git status --porcelain` collapses that to `?? dir/`;
    // `-uall` lists the file, so `--changed` catches it.
    std::fs::create_dir_all(root.join("src/fresh")).unwrap();
    write(&root.join("src/fresh/c.blsp"), "(defn   c   () 3)\n");
    let fresh = nest_format_changed(root);
    assert!(
        fresh.contains("1 changed file considered"),
        "a new file in a new untracked dir must be considered, got:\n{fresh}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/fresh/c.blsp")).unwrap(),
        "(defn c () 3)\n",
        "the new-dir file should have been reformatted"
    );
}

#[test]
fn format_changed_falls_back_outside_a_git_repo() {
    let tmp = tempdir();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    write(&root.join("project.blsp"), "(project :name scratch)\n");
    write(&root.join("src/a.blsp"), "(defn   a   () 1)\n");
    let out = nest_format_changed(root);
    assert!(
        out.contains("not a git repository"),
        "outside a git repo it should fall back to the whole project, got:\n{out}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/a.blsp")).unwrap(),
        "(defn a () 1)\n",
        "the fallback whole-project format should still reformat the file"
    );
}

/// Minimal unique temp dir (avoids a dev-dependency just for this).
fn tempdir() -> TempDir {
    let base = std::env::temp_dir();
    let pid = std::process::id();
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = base.join(format!("brood-fmt-{pid}-{n}"));
    std::fs::create_dir_all(&path).unwrap();
    TempDir { path }
}

struct TempDir {
    path: std::path::PathBuf,
}
impl TempDir {
    fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
