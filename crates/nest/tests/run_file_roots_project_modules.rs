//! KI-129: a script OUTSIDE `src/` run with `nest run FILE` loads a project module once,
//! rooted — never a second time bare.
//!
//! `nest run FILE` runs `project/setup` (which installs the project's package context,
//! ADR-070), pre-flight-checks the file — the checker's `(:use model)` roots to
//! `proj/model` and loads it — and then launches the file as its own green process
//! through `spawn_root_program`. That process was built WITHOUT the package context the
//! spawn path (`spawn_impl`) already inherits, so the file's own `(:use model)` evaluated
//! at root, `require-one 'model` found `src/model.blsp` on the load path and evaluated it
//! again under the bare name. Every `deftype` in it was then registered under two names,
//! and `annot::alias_ty`'s unique-suffix rule — two candidates decline, by design
//! (ADR-327) — read the bare `model` as an unknown type.
//!
//! Nothing in the project's own gates runs a file the project does not own: `nest test`
//! loads through the project loader, `nest run` of the project runs `:main`, and the
//! checker checks files IN the project. A scratchpad probe against bedit was the first
//! reader. The guard asserts on the two things a reader sees: `*features*` holds the
//! rooted name ONLY, and a `sig` naming the alias resolves.

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

fn fixture() -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("brood-ki129-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(path.join("proj/src")).unwrap();
    std::fs::write(
        path.join("proj/project.blsp"),
        "(project :name \"proj\" :version \"0.1.0\" :source-paths [\"src\"])\n",
    )
    .unwrap();
    std::fs::write(
        path.join("proj/src/model.blsp"),
        "(defmodule model)\n\
         (deftype model (record :x int))\n\
         (defn mk (x) {:x x})\n\
         (sig mk (int -> model))\n",
    )
    .unwrap();
    // The script lives BESIDE the project, not in it.
    std::fs::write(
        path.join("probe.blsp"),
        "(defmodule probe (:use model))\n\
         (io/puts (str \"features: \" (sort (seq/filter (keys *features*) (fn (k) (includes? (str k) \"model\"))))))\n\
         (io/puts (str \"check: \" (reflect/check-string-here \"(sig h (model -> string)) (defn h (m) 42)\")))\n",
    )
    .unwrap();
    TempDir { path }
}

fn nest_run(dir: &Path, file: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .arg("run")
        .arg(file)
        .output()
        .expect("run nest");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_script_outside_src_loads_a_project_module_once_rooted() {
    let fixture = fixture();
    let out = nest_run(&fixture.path.join("proj"), &fixture.path.join("probe.blsp"));
    assert!(
        out.contains("features: (proj/model)"),
        "the module must be loaded exactly once, under its rooted name:\n{out}"
    );
    assert!(
        !out.contains("unknown type"),
        "a `deftype` alias registered once must resolve from the script:\n{out}"
    );
    assert!(
        out.contains("declared return type string but the body yields 42"),
        "the alias must resolve to the declared shape so the sig is checked:\n{out}"
    );
}
