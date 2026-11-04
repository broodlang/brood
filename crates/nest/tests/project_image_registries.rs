//! The project startup image (ADR-218) and the registries — KI-89's residual, both halves.
//!
//! The image's root section carries the registry globals (`*record-ids*`, `*impls*`,
//! `*impl-from*`, …). Two things went wrong with that, and this test pins both at the entry
//! point a user reaches — a second `nest test` in a project, the imaged boot:
//!
//! 1. **Written wholesale, the registries carried the BUILD session's load state**: record ids
//!    and impls of std modules that were loaded when the image was built but that an imaged
//!    boot never loads, so a process started with registered record ids that had no
//!    constructor. `write-image` now prunes registrations owned by modules the image does
//!    not carry (`image-prune-foreign-registrations`).
//! 2. **Restored wholesale, the root section REPLACED the live registries**, erasing what
//!    modules loaded before the restore had registered — `nest` boots with `datetime` loaded,
//!    and its `Temporal/->iso` impls vanished on every imaged boot once (1) stopped the image
//!    from accidentally carrying them. `project-install-image` now merges the live entries
//!    back (`project-registry-merge-live!`).
//!
//! Run 1 builds the image; run 2 boots from it and must still dispatch `->iso` on a datetime
//! value. Sabotage-verified: with the merge removed, run 2 fails
//! `ability Temporal/->iso: no impl for :datetime/datetime`.

use std::path::Path;
use std::process::Command;

fn scaffold(dir: &Path) {
    let src = dir.join("src");
    let tests = dir.join("tests");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&tests).unwrap();
    std::fs::write(
        dir.join("project.blsp"),
        "(project :name \"regimg-demo\")\n",
    )
    .unwrap();
    // A project module with a record of its own, so the image has a project-owned
    // registration to carry (and the prune has something to KEEP, not only to drop).
    std::fs::write(
        src.join("app.blsp"),
        "(defmodule app)\n(defrecord ticket (id))\n(defn make (n) (ticket n))\n",
    )
    .unwrap();
    std::fs::write(
        tests.join("registries_test.blsp"),
        "(defmodule registries-test (:use test) (:use datetime) (:use app))\n\
         (describe \"an imaged boot keeps every registration consistent\"\n\
           (test \"a std module loaded at boot still dispatches its ability impls\"\n\
             (is (string? (->iso (utc-now)))))\n\
           (test \"the project's own record id names a bound constructor\"\n\
             (is (contains? *record-ids* (%identity-of (make 1))))\n\
             (is (= 7 (get (make 7) :id))))\n\
           (test \"no registered record id lacks its constructor\"\n\
             (is (empty? (seq/filter (keys *record-ids*)\n\
                           (fn (id) (not (bound? (symbol (->string id))))))))))\n",
    )
    .unwrap();
}

/// A project whose module's load writes a map entry and a list member, and a test that asks
/// who wrote each (KI-357, ADR-595).
fn scaffold_writers(dir: &Path) {
    let src = dir.join("src");
    let tests = dir.join("tests");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&tests).unwrap();
    std::fs::write(
        dir.join("project.blsp"),
        "(project :name \"regwriters-demo\")\n",
    )
    .unwrap();
    std::fs::write(
        src.join("hooks.blsp"),
        "(defmodule hooks)\n\
         (registry/put! :demo-registry/rows :row 1)\n\
         (%registry-update! 'demo-registry/*hooks* :append-new nil :appended)\n\
         (%registry-update! 'demo-registry/*hooks* :cons-new nil :consed)\n\
         (defn ready () true)\n",
    )
    .unwrap();
    std::fs::write(
        tests.join("writers_test.blsp"),
        "(defmodule writers-test (:use test) (:use hooks))\n\
         (describe \"who wrote what the project's load registered\"\n\
           (test \"the module, from source or from the image\"\n\
             (is (hooks/ready))\n\
             (assert= (registry/writer :demo-registry/rows :row) 'regwriters-demo/hooks)\n\
             (assert= demo-registry/*hooks* '(:consed :appended))\n\
             (assert= (%registry-writer 'demo-registry/*hooks* [:consed]) 'regwriters-demo/hooks)\n\
             (assert= (%registry-writer 'demo-registry/*hooks* [:appended]) 'regwriters-demo/hooks)\n\
             (assert= (count (seq/filter (registry/written-by #{'regwriters-demo/hooks})\n\
                               (fn (write) (or (:member write)\n\
                                               (= (first (:path write)) :demo-registry/rows)))))\n\
                      3)))\n",
    )
    .unwrap();
}

fn nest_test(dir: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .arg("test")
        .current_dir(dir)
        .output()
        .expect("spawn nest test");
    let text = format!(
        "--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn a_second_nest_test_boots_from_the_image_with_consistent_registries() {
    let dir = std::env::temp_dir().join(format!("brood-regimg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    scaffold(&dir);
    let (ok1, text1) = nest_test(&dir);
    assert!(
        ok1,
        "run 1 (source load, writes the image) failed:\n{text1}"
    );
    assert!(
        dir.join(".brood").join("image.bin").exists(),
        "run 1 must have written the project image\n{text1}"
    );
    let (ok2, text2) = nest_test(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        ok2,
        "run 2 (imaged boot) failed — a registration the root section restored clobbered or \
         orphaned something (KI-89):\n{text2}"
    );
    assert!(
        text2.contains("3 tests, 3 passed"),
        "both runs must run all three tests\n{text2}"
    );
}

/// KI-357 (ADR-595): a module the project image materialises is credited with what its source
/// load wrote — a map entry and two list members — exactly as a source load is. The image
/// installs the registry VALUES whole; before this nothing in an imaged boot had a writer.
#[test]
fn an_imaged_boot_credits_each_registry_entry_to_the_module_that_wrote_it() {
    let dir = std::env::temp_dir().join(format!("brood-regwriters-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    scaffold_writers(&dir);
    let (ok1, text1) = nest_test(&dir);
    assert!(
        ok1,
        "run 1 (source load, writes the image) failed:\n{text1}"
    );
    assert!(
        dir.join(".brood").join("image.bin").exists(),
        "run 1 must have written the project image\n{text1}"
    );
    let (ok2, text2) = nest_test(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        ok2,
        "run 2 (imaged boot) credited an entry to someone other than its writer (KI-357):\n{text2}"
    );
    assert!(
        text2.contains("1 passed, 0 failed"),
        "run 2 ran the test\n{text2}"
    );
}
