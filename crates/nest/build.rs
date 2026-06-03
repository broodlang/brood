//! Bake the prebuilt lean+gui `brood` runtime into `nest`, so `nest release` can
//! append an app to it with no Rust toolchain at release time (ADR-038).
//!
//! `make install` builds that runtime first, then builds `nest` with
//! `BROOD_EMBED_RUNTIME=<path>` set — this script copies it to
//! `$OUT_DIR/embedded-runtime`, which `main.rs` pulls in via `include_bytes!`.
//! A plain `cargo build` (no env var) writes an empty file, so the embedded
//! slice is empty and `nest release` falls back to building the runtime.

use std::{env, fs, path::PathBuf};

fn main() {
    let dest = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("embedded-runtime");

    match env::var("BROOD_EMBED_RUNTIME") {
        Ok(path) if !path.trim().is_empty() => {
            let bytes = fs::read(&path)
                .unwrap_or_else(|e| panic!("BROOD_EMBED_RUNTIME={path:?} is unreadable: {e}"));
            fs::write(&dest, &bytes).expect("write embedded-runtime");
            // Rebuild the embed if the runtime binary changes underneath us.
            println!("cargo:rerun-if-changed={path}");
        }
        // No runtime to embed — write an empty placeholder so `include_bytes!`
        // always has a file to read (and `main.rs` treats empty as "absent").
        _ => {
            fs::write(&dest, b"").expect("write empty embedded-runtime");
        }
    }
    // Re-run when the toggle flips (set ↔ unset), not just when its value changes.
    println!("cargo:rerun-if-env-changed=BROOD_EMBED_RUNTIME");

    // Bake in the triple this `nest` was built *for*, so `nest release --target`
    // can tell "the host's own triple" (the embedded runtime serves it) apart
    // from a genuine cross-target (needs a cached prebuilt runtime).
    println!(
        "cargo:rustc-env=NEST_HOST_TRIPLE={}",
        env::var("TARGET").expect("cargo sets TARGET for build scripts")
    );
}
