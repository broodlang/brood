//! A bare `(require 'util)` of a project module already loaded under its rooted name
//! (`app/util`, ADR-070) is a no-op (KI-415). The bare name fell through to the filename probe
//! on the load path, which found the module's own file and evaluated it a second time,
//! rebinding every definition over whatever had been redefined since: a REPL edit, a reload, a
//! tool's prelude. Run twice, so the module is served from source and then from the image.
//! **Sabotage-verified**: without the owner lookup both runs print `AFTER 40`.

#![cfg(unix)]

use std::path::Path;
use std::process::Command;

mod support;
use support::temp_dir;

fn fixture(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("project.blsp"),
        "(project\n  :name redefs\n  :main main)\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/helper.blsp"),
        "(defmodule helper)\n(defn base () 40)\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.blsp"),
        "(defmodule main)\n(defn main ()\n  (io/puts (str \"BEFORE \" (helper/base)))\n  (def helper/base (fn () :redefined))\n  (require-one 'helper)\n  (io/puts (str \"AFTER \" (helper/base))))\n",
    )
    .unwrap();
}

fn run(project: &Path, cache: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .arg("run")
        .current_dir(project)
        .env("XDG_CACHE_HOME", cache)
        .env("BROOD_NO_CRASH_REPORT", "1")
        .output()
        .expect("spawn nest");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_bare_require_of_a_loaded_project_module_keeps_its_redefinitions() {
    let dir = temp_dir("require-bare");
    let project = dir.path.join("project");
    let cache = dir.path.join("cache");
    fixture(&project);
    for which in ["from source", "from the image"] {
        let text = run(&project, &cache);
        assert!(
            text.contains("BEFORE 40") && text.contains("AFTER :redefined"),
            "{which}, the redefinition should survive the require:\n{text}"
        );
    }
}
