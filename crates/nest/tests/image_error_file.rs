//! An error raised in a project module served from the startup image names the module's own
//! file (KI-414). The image stamped a std section's forms with the std module's file (KI-184) but
//! a project section's with none, so an imaged module's error carried its line and column and no
//! file, and the first `try`, loader or crash reporter to see it filled in its OWN file:
//! womanblend's hair crash was reported at `std/seq.blsp:1270`, `seq/pmap`'s worker, a line of
//! `b3d-hair.blsp`.
//!
//! Two runs of one project: the first loads the module from source and writes the image, the
//! second is served from it. Both must name `raiser.blsp`.
//! **Sabotage-verified**: with the loader passing only the module's key again, the second run's
//! error carries no file at all (`FILE nil`), for whatever catches it to fill in its own.

#![cfg(unix)]

use std::path::Path;
use std::process::Command;

mod support;
use support::temp_dir;

fn fixture(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("project.blsp"),
        "(project\n  :name imagefile\n  :main main)\n",
    )
    .unwrap();
    // the error raised two closures deep, as the hair's was
    std::fs::write(
        dir.join("src/raiser.blsp"),
        "(defmodule raiser)\n(defn blend (rows) (seq/mapv rows (fn (row) (seq/mapv row (fn (x) (- x 1.0))))))\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.blsp"),
        "(defmodule main)\n(defn main ()\n  (let (e (try (raiser/blend [[:pane]]) (catch e e)))\n    (io/puts (str \"FILE \" (get e :file) \" LINE \" (get e :line)))))\n",
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
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    text.lines()
        .find(|line| line.starts_with("FILE "))
        .unwrap_or_else(|| panic!("the project printed no FILE line:\n{text}"))
        .to_string()
}

#[test]
fn an_imaged_project_modules_error_names_its_own_file() {
    let dir = temp_dir("image-error-file");
    let project = dir.path.join("project");
    let cache = dir.path.join("cache");
    fixture(&project);
    let from_source = run(&project, &cache);
    let from_image = run(&project, &cache);
    for (which, line) in [
        ("from source", &from_source),
        ("from the image", &from_image),
    ] {
        assert!(
            line.contains("src/raiser.blsp LINE 2"),
            "{which}, the error should name raiser.blsp line 2: {line}"
        );
    }
}
