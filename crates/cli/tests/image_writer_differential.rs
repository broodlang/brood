//! The stdlib image's WRITER is not part of the checker's answer (KI-172). Two images of the
//! same std, written by two `brood` processes in different states — one that ran its
//! pre-flight check first (which loads modules), one that skipped it — must give the checker
//! the same signature for every function of the modules the optimiser's source rewrites
//! touch. Before the loader held `SourceRewritesOn`, the checking process loaded `seq` with
//! the tally rewrite OFF, its image carried that body, and `debug/hits` read
//! `(map any number)` under one image and `(or map table)` under the other (2026-09-20).
//!
//! The probe is `reflect/file-signatures` over the modules that call a rewritten std
//! function through the LOADED heap (the verdict the image decides), read in a fresh
//! process per arm with that arm's cache. Byte for byte.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn arm_cache(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brood-image-writer-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create arm cache dir");
    dir
}

fn brood(cache: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.env("XDG_CACHE_HOME", cache)
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_NO_CHECK_CACHE", "1")
        .env("BROOD_TIER", "1")
        .env_remove("BROOD_VM")
        .env_remove("BROOD_NO_JIT")
        .env_remove("BROOD_NO_STDIMAGE")
        .env_remove("BROOD_NO_IMAGE_SIGS")
        .env_remove("BROOD_COVERAGE")
        .current_dir(workspace_root());
    cmd
}

fn run(cmd: &mut Command, program: &Path) -> String {
    let out = cmd.arg(program).output().expect("run brood");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn an_image_written_after_a_check_reads_the_same_as_one_written_without() {
    let scratch = std::env::temp_dir().join(format!("brood-image-writer-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("scratch");
    // The writer: `stdimage/build` from a program that, on the checking arm, has ALREADY
    // had `seq` loaded by its pre-flight (the file names `seq/frequencies`, so the drain
    // brings it in — and before the fix, expanded it with the rewrites off). No `defn`:
    // the builder refuses a root global it cannot attribute to a module.
    let writer = scratch.join("writer.blsp");
    std::fs::write(
        &writer,
        "(io/puts (str (count (seq/frequencies [1 2 2]))))\n\
         (stdimage/build)\n\
         (io/puts (str (get (stdimage/status) :state)))\n",
    )
    .expect("write the writer program");
    // The probe: what the checker holds for every function of the modules that reach a
    // rewritten body through the loaded heap. `debug/hits` is the one that differed.
    let probe = scratch.join("probe.blsp");
    std::fs::write(
        &probe,
        "(doseq (f [\"std/tool/debug.blsp\" \"std/seq.blsp\" \"std/stats.blsp\" \"std/tool/audit.blsp\"])\n\
         \x20 (doseq (s (reflect/file-signatures f))\n\
         \x20   (io/puts (str (get s :name) \" \" (get s :sig)))))\n",
    )
    .expect("write the probe program");
    let mut arms: Vec<(&str, String)> = Vec::new();
    for (tag, skip_check) in [("checked", false), ("unchecked", true)] {
        let cache = arm_cache(tag);
        let mut write = brood(&cache);
        if skip_check {
            write.env("BROOD_NO_CHECK", "1");
        }
        let wrote = run(&mut write, &writer);
        assert!(
            wrote.trim_end().ends_with(":live"),
            "{tag}: the writer did not leave a live image:\n{wrote}"
        );
        let mut read = brood(&cache);
        read.env("BROOD_NO_CHECK", "1");
        let read = run(&mut read, &probe);
        assert!(
            read.contains("hits "),
            "{tag}: the probe did not reach `debug/hits`:\n{read}"
        );
        arms.push((tag, read));
        let _ = std::fs::remove_dir_all(&cache);
    }
    let _ = std::fs::remove_dir_all(&scratch);
    let (checked, unchecked) = (&arms[0].1, &arms[1].1);
    if checked != unchecked {
        let first = checked
            .lines()
            .zip(unchecked.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        let show = |text: &str| {
            text.lines()
                .skip(first.saturating_sub(1))
                .take(6)
                .collect::<Vec<_>>()
                .join("\n")
        };
        panic!(
            "the image's writer changed what the checker holds (first difference at line {}).\n\
             --- image written by a process that had checked first:\n{}\n\
             --- image written without a check:\n{}",
            first + 1,
            show(checked),
            show(unchecked)
        );
    }
}
