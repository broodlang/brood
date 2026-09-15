//! `nest test FILE…` scopes each named file like the whole-project run does (KI-141's
//! follow-up). The named files used to be loaded into ONE image and run together, so a
//! test's top-level `def` in the first file was a public global to every file after it —
//! `audit_test` read `mcp_test`'s eval-tool defs as undocumented public names whenever the
//! pre-push hook named the two together, and a fault that needed two files' isolates to
//! interleave could not be reproduced with any file list at all.
//!
//! Two files outside any project: the first defines a global at top level and asserts it
//! sees it; the second asserts the name is NOT bound. Before the fix the second failed.

use std::process::Command;

#[test]
fn a_named_files_top_level_def_does_not_reach_the_next_named_file() {
    let dir = std::env::temp_dir().join(format!("nest-named-scoped-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("aa_leaks_test.blsp"),
        "(defmodule aa-leaks-test (:use test))\n\
         (def leak-probe 1)\n\
         (describe \"the first file\"\n\
           (test \"sees its own top-level def\"\n\
             (assert= 1 leak-probe)\n\
             (is (bound? 'aa-leaks-test/leak-probe))))\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("zz_clean_test.blsp"),
        "(defmodule zz-clean-test (:use test))\n\
         (describe \"the second file\"\n\
           (test \"does not see the first file's top-level def\"\n\
             (is (not (bound? 'aa-leaks-test/leak-probe)))))\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(&dir)
        .args(["test", "aa_leaks_test.blsp", "zz_clean_test.blsp"])
        .output()
        .expect("nest runs");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        text.contains("2 tests, 2 passed"),
        "the first named file's top-level def reached the second — named files must be \
         scoped per file like the whole-project run:\n{text}"
    );
}
