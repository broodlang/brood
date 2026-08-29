//! A baked-in std module's forms must be attributed to the file they were WRITTEN in.
//!
//! `std/` is compiled into the binary, so a std module has no path at runtime and
//! `%load-string` originally set none — its forms inherited whatever file happened to be
//! mid-load when the `require` ran. The visible damage: line coverage credited a 21-line
//! `src/main.blsp` with `std/log`'s lines 127-131 and 175. The same field feeds
//! `CompiledArm::src_file`, hence `:trace` frames, so it was never coverage-only.
//!
//! The property that catches it, and the one asserted here: **every recorded line must
//! exist inside the file it is attributed to.** A line number from a different file
//! almost always lands past the end of the file it was credited to, which is a check
//! that needs no knowledge of what the lines contain.
//!
//! Line coverage is used as the readout because it is the one place these attributions
//! are directly observable. The bug is not a coverage bug.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

mod support;

fn repo_root() -> PathBuf {
    // .../crates/cli -> repo root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

struct TempFile {
    path: PathBuf,
}
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// `file -> the lines the compiler instrumented in it`, harvested from a run that
/// requires a few std modules with several call-bearing bodies.
fn instrumented_lines() -> (BTreeMap<String, Vec<u32>>, TempFile) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let script = TempFile {
        path: std::env::temp_dir().join(format!("brood-attr-{}-{nanos}.blsp", std::process::id())),
    };
    // `log` is the module the original misattribution was found on; the others are
    // ordinary multi-function modules. Their bodies are force-compiled rather than
    // called: an arm registers its lines when it COMPILES, which happens on first call,
    // and precompiling is both cheaper than arranging real calls and the exact path the
    // misattribution was found on (`nest test --cover-lines` does this before the suite).
    std::fs::write(
        &script.path,
        "(require-one 'log)\n(require-one 'set)\n(require-one 'json)\n\
         (fold (fn (_ s)\n\
                 (when (= (type-of (reflect/eval s)) :fn) (%coverage-precompile (reflect/eval s))))\n\
           nil (reflect/global-names))\n\
         (fold (fn (_ e) (io/puts (str \"ATTR \" (seq/vector-ref e 0) \" \" (seq/vector-ref e 1)))) \
         nil (%coverage-instrumented))\n",
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg(&script.path)
        .env("BROOD_COVERAGE", "1")
        .env("BROOD_NO_JIT", "1")
        // Line-coverage instrumentation is a VM-compiler pass (the `RecordLine`
        // opcode), so pin the VM: the tree-walker differential job sets `BROOD_VM=0`
        // in the environment, which this subprocess would otherwise inherit — running
        // the script on the tree-walker, where no lines get instrumented and the
        // attribution this test checks never happens.
        .env("BROOD_VM", "1")
        .current_dir(repo_root());
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run brood");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "the probe script should run:\n{}",
        text
    );

    let mut found = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("ATTR ") else {
            continue;
        };
        let Some((file, lines)) = rest.split_once(' ') else {
            continue;
        };
        let lines: Vec<u32> = lines
            .trim_matches(|c| c == '(' || c == ')')
            .split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect();
        found.insert(file.to_string(), lines);
    }
    assert!(
        !found.is_empty(),
        "the coverage readout should not be empty — is BROOD_COVERAGE still honoured?\n{}",
        text
    );
    (found, script)
}

#[test]
fn a_baked_in_std_module_is_attributed_to_its_own_source_file() {
    let (found, _script) = instrumented_lines();
    let root = repo_root();

    let std_files: Vec<&String> = found.keys().filter(|f| f.starts_with("std/")).collect();
    assert!(
        !std_files.is_empty(),
        "no std module was attributed to a std/ path — the requiring file's name is \
         probably being used again. Got: {:?}",
        found.keys().collect::<Vec<_>>()
    );
    // The module the bug was found on, and the reason it is in the probe script.
    assert!(
        found.contains_key("std/log.blsp"),
        "std/log.blsp should be attributed to itself. Got: {:?}",
        found.keys().collect::<Vec<_>>()
    );

    for file in std_files {
        let path = root.join(file);
        assert!(
            path.is_file(),
            "`{file}` should be a real path in the source tree, openable by a tool that \
             is handed it"
        );
        let count = std::fs::read_to_string(&path).unwrap().lines().count() as u32;
        let recorded = &found[file];
        let past_end: Vec<u32> = recorded.iter().copied().filter(|l| *l > count).collect();
        assert!(
            past_end.is_empty(),
            "{file} has {count} lines but was credited with {past_end:?} — those lines \
             belong to some other file"
        );
        assert!(
            recorded.iter().all(|l| *l >= 1),
            "{file}: line numbers must be 1-based, got {recorded:?}"
        );
    }
}

/// The complement: the probe script itself is a real file on disk and keeps its own
/// (absolute) path, so the fix didn't push everything through the embedded naming.
#[test]
fn a_file_loaded_from_disk_keeps_its_own_path() {
    let (found, script) = instrumented_lines();
    let key = script.path.to_string_lossy().to_string();
    assert!(
        found.contains_key(&key),
        "the script's own forms should be attributed to its path. Got: {:?}",
        found.keys().collect::<Vec<_>>()
    );
    let count = std::fs::read_to_string(&script.path)
        .unwrap()
        .lines()
        .count() as u32;
    assert!(
        found[&key].iter().all(|l| *l <= count),
        "the script has {count} lines but was credited with {:?}",
        found[&key]
    );
}
