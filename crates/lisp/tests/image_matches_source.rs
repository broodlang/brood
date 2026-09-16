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

//! **This test owns its cache, and that is load-bearing.** With a current image on disk
//! `Interp::new()` installs at boot, so the SOURCE arm materialises too and the two arms
//! agree *by accident*; nextest's own setup script builds an image before the test starts, so
//! for as long as this test read the machine's cache it was green locally on every run and
//! red on the first genuinely cold one — which is how `*std-image-installed*` was found
//! missing from `INSTALL_BOOKKEEPING` (2026-09-04), by CI rather than by anything here.
//! Pointing `XDG_CACHE_HOME` at a fresh directory makes both arms well-defined whatever the
//! machine happens to be holding: the source arm has no image to install, and the image arm
//! builds its own. A test whose verdict depends on the developer's cache is not a gate.

use brood::Interp;

/// The install's own bookkeeping, defined once in the runtime — see
/// [`brood::boot::INSTALL_BOOKKEEPING`] for why these are skipped and where they are asserted
/// instead.
use brood::boot::INSTALL_BOOKKEEPING;

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

/// A private cache directory for this test process, removed and recreated so a previous
/// run's artifacts cannot decide this one's verdict.
fn own_the_cache() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("brood-image-differential-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create a private cache dir");
    // Safe here: nextest runs every test in its own process (`make test` is nextest), so
    // nothing else is reading the environment concurrently.
    unsafe { std::env::set_var("XDG_CACHE_HOME", &dir) };
    dir
}

#[test]
fn an_imaged_module_binds_what_its_source_binds() {
    let cache = own_the_cache();
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
        let _ = std::fs::remove_dir_all(&cache);
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

/// KI-136's differential: for EVERY module, what its load leaves in EVERY registry —
/// each entry's key, and for a two-level registry the inner keys — materialised, against
/// loaded from source. Each module is loaded alone inside `%isolate-discard-loads`, so a
/// module is credited with exactly what its own load (dependencies included) registers.
///
/// The gate above compares names, kinds, privacy and signatures, and could not see the
/// class this one exists for: the image replayed registrations a module never made —
/// `editor/face`'s section carried every face any std module `def-face`d at build time
/// (26) where its source load registers 6 — so a module resolving faces another module
/// declares worked from the image and not from source, and the only witness was a test
/// that passed on one binary and failed on another (KI-136).
///
/// `*features-loading*` is in-flight state and `*require-edges*` is installed for every
/// module at once by the image; neither is a registration.
const REGISTRY_SNAPSHOT: &str = r#"
    (do
      (defn- %reg-owner (r path kv)
        (cond
          (= r '*module-docs*) (first kv)
          (= r '*impl-from*) (->string (second kv))
          (= r '*impls*) (let (f (get *impl-from* [(first (first path)) (second (first path)) (second path)]))
                           (when f (->string f)))
          else (let (w (%registry-writer r path)) (when w (->string w)))))
      (defn- %mine? (m owner) (or (nil? owner) (= owner m)))
      (defn- %reg-lines (m r v)
        (if (map? v)
          (sort
            (apply append
              (map (%map-pairs v)
                   (fn (kv)
                     (let (k1 (first kv)
                           v1 (second kv)
                           owner (%reg-owner r [k1] kv))
                       (cond
                         (and owner (= owner m))
                           (list (str "  " (->string r) " " (str k1) " <- " owner))
                         (and (nil? owner) (map? v1))
                           (seq/keep (%map-pairs v1)
                                     (fn (kv2)
                                       (let (o2 (%reg-owner r [k1 (first kv2)] kv2))
                                         (when (%mine? m o2)
                                           (str "  " (->string r) " " (str k1) " " (str (first kv2))
                                                " <- " (or o2 "-"))))))
                         (nil? owner) (list (str "  " (->string r) " " (str k1) " <- -"))
                         else (list)))))))
          (list)))
      (apply str
        (map (cons "%baseline" (reflect/builtin-modules))
             (fn (m)
               (%isolate-discard-loads
                 (fn ()
                   (when (not (= m "%baseline")) (try (require-one m) (catch _ nil)))
                   (str m "\n"
                        (string/join
                          (apply append
                            (map (sort (%registry-names))
                                 (fn (r)
                                   (if (contains? #{'*features* '*features-loading* '*require-edges* '*module-files*} r)
                                     (list)
                                     (%reg-lines m r (if (bound? r) (reflect/eval r) nil))))))
                          "\n")
                        "\n")))))))
"#;

fn registry_snapshot(install_image: bool) -> String {
    if install_image {
        // Build in a runtime of its own: the build LOADS every module from source into the
        // process that runs it, and a snapshot taken there would see every registration
        // whichever module it had just materialised.
        let mut builder = Interp::new();
        builder
            .eval_str("(stdimage/build)")
            .expect("build the stdlib image");
    }
    let mut interp = Interp::new();
    if install_image {
        let installed = interp
            .eval_str("(%std-image-install)")
            .map(|v| interp.print(v))
            .expect("install the stdlib image");
        assert_ne!(
            installed, "nil",
            "could not install the stdlib image just built"
        );
    }
    let v = interp
        .eval_str(REGISTRY_SNAPSHOT)
        .expect("snapshot the registries");
    match v.unpack() {
        brood::core::value::ValueRef::Str(id) => interp.heap.string(id).to_string(),
        other => panic!("snapshot did not produce a string: {other:?}"),
    }
}

#[test]
fn an_imaged_module_registers_what_its_source_registers() {
    let cache = own_the_cache();
    let from_source = registry_snapshot(false);
    let from_image = registry_snapshot(true);
    if from_source == from_image {
        let _ = std::fs::remove_dir_all(&cache);
        return;
    }
    // Report per module: the entries only one arm registers after that module's load.
    let by_module = |s: &str| -> Vec<(String, Vec<String>)> {
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for line in s.lines() {
            if let Some(entry) = line.strip_prefix("  ") {
                if let Some(last) = out.last_mut() {
                    last.1.push(entry.to_string());
                }
            } else if !line.is_empty() {
                out.push((line.to_string(), Vec::new()));
            }
        }
        out
    };
    // Each arm against its own BASELINE — the same snapshot with no module loaded — so what
    // is compared is what loading the module ADDED. The two arms do not start equal: the
    // image install materialises a few modules up front, and the snapshot's own `sort`/`str`
    // lazily load `seq`/`math` from source inside every isolate.
    let src = by_module(&from_source);
    let img = by_module(&from_image);
    let (src_base, src) = src.split_first().expect("a baseline");
    let (img_base, img) = img.split_first().expect("a baseline");
    let mut report = String::new();
    for ((m, s), (_, i)) in src.iter().zip(img.iter()) {
        let s: Vec<&String> = s.iter().filter(|e| !src_base.1.contains(e)).collect();
        let i: Vec<&String> = i.iter().filter(|e| !img_base.1.contains(e)).collect();
        let only_src: Vec<&&String> = s.iter().filter(|e| !i.contains(e)).collect();
        let only_img: Vec<&&String> = i.iter().filter(|e| !s.contains(e)).collect();
        if only_src.is_empty() && only_img.is_empty() {
            continue;
        }
        report.push_str(&format!(
            "{m}: {} only from source, {} only from the image\n",
            only_src.len(),
            only_img.len()
        ));
        for e in only_src.iter().take(6) {
            report.push_str(&format!("  source only: {e}\n"));
        }
        for e in only_img.iter().take(6) {
            report.push_str(&format!("  image only : {e}\n"));
        }
    }
    panic!("materialising a module registered something its source load does not (or the reverse):\n{report}");
}
