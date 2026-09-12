//! A `deftype` alias resolves through the file's imports (ADR-327 follow-up, 2026-09-12).
//!
//! ADR-327 resolved a bare alias name in the checked file's own namespace first, then to
//! the ONE loaded module declaring it — two candidates decline, so an ambiguous name is an
//! unknown type rather than a silent guess. That left the `(:use …)` header out of the
//! decision: two loaded modules both declaring `shape` made `(sig f (shape -> …))` an
//! unknown type even in a file that `(:use a)`s exactly one of them, which is not what a
//! reader of that header expects. The imports now sit between the own-namespace step and
//! the unique-suffix step: a bare name resolves to the ONE `:use`d module declaring it,
//! and `short/name` through an `(:alias mod :as short)` reaches `mod/name`. Two `:use`d
//! declarers still decline.
//!
//! Loose disk modules, checked from their directory, as `checker_cross_module_ability.rs`
//! does — the loader finds `a`/`b` on the default load path.

use std::path::PathBuf;
use std::process::Command;

mod support;

struct TempDir {
    path: PathBuf,
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn write_fixture() -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "brood-deftype-imports-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("create temp dir");
    // Two modules, each declaring a `shape` of a different shape.
    std::fs::write(
        path.join("a.blsp"),
        "(defmodule a)\n(deftype shape (record :w int :h int))\n(defn a-mk () {:w 1 :h 2})\n",
    )
    .unwrap();
    std::fs::write(
        path.join("b.blsp"),
        "(defmodule b)\n(deftype shape (tuple int int))\n(defn b-mk () [1 2])\n",
    )
    .unwrap();
    // `(:use a)` with b ALSO loaded (through an alias clause): both declare `shape`, and the
    // bare name is a's — the module the header `:use`s — so `(:w s)` is an int and a tuple
    // is refused. Without the imports step this is the two-candidates case and declines.
    std::fs::write(
        path.join("uses-a.blsp"),
        "(defmodule uses-a (:use a) (:alias b :as bb))\n\
         (sig width (shape -> int))\n\
         (defn width (s) (:w s))\n\
         (defn bad () (width [1 2]))\n",
    )
    .unwrap();
    // `(:alias b :as bb)`: `bb/shape` is b's tuple; a record is refused.
    std::fs::write(
        path.join("aliases-b.blsp"),
        "(defmodule aliases-b (:alias b :as bb))\n\
         (sig second-of (bb/shape -> int))\n\
         (defn second-of (s) (second s))\n\
         (defn bad () (second-of {:w 1 :h 2}))\n",
    )
    .unwrap();
    // Both used: still ambiguous, still declined by name.
    std::fs::write(
        path.join("uses-both.blsp"),
        "(defmodule uses-both (:use a) (:use b))\n\
         (sig f (shape -> int))\n\
         (defn f (s) 0)\n",
    )
    .unwrap();
    TempDir { path }
}

fn check(dir: &std::path::Path, file: &str) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.args(["--check", file]).current_dir(dir);
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run brood --check");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_bare_alias_resolves_to_the_one_used_module_declaring_it() {
    let dir = write_fixture();
    let text = check(&dir.path, "uses-a.blsp");
    assert!(
        !text.contains("unknown type `shape`"),
        "`shape` must resolve through `(:use a)`:\n{text}"
    );
    assert!(
        text.contains("uses-a/width: argument 1 expects shape, got"),
        "the resolved alias must reject a tuple where a's record is declared:\n{text}"
    );
}

#[test]
fn a_prefixed_alias_resolves_through_an_alias_clause() {
    let dir = write_fixture();
    let text = check(&dir.path, "aliases-b.blsp");
    assert!(
        !text.contains("unknown type `bb/shape`"),
        "`bb/shape` must resolve through `(:alias b :as bb)`:\n{text}"
    );
    assert!(
        text.contains("aliases-b/second-of: argument 1 expects bb/shape, got"),
        "the resolved alias must reject a record where b's tuple is declared:\n{text}"
    );
}

#[test]
fn two_used_declarers_still_decline() {
    let dir = write_fixture();
    let text = check(&dir.path, "uses-both.blsp");
    assert!(
        text.contains("unknown type `shape`"),
        "two `:use`d modules declaring `shape` must stay ambiguous:\n{text}"
    );
}
