//! **A boot from the prelude image must not consult a stale section directory.**
//!
//! This is the reproduction ADR-314 recorded as *missing*. That ADR describes the failure —
//! `unbound symbol: io/puts` on a tree where nothing is wrong with `io` — states the
//! mechanism as a hypothesis, and then records three attempts to reproduce it, all of which
//! passed under a sabotage that removed the fix. The default stayed off for that reason: a
//! feature with a real failure in its history and no test for it.
//!
//! The mechanism, confirmed 2026-09-04. `%add-image-source!` APPENDS. A boot from the
//! prelude image restores bindings rather than evaluating the prelude, so `*image-sources*`
//! comes back holding a snapshot of whatever stdlib install was live when that prelude image
//! was written. Replaying `%std-image-install` over the snapshot therefore leaves **two**
//! directories for the **same file path** — the stale one first — and `%image-section-for`
//! scans in install order. Because the path still exists and is readable, the stale entry's
//! offset does not fail cleanly and fall back to source; it returns garbage from the current
//! file. `%std-image-reinstall!` clears the registry before installing.
//!
//! What the three earlier attempts each broke is the same-path-still-readable condition:
//! deleting the image (a missing file fails cleanly), rebuilding the layout without a
//! prelude image written under the old one (no snapshot to be stale), and a module left out
//! entirely (no section at all, so `require` loads source).
//!
//! The sequence below is a real deployment, not a contrivance: a **lean** `nest` and a
//! **full** `brood` share `~/.cache/brood`, and the image id is keyed on version + git sha +
//! stdlib content hash — **not** on which modules the writer chose to include. So two
//! binaries of the same tree legitimately write different layouts to the same path.

use std::process::Command;

/// A program that exercises exactly what the failure took down: a prelude-owned name bound
/// to a primitive (`io/puts`), and a module materialised from the image (`json`).
const PROBE: &str = r#"
(io/puts "json=" (json/encode {:a 1}))
(io/puts "ok")
"#;

fn cache_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("brood-relaid-image-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create cache dir");
    dir
}

fn write_program(name: &str, body: &str) -> std::path::PathBuf {
    let path =
        std::env::temp_dir().join(format!("brood-relaid-{}-{name}.blsp", std::process::id()));
    std::fs::write(&path, body).expect("write program");
    path
}

/// Remove the prelude image and the expanded-prelude text cache, so the next run takes the
/// full source boot and re-snapshots whatever install state is live at that moment.
fn discard_prelude_artifacts(cache: &std::path::Path) {
    let dir = cache.join("brood");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        panic!(
            "no cache dir at {} — the earlier runs wrote nothing",
            dir.display()
        );
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("prelude-expanded-") {
            std::fs::remove_file(entry.path()).expect("remove prelude artifact");
            removed += 1;
        }
    }
    assert!(
        removed > 0,
        "found no prelude-expanded-* artifacts to discard in {} — the naming changed, and \
         this test would silently stop arming its own repro",
        dir.display()
    );
}

fn brood(
    cache: &std::path::Path,
    program: &std::path::Path,
    no_image: bool,
) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.env("XDG_CACHE_HOME", cache)
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_BOOT_TRACE", "1")
        .env("BROOD_TIER", "1")
        .env_remove("BROOD_VM")
        .env_remove("BROOD_NO_JIT")
        .env_remove("BROOD_NO_STDIMAGE")
        .env_remove("BROOD_COVERAGE")
        .env_remove("BROOD_PRELUDE_IMAGE")
        .env_remove("BROOD_NO_PRELUDE_IMAGE");
    // The image is the default (ADR-314); the control arm opts out. Both spellings are
    // cleared above so an ambient one cannot silently turn either arm into the other.
    if no_image {
        cmd.env("BROOD_NO_PRELUDE_IMAGE", "1");
    }
    cmd.arg(program).output().expect("run brood")
}

#[test]
fn a_prelude_image_written_under_a_different_stdlib_layout_still_boots() {
    let cache = cache_dir("main");
    let probe = write_program("probe", PROBE);
    let full = write_program("full", "(stdimage/build)\n");
    // Four modules, so the rebuilt file has a handful of sections where the first had ~107 —
    // the same path, a different layout, which is what makes a stale offset readable.
    let lean = write_program(
        "lean",
        "(stdimage/build [\"json\" \"string\" \"io\" \"file\"])\n",
    );

    // 1. A FULL stdlib image.
    let out = brood(&cache, &full, false);
    assert!(
        out.status.success(),
        "building the full stdlib image failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 2. ARM IT: discard the prelude artifacts and cold-boot again, so the prelude image's
    //    snapshot of the install is taken WITH THE FULL STDLIB IMAGE LIVE. This step is the
    //    whole test. Without it, step 1's own cold boot — which ran before any stdlib image
    //    existed — writes a prelude image whose snapshot is empty, the replay has nothing
    //    stale to append to, and the case passes with the fix removed. It did: the first cut
    //    of this test passed its own sabotage for exactly this reason.
    //
    //    In the real deployment the arming is free, because the writer of the stdlib image
    //    is a DIFFERENT binary (a lean `nest`) with its own prelude image, so `brood`'s is
    //    written on its first boot after that image already exists.
    discard_prelude_artifacts(&cache);
    let cold = brood(&cache, &probe, false);
    let cold_err = String::from_utf8_lossy(&cold.stderr);
    assert!(
        cold_err.contains("source boot"),
        "the arming boot did not cold-boot, so the prelude image kept an older snapshot and \
         this test proves nothing. Boot trace was:\n{cold_err}"
    );

    // 3. Re-lay the stdlib image with fewer modules, at the same path and the same id.
    let out = brood(&cache, &lean, false);
    assert!(
        out.status.success(),
        "building the lean stdlib image failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 4. Boot again. Before `%std-image-reinstall!` this printed
    //    `unbound symbol: io/puts` and exited nonzero.
    let out = brood(&cache, &probe, false);
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // The gate is vacuous unless this boot really came from the prelude image — an absent or
    // stale artifact would silently take the text path, where the bug does not exist.
    assert!(
        stderr.contains("(prelude image)"),
        "this run did not boot from the prelude image, so it cannot detect the stale-directory \
         bug at all. Boot trace was:\n{stderr}"
    );
    // Assert the OUTPUT is present, never merely that no error appeared: a run that dies
    // before printing has no error text of the shape we would grep for either.
    assert!(
        stdout.contains("json= {\"a\":1}") && stdout.contains("ok"),
        "the probe did not produce its output — a section read landed on a stale offset.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        out.status.success(),
        "the probe exited {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code()
    );

    // And the two boot paths must agree under this artifact state, not merely both survive.
    let src = brood(&cache, &probe, true);
    assert_eq!(
        stdout,
        String::from_utf8_lossy(&src.stdout),
        "the imaged boot and the source boot disagree after the stdlib image was re-laid"
    );
}
