//! The registry flow end to end (ADR-147): `nest publish` into an index, then
//! `nest add pkg :version V` resolving out of it, then actually calling the dep.
//!
//! This was the one subcommand group never exercised — the configured default index
//! (`https://github.com/broodlang/registry`) does not exist, so nothing could be
//! published or resolved against it. It turns out the whole flow *is* testable: the
//! index may be a **local directory** (`registry--read-dir` uses a local path in
//! place), the package source may be a **local git repo**, and `:registry` is a user
//! config key that honours `XDG_CONFIG_HOME` — so a test can point the whole thing at
//! a temp tree without touching the developer's real config.
//!
//! Writing it immediately found a bug: `fetch`/`tree`/`add`/`remove`/`update` never
//! called `load-config`, so they used the hardcoded default index regardless of what
//! `:registry` said, and `nest add pkg :version 1.0.0` failed against a perfectly good
//! local registry. Only `publish`/`search` loaded it. They now share one bootstrap.

use std::path::Path;
use std::process::Command;

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

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
    let path = std::env::temp_dir().join(format!("brood-reg-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    TempDir { path }
}

struct Out {
    text: String,
    ok: bool,
}

/// Run `nest` with the registry config pointed at `config_home` (may be `None`).
fn nest_at(dir: &Path, config_home: Option<&Path>, args: &[&str]) -> Out {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nest"));
    cmd.current_dir(dir).args(args);
    if let Some(home) = config_home {
        cmd.env("XDG_CONFIG_HOME", home);
    }
    let out = cmd.output().expect("run nest");
    Out {
        text: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        ok: out.status.success(),
    }
}

fn git(dir: &Path, args: &[&str]) {
    // Neutralize any ambient signing / credential config so a `git commit`/`tag`
    // here never blocks on a locked GPG/SSH signing agent — the test result must
    // not depend on whether a developer's desktop keychain happens to be unlocked
    // (that made CI flap). Matches the guard in `format_changed.rs`.
    let ok = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "-c",
            "credential.helper=",
        ])
        .args(args)
        .output()
        .expect("run git")
        .status
        .success();
    assert!(ok, "git {args:?} failed in {}", dir.display());
}

/// A workspace with: an empty local index, a publishable package backed by a real
/// local git repo, a consumer project, and a config pointing `:registry` at the index.
struct Workspace {
    _tmp: TempDir,
    index: std::path::PathBuf,
    package: std::path::PathBuf,
    consumer: std::path::PathBuf,
    config_home: std::path::PathBuf,
}

fn workspace(tag: &str) -> Workspace {
    let tmp = tempdir(tag);
    let root = tmp.path.clone();

    let index = root.join("index");
    std::fs::create_dir_all(index.join("packages")).unwrap();

    // The package, as a real git repo — the registry entry records a git source, so
    // resolution clones it.
    let package = root.join("greeter");
    std::fs::create_dir_all(package.join("src")).unwrap();
    std::fs::write(
        package.join("project.blsp"),
        format!(
            "(project :name \"greeter\" :version \"1.0.0\" :description \"Greets.\" \
             :repository \"{}\")\n",
            package.display()
        ),
    )
    .unwrap();
    std::fs::write(
        package.join("src/greeter.blsp"),
        "(defmodule greeter \"Greets.\")\n\n(defn greeter-hi () \"hi from greeter\")\n",
    )
    .unwrap();
    git(&package, &["init", "-q", "."]);
    git(&package, &["config", "user.email", "test@example.com"]);
    git(&package, &["config", "user.name", "Test"]);
    git(&package, &["add", "-A"]);
    git(&package, &["commit", "-q", "-m", "greeter 1.0.0"]);
    git(&package, &["tag", "v1.0.0"]);

    let consumer = root.join("consumer");
    assert!(
        nest_at(&root, None, &["new", "consumer"]).ok,
        "scaffolding the consumer failed"
    );

    // `:registry` is a USER config key, not a manifest one — honouring
    // XDG_CONFIG_HOME keeps this test out of the developer's real config.
    let config_home = root.join("cfg");
    std::fs::create_dir_all(config_home.join("brood")).unwrap();
    std::fs::write(
        config_home.join("brood/config.blsp"),
        format!("(config :registry \"{}\")\n", index.display()),
    )
    .unwrap();

    Workspace {
        _tmp: tmp,
        index,
        package,
        consumer,
        config_home,
    }
}

#[test]
fn publish_writes_an_entry_a_local_index_can_serve() {
    if !have_git() {
        eprintln!("skipping: git not available");
        return;
    }
    let w = workspace("publish");
    let published = nest_at(&w.package, None, &["publish", w.index.to_str().unwrap()]);
    assert!(published.ok, "publish failed:\n{}", published.text);

    let entry = std::fs::read_to_string(w.index.join("packages/greeter.blsp"))
        .expect("publish should have written packages/greeter.blsp");
    assert!(entry.contains("1.0.0"), "entry lacks the version:\n{entry}");
    assert!(
        entry.contains("Greets."),
        "entry lacks the description:\n{entry}"
    );
}

#[test]
fn publish_refuses_a_url_and_a_duplicate_version() {
    if !have_git() {
        eprintln!("skipping: git not available");
        return;
    }
    let w = workspace("refuse");

    let url = nest_at(&w.package, None, &["publish", "https://example.com/reg"]);
    assert!(!url.ok, "publishing to a URL should be refused");
    assert!(
        url.text.contains("local index checkout"),
        "the refusal should say why:\n{}",
        url.text
    );

    assert!(nest_at(&w.package, None, &["publish", w.index.to_str().unwrap()]).ok);
    let again = nest_at(&w.package, None, &["publish", w.index.to_str().unwrap()]);
    assert!(!again.ok, "republishing the same version should be refused");
    assert!(
        again.text.contains("already published"),
        "the refusal should say why:\n{}",
        again.text
    );
}

#[test]
fn search_finds_a_published_package_by_name_and_description() {
    if !have_git() {
        eprintln!("skipping: git not available");
        return;
    }
    let w = workspace("search");
    assert!(nest_at(&w.package, None, &["publish", w.index.to_str().unwrap()]).ok);

    let index = w.index.to_str().unwrap();
    for term in ["greeter", "Greets"] {
        let found = nest_at(&w.consumer, None, &["search", term, index]);
        assert!(found.ok, "search {term} failed:\n{}", found.text);
        assert!(
            found.text.contains("greeter") && found.text.contains("1.0.0"),
            "search {term} should list the package:\n{}",
            found.text
        );
    }
    let miss = nest_at(&w.consumer, None, &["search", "zzz-no-such-pkg", index]);
    assert!(miss.ok, "a miss is not an error:\n{}", miss.text);
    assert!(
        miss.text.contains("No packages matching"),
        "a miss should say so:\n{}",
        miss.text
    );
}

/// The regression for the bug this file found: every package command must honour the
/// configured `:registry`, not just `publish`/`search`.
#[test]
fn add_by_version_resolves_from_the_configured_registry() {
    if !have_git() {
        eprintln!("skipping: git not available");
        return;
    }
    let w = workspace("add");
    assert!(nest_at(&w.package, None, &["publish", w.index.to_str().unwrap()]).ok);

    let added = nest_at(
        &w.consumer,
        Some(&w.config_home),
        &["add", "greeter", ":version", "1.0.0"],
    );
    assert!(
        added.ok,
        "`add :version` must use the CONFIGURED registry, not the hardcoded default:\n{}",
        added.text
    );

    let tree = nest_at(&w.consumer, Some(&w.config_home), &["tree"]);
    assert!(tree.ok, "tree failed:\n{}", tree.text);
    assert!(
        tree.text.contains("greeter"),
        "the dep should appear in the tree:\n{}",
        tree.text
    );

    // The point of a dependency: the consumer can call into it.
    std::fs::write(
        w.consumer.join("tests/use_registry_test.blsp"),
        "(defmodule use-registry-test (:use test) (:use greeter))\n\n\
         (describe \"registry dep\"\n  (test \"callable\" (assert= (greeter-hi) \"hi from greeter\")))\n",
    )
    .unwrap();
    let tested = nest_at(&w.consumer, Some(&w.config_home), &["test"]);
    assert!(
        tested.ok,
        "the consumer should be able to call the registry dep:\n{}",
        tested.text
    );
}

#[test]
fn search_without_an_index_argument_uses_the_configured_registry() {
    if !have_git() {
        eprintln!("skipping: git not available");
        return;
    }
    let w = workspace("cfgsearch");
    assert!(nest_at(&w.package, None, &["publish", w.index.to_str().unwrap()]).ok);
    let found = nest_at(&w.consumer, Some(&w.config_home), &["search", "greeter"]);
    assert!(found.ok, "search failed:\n{}", found.text);
    assert!(
        found.text.contains("greeter"),
        "the configured registry should have been searched:\n{}",
        found.text
    );
}

#[test]
fn an_unreachable_registry_explains_itself_without_dumping_a_structure() {
    // The default index does not exist, so this is also the real-world first-run
    // experience. It must not print `[:error {:kind :runtime, :message …}]`.
    let w = workspace("unreachable");
    let searched = nest_at(&w.consumer, None, &["search", "anything"]);
    assert!(!searched.ok, "an unreachable registry should fail");
    assert!(
        searched.text.contains("cannot reach the registry index"),
        "it should name the problem:\n{}",
        searched.text
    );
    assert!(
        !searched.text.contains(":kind :runtime"),
        "it should not dump the raw error structure:\n{}",
        searched.text
    );
}
