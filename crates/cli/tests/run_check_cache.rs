//! **The run pre-flight's verdict cache** (`cli_support::run_check_cache_*`): `brood file`
//! type-checks a program before running it, and the checker's walk scales with the file —
//! 34M instructions on the `pipeline` benchmark row (18.7% of the run), paid on every run
//! of an unchanged program. The second run of the same text under the same binary replays
//! the verdict instead.
//!
//! What this pins, each against a private `XDG_CACHE_HOME`:
//! - a miss writes one entry and a hit prints the same warnings — and the hit really is a
//!   replay: an entry edited by hand is what the next run prints (a walk would print the
//!   truth, so this cannot pass by re-checking);
//! - a check that loaded a module from the LOAD-PATH writes nothing, and the verdict then
//!   follows an edit to that module (the soundness condition — that file's content is not
//!   in the key);
//! - `BROOD_NO_CHECK_CACHE=1` neither reads nor writes;
//! - `brood --check` never reads the cache: the authoritative entry point prints the walk it
//!   just did, even with a doctored entry in place.

use std::path::{Path, PathBuf};
use std::process::Command;

const WARNING_PROGRAM: &str = "(defn f (x) (+ x 1))\n(io/puts (string/length 3))\n";
const WARNING_LINE: &str = "warning: string/length: argument 1 expects string, got 3 (3)";

struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "brood-run-check-cache-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("sandbox dir");
        Sandbox { dir }
    }
    fn cache_home(&self) -> PathBuf {
        self.dir.join("cache")
    }
    fn entries(&self) -> Vec<PathBuf> {
        let d = self.cache_home().join("brood").join("run-check");
        match std::fs::read_dir(&d) {
            Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
            Err(_) => Vec::new(),
        }
    }
    fn write(&self, name: &str, src: &str) -> PathBuf {
        let p = self.dir.join(name);
        std::fs::write(&p, src).expect("write program");
        p
    }
    /// Run `brood` with `args` from `cwd`, returning stderr (where the pre-flight prints).
    fn brood(&self, cwd: &Path, args: &[&str], extra_env: &[(&str, &str)]) -> String {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
        cmd.current_dir(cwd)
            .env("XDG_CACHE_HOME", self.cache_home())
            .env("BROOD_NO_CRASH_REPORT", "1")
            .env_remove("BROOD_NO_CHECK")
            .env_remove("BROOD_NO_CHECK_CACHE")
            .env_remove("BROOD_CHECK_STRICT")
            .args(args);
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("run brood");
        String::from_utf8_lossy(&out.stderr).into_owned()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_hit_replays_the_recorded_verdict_and_a_miss_records_one() {
    let sb = Sandbox::new("hit");
    let prog = sb.write("warn.blsp", WARNING_PROGRAM);
    let p = prog.to_str().unwrap();
    assert!(
        sb.entries().is_empty(),
        "the sandbox cache must start empty"
    );
    let first = sb.brood(&sb.dir, &[p], &[]);
    assert!(
        first.contains(WARNING_LINE),
        "the miss prints the walk's warning:\n{first}"
    );
    let entries = sb.entries();
    assert_eq!(entries.len(), 1, "one entry after the miss: {entries:?}");
    let second = sb.brood(&sb.dir, &[p], &[]);
    assert!(
        second.contains(WARNING_LINE),
        "the hit prints the same warning:\n{second}"
    );
    assert_eq!(sb.entries().len(), 1, "a hit writes nothing");
    // Doctor the entry: if the second run really replays rather than re-walks, the
    // doctored text is what it prints. Sabotage-proof by construction — a re-walk prints
    // the true warning and cannot contain this string.
    let text = std::fs::read_to_string(&entries[0]).expect("read entry");
    let doctored = text.replace("string/length", "CACHED-VERDICT-MARKER");
    assert_ne!(
        text, doctored,
        "the entry carries the warning text:\n{text}"
    );
    std::fs::write(&entries[0], doctored).expect("doctor entry");
    let third = sb.brood(&sb.dir, &[p], &[]);
    assert!(
        third.contains("CACHED-VERDICT-MARKER"),
        "the third run did not replay the cache entry — it walked the file again:\n{third}"
    );
}

/// A hit replays the walk's LOADS, not only its warnings. The pre-flight loads the file's
/// own references eagerly and those loads survive into the run; replaying only the warnings
/// left the run to load them lazily at first use — correct, and since KI-167 no slower, but
/// the body compiled before the load recompiles (ADR-366) and that churn measured +4 MB
/// peak RSS on `(io/puts 0)` (50.7 MB against 44.7 with the walk; 44.3 with the replay).
/// Observable as the churn itself: `BROOD_TRACE_COMPILE=1` prints `[compile] stale-bindings
/// arm=<closure>: a miss on io/puts loaded its module` when the program's top-level form was
/// compiled before `io` loaded — the no-check control shows the line, the walk does not, and
/// the hit must not either. Sabotage: skip the replay → the hit prints it.
#[test]
fn a_hit_replays_the_loads_the_walk_made() {
    let sb = Sandbox::new("loads");
    // A std reference defers only under an IMAGED boot (the kind index that says a head is
    // not a macro is the image's — ADR-335); a source boot loads it at expansion and the
    // control below could not tell a replay from that. The sandbox cache starts empty, so
    // build this binary's stdlib image into it first, the way `stdimage_reporting.rs` does.
    std::fs::write(
        sb.dir.join("build-image.blsp"),
        "(require-one 'stdimage) (stdimage/build)\n",
    )
    .unwrap();
    let built = Command::new(env!("CARGO_BIN_EXE_brood"))
        .current_dir(&sb.dir)
        .env("XDG_CACHE_HOME", sb.cache_home())
        .env("BROOD_NO_CHECK", "1")
        .env_remove("BROOD_NO_STDIMAGE")
        .env_remove("BROOD_NO_PRELUDE_IMAGE")
        .arg("build-image.blsp")
        .output()
        .expect("build the stdlib image");
    assert!(
        built.status.success(),
        "building the sandbox's stdlib image failed:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let prog = sb.write("loads.blsp", "(io/puts (str \"hello \" 1))\n");
    let p = prog.to_str().unwrap();
    const STALE: &str = "[compile] stale-bindings arm=<closure>: a miss on io/puts";
    let run = |extra: &[(&str, &str)]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
        cmd.current_dir(&sb.dir)
            .env("XDG_CACHE_HOME", sb.cache_home())
            .env("BROOD_NO_CRASH_REPORT", "1")
            .env("BROOD_TRACE_COMPILE", "1")
            .env_remove("BROOD_NO_CHECK")
            .env_remove("BROOD_NO_CHECK_CACHE")
            .env_remove("BROOD_NO_STDIMAGE")
            .env_remove("BROOD_NO_PRELUDE_IMAGE")
            .arg(p);
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("run brood");
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };
    // The control: with no check at all, `io` loads lazily at the call and the form that
    // was compiled before it is marked stale — the churn the replay exists to prevent.
    let control = run(&[("BROOD_NO_CHECK", "1")]);
    assert!(
        control.contains(STALE),
        "without a check `io` must load lazily and mark the form stale, or this test cannot \
         see a replay:\n{control}"
    );
    let miss = run(&[]);
    assert!(
        miss.contains("hello 1") && !miss.contains(STALE),
        "the walk loads `io` before the form compiles:\n{miss}"
    );
    let entries = sb.entries();
    assert_eq!(entries.len(), 1);
    let text = std::fs::read_to_string(&entries[0]).unwrap();
    assert!(
        text.lines()
            .nth(1)
            .is_some_and(|l| l.starts_with("loads") && l.contains("io")),
        "the entry must record that the walk loaded `io`:\n{text}"
    );
    let hit = run(&[]);
    assert!(
        hit.contains("hello 1") && !hit.contains(STALE),
        "the hit did not replay the walk's load of `io` — the form compiled before the lazy \
         load and was marked stale:\n{hit}"
    );
}

#[test]
fn a_check_that_loaded_a_load_path_module_is_not_cached_and_follows_its_edit() {
    let sb = Sandbox::new("loadpath");
    let lp = sb.dir.join("lp");
    std::fs::create_dir_all(&lp).unwrap();
    std::fs::write(
        lp.join("mymod.blsp"),
        "(defmodule mymod)\n(defn hi (x) x)\n",
    )
    .unwrap();
    // `hi` is called with two arguments: an arity warning the checker can only produce
    // once it has LOADED `mymod` off the load-path.
    let prog = sb.write(
        "useit.blsp",
        "(defmodule useit (:use mymod))\n(io/puts (hi 1 2))\n",
    );
    let p = prog.to_str().unwrap();
    let first = sb.brood(&lp, &[p], &[]);
    assert!(
        first.contains("warning: mymod/hi: expected 1 argument"),
        "the load-path module did not load for the check — the shape under test is gone:\n{first}"
    );
    assert!(
        sb.entries().is_empty(),
        "a verdict that depends on a load-path module must not be cached: {:?}",
        sb.entries()
    );
    // The dependency changes so the call is now correct: the next run must say so.
    std::fs::write(
        lp.join("mymod.blsp"),
        "(defmodule mymod)\n(defn hi (x y) x)\n",
    )
    .unwrap();
    let second = sb.brood(&lp, &[p], &[]);
    assert!(
        !second.contains("warning: mymod/hi"),
        "the verdict did not follow the dependency's edit:\n{second}"
    );
    assert!(
        sb.entries().is_empty(),
        "still nothing cached: {:?}",
        sb.entries()
    );
}

#[test]
fn the_off_switch_neither_reads_nor_writes_and_check_never_reads() {
    let sb = Sandbox::new("off");
    let prog = sb.write("warn.blsp", WARNING_PROGRAM);
    let p = prog.to_str().unwrap();
    let off = sb.brood(&sb.dir, &[p], &[("BROOD_NO_CHECK_CACHE", "1")]);
    assert!(off.contains(WARNING_LINE), "{off}");
    assert!(
        sb.entries().is_empty(),
        "the off switch wrote an entry: {:?}",
        sb.entries()
    );
    // Seed a doctored entry through a real miss, then prove neither the off switch nor
    // `--check` reads it.
    sb.brood(&sb.dir, &[p], &[]);
    let entries = sb.entries();
    assert_eq!(entries.len(), 1);
    let text = std::fs::read_to_string(&entries[0]).unwrap();
    std::fs::write(
        &entries[0],
        text.replace("string/length", "CACHED-VERDICT-MARKER"),
    )
    .unwrap();
    let off = sb.brood(&sb.dir, &[p], &[("BROOD_NO_CHECK_CACHE", "1")]);
    assert!(
        off.contains(WARNING_LINE) && !off.contains("CACHED-VERDICT-MARKER"),
        "{off}"
    );
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    let out = cmd
        .current_dir(&sb.dir)
        .env("XDG_CACHE_HOME", sb.cache_home())
        .env_remove("BROOD_NO_CHECK_CACHE")
        .args(["--check", p])
        .output()
        .expect("run brood --check");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(WARNING_LINE) && !stdout.contains("CACHED-VERDICT-MARKER"),
        "`brood --check` must print the walk it just did, never a cached verdict:\n{stdout}"
    );
}
