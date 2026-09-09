//! `(:use m :exclude […])` that excludes every public name must not report the module as
//! broken.
//!
//! The KI-120 diagnostic asks a real question — "this `(:use m)` imported nothing, so are
//! `m`'s globals gone under a `*features*` that still says loaded?" — and it is default-ON
//! so a recurrence of that wave self-reports. But it answered the question with the wrong
//! count: `referred`, the number of names this `:use` actually took, which is zero both
//! when the globals are missing (the bug) and when the caller excluded all of them
//! (perfectly healthy). The guard against false positives was `is_embedded_module`, which
//! excuses a USER module that excludes everything and not a std one.
//!
//! bedit trips exactly that: `src/completion.blsp` opens with
//! `(:use fuzzy :exclude [filter match])` — `fuzzy`'s only two publics — and calls
//! `fuzzy/filter` qualified. Every cold `nest check` of the flagship downstream project
//! printed a line claiming no public `fuzzy/` global was bound while both were. It reads as
//! a one-off because the run that shows it is the one that has to rebuild `.brood/image.bin`;
//! delete that file and it is deterministic.
//!
//! The fix counts the module's public names (`public_seen`) rather than the imported ones,
//! so the message means what it says. The KI-120 signal is preserved by construction: the
//! added condition is strictly narrower and is false only when the globals exist, which is
//! precisely the case the wave does not produce.

mod support;

use std::path::PathBuf;

// The cli tests each carry their own tiny TempDir (no `tempfile` dev-dependency here);
// mirrored from `stdimage_reporting.rs` so the shape stays recognisable.
struct TempDir {
    path: PathBuf,
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn temp_dir(tag: &str) -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("brood-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create temp dir");
    TempDir { path }
}

/// Run `src` as a program and return its stderr.
fn stderr_of(dir: &std::path::Path, name: &str, src: &str) -> String {
    let out = support::spawn_brood_env(dir, name, src, &[])
        .wait_with_output()
        .expect("run brood");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn excluding_every_public_name_of_a_std_module_is_not_reported_as_missing_globals() {
    let dir = temp_dir("refer-exclude-all");
    // `fuzzy` is an embedded std module whose entire public surface is `filter` and
    // `match`; excluding both is bedit's shape.
    let err = stderr_of(
        &dir.path,
        "exclude_all.blsp",
        "(defmodule exclude-all-probe (:use fuzzy :exclude [filter match]))\n\
         (io/puts (fuzzy/match \"ab\" \"a-b\"))\n",
    );
    assert!(
        !err.contains("[refer]"),
        "excluding every public name is a legitimate empty refer, not missing globals.\n\
         stderr was:\n{err}"
    );
}

#[test]
fn an_ordinary_refer_all_of_the_same_module_is_also_silent() {
    // The control: if this warned too, the test above would pass for the wrong reason
    // (a build where the diagnostic never fires at all proves nothing).
    let dir = temp_dir("refer-all");
    let err = stderr_of(
        &dir.path,
        "refer_all.blsp",
        "(defmodule refer-all-probe (:use fuzzy))\n(io/puts (match \"ab\" \"a-b\"))\n",
    );
    assert!(
        !err.contains("[refer]"),
        "a healthy refer-all must be silent.\nstderr was:\n{err}"
    );
}
