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

(let (names (sort (reflect/global-names)))
  (io/puts "GLOBALS " (count names))
  (doseq (n names)
    (io/puts n
             " kind=" (->string (type-of (reflect/eval (symbol n))))
             " private=" (->string (reflect/private? (symbol n)))
             " sig=" (->string (reflect/type-signature n))
             " loc=" (->string (reflect/source-location n))
             " dyn=" (->string (dyn? n)))))
"#;

fn dump(use_image: bool) -> (String, String) {
    let path = std::env::temp_dir().join("brood-prelude-differential.blsp");
    let mut f = std::fs::File::create(&path).expect("create dump program");
    f.write_all(DUMP.as_bytes()).expect("write dump program");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_BOOT_TRACE", "1");
    if use_image {
        cmd.env("BROOD_PRELUDE_IMAGE", "1");
    }
    let out = cmd.arg(&path).output().expect("run brood");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Runs with **no exclusions** — every global is compared. An earlier version excluded the
/// six stdlib-image install-bookkeeping names because the two arms disagreed about them,
/// and one of those disagreements *was* a live bug: the imaged boot restored a stale
/// `*image-sources*`, whose symptom was `unbound symbol: io/puts` on a tree where nothing
/// was wrong with `io`. Excluding a global because the arms disagree is excluding the
/// evidence. They agree now that `%std-image-install` is replayed on the imaged path, so
/// the exclusions are gone and must stay gone.
#[test]
fn an_imaged_boot_and_a_source_boot_agree_on_every_global() {
    // Warm both artifacts first: the very first run of a fresh binary boots from source and
    // writes them, and comparing against that run would compare source with source.
    let _ = dump(true);

    let (image_out, image_err) = dump(true);
    let (text_out, text_err) = dump(false);

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
        assert_eq!(
            out.lines().count(),
            n + 1,
            "the {label} arm's line count does not match its own header — the dump stopped early"
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
