//! The runtime and build-time stdlib hashes must be the same function.
//!
//! `build.rs` bakes `BROOD_STDLIB_HASH` — a content hash of every `std/**/*.blsp` — into the
//! binary, and `cli_support::warn_if_stdlib_is_stale` recomputes it from the tree to tell a
//! developer their binary is older than their edits. A build script cannot share code with
//! the crate it builds, so that algorithm exists twice, and two copies of a hash drift
//! silently: the warning would simply stop firing, and the trap it exists for is one where
//! nothing looks wrong in the first place.
//!
//! This is a real gate rather than a tautology because `cargo test` rebuilds before running,
//! so the baked hash always describes the tree the test reads.

#[test]
fn the_runtime_and_build_time_stdlib_hashes_agree() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root");
    let from_tree = brood::cli_support::stdlib_tree_hash_for_test(&root)
        .expect("the tree has std/**/*.blsp to hash");
    assert_eq!(
        from_tree,
        env!("BROOD_STDLIB_HASH"),
        "the runtime hash in cli_support.rs has drifted from the one build.rs bakes in — \
         the staleness warning would stop firing, silently"
    );
}
