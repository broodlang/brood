//! **Materialising a module from the startup image must leave the same state as loading
//! its source.** One differential, over every baked-in module at once.
//!
//! The image's whole history is divergences found one at a time, by symptom, each silent:
//!
//! - declared **sigs** were not written at all for a named section, so `nest check` lost
//!   every std signature and the reversed-argument lint had nothing to compare against —
//!   a gate that stopped gating (`image_section_sigs.rs`);
//! - **require edges** were not replayed, so an imaged start built a heap with holes
//!   (ADR-256);
//! - **ability impls/registrations** were not replayed, so a restored `http` was bound but
//!   not dispatchable (ADR-256);
//! - `provide` ran before the edges, so a racing process saw a module whose dependencies
//!   were missing (ADR-256);
//! - and a section replaced an autoload **stub** before its own private helpers were
//!   bound, so a racing process died on `unbound symbol: string/whitespace?` — KI-72,
//!   which read as a scheduler hang for two sessions (ADR-279).
//!
//! Five, all of the same shape: *materialising defines bindings and evaluates nothing*, so
//! anything the evaluation would have done has to be replayed, and anything the evaluation
//! would have recorded has to be written. Each was caught by a downstream symptom rather
//! than by construction, which is why this test compares the two paths directly instead of
//! testing one more consequence.
//!
//! What is compared, for every global both arms bind: its **name**, its **kind**
//! (`:fn`/`:macro`/`:native`/data — the distinction `KIND_MACRO` exists to preserve), its
//! **privacy** (ADR-146, which decides `(:use)` refer-all and doc visibility), and its
//! **declared signature**. Values are deliberately not compared: two closures built by
//! different routes are not `=`, and the class of defect this guards has never been a wrong
//! value — it has been a missing name, a lost attribute, or a wrong kind.

//! **Run this against a genuinely empty cache when you change what it compares.** With a
//! current image on disk `Interp::new()` installs at boot, so the SOURCE arm materialises too
//! and the two arms can agree by accident — and nextest's own setup script builds an image
//! before the test starts, so no invocation through nextest can reproduce the cold state. Run
//! the test binary directly with an empty `XDG_CACHE_HOME` to see what CI sees; that is how
//! `*std-image-installed*` was found missing from `INSTALL_BOOKKEEPING` (2026-09-04), green
//! locally on every run and red on the first cold one.

use brood::Interp;

/// The install's **own** bookkeeping, which legitimately differs between the arms: the
/// image arm populates these by installing, the source arm never touches them. Excluded by
/// name rather than by dropping every root global, because a module's root globals are
/// exactly what broke once before — `*lineedit-keymap*` was credited to `repl`'s section
/// and `(require 'editor/lineedit)` restored the module without it.
const INSTALL_BOOKKEEPING: [&str; 7] = [
    "*image-sources*",
    "*std-image-file*",
    // How many sections THIS process installed. It is a fact about the process, not about
    // any module, and differing between the arms is its entire purpose — but only when the
    // cache is empty. With a current image on disk `Interp::new()` installs at boot, so the
    // SOURCE arm reads an int too and the arms agree by accident; CI's cache is cold, which
    // is why this was green locally and red there.
    "*std-image-installed*",
    "*std-image-sections*",
    "*std-impls*",
    "*std-regs*",
    "*std-require-edges*",
];

/// Every global, with the attributes materialisation has historically dropped. Sorted, so
/// the two arms are comparable line by line and a diff names the offender.
const SNAPSHOT: &str = r#"
    (do
      (doseq (m (reflect/builtin-modules)) (try (require-one m) (catch _ nil)))
      (apply str
        (map (sort (reflect/global-names)) (fn (s)
               (str (->string s)
                    " " (->string (type-of (reflect/eval s)))
                    (if (reflect/private? s) " private" "")
                    " :: " (or (try (reflect/type-signature s) (catch _ nil)) "-")
                    "\n")))))
"#;

fn snapshot(install_image: bool) -> String {
    let mut interp = Interp::new();
    if install_image {
        // Build one if there is no current image, then install. The id carries the git sha
        // and a hash of every `.blsp`, so *any* commit or std edit makes the previous image
        // stale — and an install that misses returns nil and leaves this arm as the SOURCE
        // arm wearing the image's name. That is KI-72's signature trap, and a test that
        // merely asks the developer to build first would take it on every commit.
        //
        // Building here is also the only *correct* order: nothing is installed yet, so the
        // build reads the modules from source. Building while an image is installed
        // re-encodes the materialised state and launders any divergence into the next
        // image (ADR-280).
        let installed = interp
            .eval_str("(or (%std-image-install) (do (stdimage/build) (%std-image-install)))")
            .map(|v| interp.print(v))
            .expect("install the stdlib image");
        assert_ne!(
            installed, "nil",
            "could not install a stdlib image even after building one, so this differential \
             would compare source against source and pass vacuously",
        );
    }
    let v = interp.eval_str(SNAPSHOT).expect("snapshot the globals");
    // The RAW string, not `print`'s quoted-and-escaped rendering — the snapshot is
    // newline-separated and the comparison below is line-by-line.
    match v.unpack() {
        brood::core::value::ValueRef::Str(id) => interp.heap.string(id).to_string(),
        other => panic!("snapshot did not produce a string: {other:?}"),
    }
}

#[test]
fn an_imaged_module_binds_what_its_source_binds() {
    let from_source = snapshot(false);
    let from_image = snapshot(true);
    // Name the offenders rather than the byte offset: a diff of ~3000 lines is unreadable,
    // and the answer is always "which names differ, and how".
    let keep = |l: &&str| {
        let name = l.split(' ').next().unwrap_or(l);
        !INSTALL_BOOKKEEPING.contains(&name)
    };
    let src: Vec<&str> = from_source.lines().filter(keep).collect();
    let img: Vec<&str> = from_image.lines().filter(keep).collect();
    let only_in = |a: &[&str], b: &[&str]| -> Vec<String> {
        let other: std::collections::HashSet<&str> =
            b.iter().map(|l| l.split(' ').next().unwrap_or(l)).collect();
        a.iter()
            .filter(|l| !other.contains(l.split(' ').next().unwrap_or(l)))
            .map(|l| (*l).to_string())
            .collect()
    };
    if src == img {
        return;
    }
    let missing = only_in(&src, &img);
    let extra = only_in(&img, &src);
    let attr: Vec<String> = {
        let by_name: std::collections::HashMap<&str, &str> = img
            .iter()
            .map(|l| (l.split(' ').next().unwrap_or(l), *l))
            .collect();
        src.iter()
            .filter_map(|l| {
                let name = l.split(' ').next().unwrap_or(l);
                by_name
                    .get(name)
                    .filter(|imaged| **imaged != *l)
                    .map(|imaged| format!("  source: {l}\n  image : {imaged}"))
            })
            .collect()
    };
    panic!(
        "materialising diverged from loading the source.\n\
         {} name(s) the image does not bind:\n{}\n\
         {} name(s) only the image binds:\n{}\n\
         {} name(s) whose kind/privacy/signature differ:\n{}",
        missing.len(),
        missing
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
        extra.len(),
        extra
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
        attr.len(),
        attr.iter().take(10).cloned().collect::<Vec<_>>().join("\n"),
    );
}
