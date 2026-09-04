//! **Booting from the prelude image must leave the same state as booting from source.**
//! One differential over the whole prelude, run as two real `brood` processes.
//!
//! This is the prelude's counterpart to `image_matches_source.rs`, and it exists for the
//! reason that file's header states: *materialising defines bindings and evaluates nothing*,
//! so anything the evaluation would have recorded has to be written explicitly. Building
//! ADR-314 turned up three such omissions in a row, each silent and each found only by a
//! broad wave of unrelated test failures:
//!
//! - the **`defdyn` marks** (`value::DYNAMICS`) were not carried, so `binding` rejected
//!   `*require-parent*` and every `require` in the language died;
//! - **`*out*`** vanished, because the write filtered out bindings whose *value* was a
//!   native — but a prelude `def` can bind a primitive under a name `builtins::register`
//!   never re-creates, and `io/puts` went with it;
//! - **def sites** were not carried, so stdlib `M-.` went dark — the one user-visible
//!   thing ADR-138 kept a whole positioned read alive to preserve.
//!
//! Three misses, no crash, and 185 failing tests pointing anywhere but here. A differential
//! is the only shape that catches the *fourth* one by construction.
//!
//! Also compared, once per arm: the **registry-name set** (`%registry-names`). KI-106 was a
//! disagreement there with every per-global attribute identical — the imaged boot never ran
//! the prelude's `%registry-update!` calls, so `*multi-algebra*`/`*multi-ret*` were bound but
//! not *marked*, and a multi-file `nest check` lost every derived multimethod mirror.
//!
//! What is compared, per global: **name**, **kind** (`:fn`/`:macro`/`:native`/data — the
//! distinction `KIND_MACRO` exists to preserve), **privacy** (ADR-146), **declared
//! signature**, **source location** and **dynamic-ness**. Values are deliberately not
//! compared, for the same reason the module differential skips them: two closures built by
//! different routes are not `=`, and every defect this guards has been a missing name or a
//! lost attribute, never a wrong value.

use std::io::Write;
use std::process::Command;

/// Prints one canonical line per global. Sorted, so the diff is positional.
const DUMP: &str = r#"
(defn- dyn? (n)
  "Is `n` a dynamic variable? Asked behaviourally, through the primitive `binding` uses,
so this needs no new introspection surface."
  (try (do (%binding (list (symbol n)) [nil] (fn () nil)) true)
    (catch _ (check-allow :discarded-catch false))))

(defn- loc-of (n)
  "A def site as `[basename line col]`. The FULL path is deliberately dropped: each arm runs
under its own XDG_CACHE_HOME, so the materialised `prelude.blsp` lives at a different
absolute path in each, and comparing those compares the harness rather than the boot. The
basename plus line:col still fails on a missing, wrong or shifted def site — which is the
thing worth catching (an imaged boot that records none takes stdlib `M-.` down)."
  (let (l (reflect/source-location n))
    (if (nil? l)
      "nil"
      (let (parts (string/split (->string (first l)) "/"))
        (->string [(nth parts (- (count parts) 1)) (nth l 1) (nth l 2)])))))

(io/puts "REGISTRIES " (->string (%registry-names)))
(let (names (sort (reflect/global-names)))
  (io/puts "GLOBALS " (count names))
  (doseq (n names)
    (io/puts n
             " kind=" (->string (type-of (reflect/eval (symbol n))))
             " private=" (->string (reflect/private? (symbol n)))
             " sig=" (->string (reflect/type-signature n))
             " loc=" (loc-of n)
             " dyn=" (->string (dyn? n)))))
"#;

/// One arm's private cache directory. Each arm gets its own, because the three artifacts
/// that live in `~/.cache/brood` — the prelude image, the expanded-prelude text cache and
/// the stdlib image — interact, and a differential has to own everything that differs
/// between its arms. Sharing the real cache is what made the first version of this test
/// green here and red on CI: whichever artifacts happened to be on disk decided the answer.
fn arm_cache(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("brood-prelude-diff-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create arm cache dir");
    dir
}

fn run_arm(
    program: &std::path::Path,
    cache: &std::path::Path,
    use_image: bool,
) -> (String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_BOOT_TRACE", "1")
        // Own the cache: no stdlib image exists under a fresh dir, so BOTH arms load std
        // modules from source. That asymmetry is what we are not testing, so remove it.
        .env("XDG_CACHE_HOME", cache)
        // Pin the engine. Without this the tree-walker CI job (`BROOD_VM=0`) ran this test
        // against a different engine than it was written for, and it is the boot path that
        // is under test, not the evaluator.
        .env("BROOD_TIER", "1")
        .env_remove("BROOD_VM")
        .env_remove("BROOD_NO_JIT")
        .env_remove("BROOD_NO_STDIMAGE")
        .env_remove("BROOD_COVERAGE");
    // The image is the DEFAULT (ADR-314, since 2026-09-04), so the arms are "leave it alone"
    // and "opt out". Clear BOTH spellings on BOTH arms first, so an ambient one cannot decide
    // the answer — an inherited opt-out would make the image arm take the text path and this
    // test compare the text path with itself; the path assertions below would catch that, but
    // not with a message that names the cause.
    cmd.env_remove("BROOD_PRELUDE_IMAGE")
        .env_remove("BROOD_NO_PRELUDE_IMAGE");
    if !use_image {
        cmd.env("BROOD_NO_PRELUDE_IMAGE", "1");
    }
    let out = cmd.arg(program).output().expect("run brood");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Run one arm to a steady state: the first invocation cold-boots and WRITES that arm's
/// artifacts, the second reads them. Returns the second. Built inside the test so the
/// answer never depends on what a previous run, or a setup script, happened to leave.
fn dump(program: &std::path::Path, cache: &std::path::Path, use_image: bool) -> (String, String) {
    let _warm = run_arm(program, cache, use_image);
    run_arm(program, cache, use_image)
}

fn dump_program() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "brood-prelude-differential-{}.blsp",
        std::process::id()
    ));
    let mut f = std::fs::File::create(&path).expect("create dump program");
    f.write_all(DUMP.as_bytes()).expect("write dump program");
    path
}

/// Deterministic by construction: each arm owns its cache directory and its engine, and
/// builds its own artifacts. An earlier version shared the real `~/.cache/brood` and pinned
/// neither, which made it green here and red on CI — whichever of the three interacting
/// artifacts happened to be on disk decided the answer.
#[test]
fn an_imaged_boot_and_a_source_boot_agree_on_every_global() {
    let program = dump_program();
    let (image_cache, text_cache) = (arm_cache("image"), arm_cache("text"));
    let (image_out, image_err) = dump(&program, &image_cache, true);
    let (text_out, text_err) = dump(&program, &text_cache, false);

    // ASSERT THE DUMP IS PRESENT, never merely that the arms agree. The first cut of this
    // test compared two EMPTY strings — the dump program died on an unbound `seq/sort`, and
    // "" == "" is agreement. It passed a deliberate sabotage because of it. This is
    // CLAIM-THE-SUMMARY-LINE, applied to a differential.
    for (label, out, err) in [
        ("image", &image_out, &image_err),
        ("source", &text_out, &text_err),
    ] {
        let header = out
            .lines()
            .find(|l| l.starts_with("GLOBALS "))
            .unwrap_or_else(|| panic!(
                "the {label} arm printed no GLOBALS header — the dump program did not run, so                  this test can conclude nothing.\nstdout:\n{out}\nstderr:\n{err}"
            ));
        let n: usize = header["GLOBALS ".len()..].trim().parse().unwrap_or(0);
        assert!(
            n > 500,
            "the {label} arm dumped only {n} globals; the prelude has ~1050, so the dump is \
             truncated and a diff over it proves nothing"
        );
        // Count from the header on: the REGISTRIES line precedes it.
        assert_eq!(
            out.lines()
                .skip_while(|l| !l.starts_with("GLOBALS "))
                .count(),
            n + 1,
            "the {label} arm's line count does not match its own header — the dump stopped early"
        );
        // The registry-name SET is compared too, not only the globals. KI-106: both arms
        // agreed on every global's name/kind/sig/site while the imaged one was missing
        // `*multi-algebra*` and `*multi-ret*` from the set `%registry-update!` maintains —
        // a fact recorded beside the bindings, invisible to a per-global diff.
        assert!(
            out.lines()
                .next()
                .is_some_and(|l| l.starts_with("REGISTRIES") && l.contains('(')),
            "the {label} arm printed no REGISTRIES line"
        );
    }

    // The gate is worthless unless the arms really took different paths. Without this an
    // absent/stale image makes both arms the text path and the test passes vacuously —
    // which is precisely how a boot-artifact test fails to fail.
    assert!(
        image_err.contains("(prelude image)"),
        "the image arm did not boot from the prelude image, so this test compared the text \
         path with itself. Boot trace was:\n{image_err}"
    );
    assert!(
        text_err.contains("cache hit") || text_err.contains("source boot"),
        "the text arm did not take the text-cache path. Boot trace was:\n{text_err}"
    );

    if image_out != text_out {
        let mut diffs = Vec::new();
        let (mut a, mut b) = (image_out.lines(), text_out.lines());
        loop {
            match (a.next(), b.next()) {
                (None, None) => break,
                (x, y) if x == y => continue,
                (x, y) => {
                    diffs.push(format!(
                        "  image: {}\n  source: {}",
                        x.unwrap_or("<end>"),
                        y.unwrap_or("<end>")
                    ));
                    if diffs.len() >= 20 {
                        break;
                    }
                }
            }
        }
        panic!(
            "the imaged boot and the source boot disagree — the image is missing something \
             the evaluation recorded (ADR-314). First {} difference(s):\n{}",
            diffs.len(),
            diffs.join("\n")
        );
    }
}
