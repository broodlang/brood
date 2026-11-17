//! `nest fetch`, `nest update` and `nest tree` cover `:dev-dependencies` (ADR-172), as setup
//! does. They read the production dependencies alone, so a project with only dev ones
//! answered "no dependencies declared" and locked nothing (sqlite could not move its dev
//! `store` to a fixed release), and `nest update` in a project with both wrote a lock
//! without the dev ones.

mod support;
use support::{nest, temp_dir};

fn write_project(dir: &std::path::Path, name: &str, deps: &str) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("project.blsp"),
        format!("(project :name \"{name}\" :version \"0.1.0\"{deps})\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("src").join(format!("{name}.blsp")),
        format!("(defmodule {name})\n(defn hello () :{name})\n"),
    )
    .unwrap();
}

#[test]
fn fetch_update_and_tree_lock_dev_dependencies_too() {
    let tmp = temp_dir("devdeps");
    write_project(&tmp.path.join("helper"), "helper", "");
    write_project(&tmp.path.join("kit"), "kit", "");
    let app = tmp.path.join("app");

    // Only dev dependencies: fetch locks them.
    write_project(&app, "app", " :dev-dependencies [[kit :path \"../kit\"]]");
    let (out, ok) = nest(&app, &["fetch"]);
    assert!(ok, "nest fetch failed: {out}");
    assert!(
        !out.contains("no dependencies declared"),
        "a dev dependency is a dependency:\n{out}"
    );
    let lock = std::fs::read_to_string(app.join("project.lock.blsp")).unwrap();
    assert!(
        lock.contains("[kit :path"),
        "the dev dependency must be locked:\n{lock}"
    );

    // Both kinds: update keeps the dev one in the lock, and tree shows it.
    write_project(
        &app,
        "app",
        " :dependencies [[helper :path \"../helper\"]] :dev-dependencies [[kit :path \"../kit\"]]",
    );
    let (out, ok) = nest(&app, &["update"]);
    assert!(ok, "nest update failed: {out}");
    let lock = std::fs::read_to_string(app.join("project.lock.blsp")).unwrap();
    assert!(
        lock.contains("[helper :path") && lock.contains("[kit :path"),
        "update must lock both kinds:\n{lock}"
    );
    let (out, ok) = nest(&app, &["tree"]);
    assert!(
        ok && out.contains("kit") && out.contains("helper"),
        "tree must show both:\n{out}"
    );
}
