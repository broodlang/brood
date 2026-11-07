//! `nest move` on disk: what the text-level tests in `tests/codemod_test.blsp` cannot see is
//! the order of the writes, and a move that fails halfway must not lose the definition.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A project directory, removed on drop.
struct Project(PathBuf);

impl Project {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("nest-move-{tag}-{}", std::process::id()));
        fs::create_dir_all(dir.join("src")).expect("make the project");
        fs::write(
            dir.join("project.blsp"),
            "(project :name \"p\" :version \"0.1.0\")\n",
        )
        .expect("write project.blsp");
        Project(dir)
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.0.join(name), text).expect("write a source");
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(name)).expect("read a source")
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn nest_move(&self, from: &str, to: &str, name: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_nest"))
            .current_dir(&self.0)
            .args(["move", from, to, name])
            .output()
            .expect("run nest move")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const SOURCE: &str = "(defmodule a)\n\n(defn f () 1)\n\n(defn g () (f))\n";

#[test]
fn a_destination_in_a_new_directory_is_made() {
    let project = Project::new("newdir");
    project.write("src/a.blsp", SOURCE);
    let output = project.nest_move("src/a.blsp", "src/sub/b.blsp", "f");
    assert!(output.status.success(), "{output:?}");
    assert!(project.read("src/sub/b.blsp").contains("(defn f () 1)"));
    assert!(project.read("src/a.blsp").contains("(b/f)"));
}

#[test]
fn a_destination_that_cannot_be_written_leaves_the_source_whole() {
    let project = Project::new("blocked");
    project.write("src/a.blsp", SOURCE);
    // a file where the destination's directory has to go
    project.write("src/sub", "in the way");
    let output = project.nest_move("src/a.blsp", "src/sub/b.blsp", "f");
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(project.read("src/a.blsp"), SOURCE);
}

#[test]
fn a_file_that_does_not_parse_is_refused_or_left_alone() {
    let project = Project::new("broken");
    project.write("src/a.blsp", SOURCE);
    project.write("src/b.blsp", "(defmodule b)\n");
    let broken = "(defmodule c (:use a))\n(defn x (\n(f)\n";
    project.write("src/c.blsp", broken);
    let output = project.nest_move("src/a.blsp", "src/b.blsp", "f");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(project.read("src/c.blsp"), broken);
    assert!(String::from_utf8_lossy(&output.stdout).contains("does not parse"));

    project.write("src/a.blsp", "(defmodule a\n(defn h () 1)\n");
    let output = project.nest_move("src/a.blsp", "src/b.blsp", "h");
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(project.read("src/a.blsp"), "(defmodule a\n(defn h () 1)\n");
}

#[test]
fn an_absolute_path_is_the_same_file_as_a_relative_one() {
    let project = Project::new("absolute");
    project.write("src/a.blsp", SOURCE);
    project.write("src/b.blsp", "(defmodule b (:use a))\n\n(defn h () (f))\n");
    let to = project.path().join("src/b.blsp");
    let output = project.nest_move("src/a.blsp", to.to_str().expect("utf-8 path"), "f");
    assert!(output.status.success(), "{output:?}");
    let destination = project.read("src/b.blsp");
    assert!(destination.contains("(defn f () 1)"), "{destination}");
    assert!(destination.contains("(defn h () (f))"), "{destination}");
}
