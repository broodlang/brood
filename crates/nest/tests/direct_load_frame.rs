//! **A directly loaded module file behaves as a required one does — and is scoped to the
//! isolate that loaded it** (KI-170's frame, and the two things it broke on landing).
//!
//! KI-170 wrapped a direct `reflect/load` of a `defmodule` file in the ADR-344 staging frame
//! so its provide and its definitions publish in one journalled write. Two consequences the
//! first cut missed, both red on CI at `c8c58c41`:
//!
//! 1. The journal replayed the file's own defs over its OWN isolate's restore, so every test
//!    file's top-level def leaked into the next file (`nest::named_files_scoped`,
//!    `bare_names_test`). A direct frame's journal entries now carry a `direct` tag: replayed
//!    for a bystander's restore (the window KI-170 closes), discarded — and dropped from the
//!    journal — by the loading isolate's own.
//! 2. A staged REGISTRY op bumped `version` but not `code_epoch`, so a compiled arm that had
//!    read the registry through a `GlobalIc` kept serving the earlier map: the second
//!    `defmulti` of a file was invisible to its `defmethod` ("no `(defmulti mm-cmp …)` is in
//!    scope"), deterministically, through the scoped runner. A `require`d module of that
//!    shape had the same latent bug.
//!
//! Both are pinned through the real entry point — `nest test` over two files, the scoped
//! runner's direct load — because a bare `reflect/load` or `brood --test` takes neither path.

use std::process::Command;

#[test]
fn a_directly_loaded_file_sees_its_second_defmulti_and_does_not_leak_into_the_next_file() {
    let dir = std::env::temp_dir().join(format!("brood-direct-load-frame-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // File A: two multimethods, a method registered between them — the epoch shape — and
    // a top-level def that must NOT reach file B.
    std::fs::write(
        dir.join("aa_multi_test.blsp"),
        "(defmodule aa-multi-test (:use test))\n\
         (defmulti dlf-a :commutative)\n\
         (defmethod dlf-a [:int :string] (n s) (str n s))\n\
         (defmulti dlf-b :antisymmetric)\n\
         (defmethod dlf-b [:int :string] (n s) 1)\n\
         (def dlf-leak-probe 1)\n\
         (describe \"a\"\n\
           (test \"both multimethods dispatch\"\n\
             (assert= \"1x\" (dlf-a 1 \"x\"))\n\
             (assert= -1 (dlf-b \"x\" 1))))\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("zz_scope_test.blsp"),
        "(defmodule zz-scope-test (:use test))\n\
         (describe \"b\"\n\
           (test \"does not see file a's top-level def\"\n\
             (is (not (bound? 'aa-multi-test/dlf-leak-probe)))))\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(&dir)
        .env("BROOD_NO_CRASH_REPORT", "1")
        .args(["test", "aa_multi_test.blsp", "zz_scope_test.blsp"])
        .output()
        .expect("run nest test");
    let _ = std::fs::remove_dir_all(&dir);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("2 tests, 2 passed"),
        "the scoped runner over the two files:\n{text}"
    );
}
