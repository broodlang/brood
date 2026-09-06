//! KI-112. `stdimage/build` attributes a module's ROOT globals by loading it and diffing the
//! global names; for a module the process had already loaded, `require-one` is a no-op and
//! the probe claims nothing, so those roots were silently dropped from the image. Dispatched
//! from `std/tool/nest.blsp` — whose load pulls the toolchain in — `nest stdimage` wrote an
//! image with none of `project`'s 31 root globals, and every later `nest check` died on an
//! unbound `*ns-package*`.
//!
//! The build now audits: a root global with no owning section, no definition site (the
//! prelude's have one) and not a native builtin is a std module's that the probe missed, and
//! the build REFUSES rather than writes. This drives the real `brood` binary with a private
//! cache directory, so a regression cannot poison the developer's cache, and asserts both
//! halves: the refusal names the module's root, and no image file appears.

use std::process::Command;

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("brood-stdimage-dirty-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn brood(dir: &std::path::Path, cache: &std::path::Path, script: &str) -> (i32, String) {
    let prog = dir.join("prog.blsp");
    std::fs::write(&prog, script).expect("write script");
    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        .current_dir(dir)
        .env("XDG_CACHE_HOME", cache)
        .env("BROOD_NO_CHECK", "1")
        .arg(&prog)
        .output()
        .expect("run brood");
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

fn image_files(cache: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(cache.join("brood"))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("std-image-"))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_build_in_a_process_that_already_loaded_a_tooling_module_is_refused_not_written() {
    let dir = scratch("refuse");
    let cache = dir.join("cache");
    std::fs::create_dir_all(cache.join("brood")).expect("cache");
    let (code, out) = brood(
        &dir,
        &cache,
        "(require-one 'project)\n\
         (require-one 'stdimage)\n\
         (io/puts (try (do (stdimage/build) \"WROTE\") (catch e (str \"refused: \" (error-message e)))))\n",
    );
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("refused: stdimage/build:") && out.contains("*ns-package*"),
        "the refusal must name the orphaned root:\n{out}"
    );
    assert!(
        image_files(&cache).is_empty(),
        "nothing may be written on a refusal: {:?}",
        image_files(&cache)
    );
}

#[test]
fn a_build_in_a_fresh_process_still_writes_and_restores_every_root() {
    let dir = scratch("clean");
    let cache = dir.join("cache");
    std::fs::create_dir_all(cache.join("brood")).expect("cache");
    // Only `stdimage` itself is loaded — the state `nest stdimage` builds from.
    let (code, out) = brood(
        &dir,
        &cache,
        "(require-one 'stdimage)\n(io/puts (if (stdimage/build) \"WROTE\" \"no-cache\"))\n",
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("WROTE"), "{out}");
    assert_eq!(image_files(&cache).len(), 1, "{:?}", image_files(&cache));
    // A fresh process materialising `project` from THAT image sees its root dynamic.
    let (code, out) = brood(
        &dir,
        &cache,
        "(require-one 'project)\n(io/puts \"bound=\" (bound? '*ns-package*))\n",
    );
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("bound= true"), "{out}");
}
