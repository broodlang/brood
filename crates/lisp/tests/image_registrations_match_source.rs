//! **Materialising a module from the startup image must register what loading its source
//! registers** — the per-module half of the ADR-280 differential (KI-136), in a binary of
//! its own because it owns its cache through `set_var` (`env_isolation` requires an
//! env-mutating test to be alone in its binary — KI-86).
//!
//! `image_matches_source.rs` compares names, kinds, privacy and signatures and could not see
//! the class this one exists for: the image replayed registrations a module never made —
//! `editor/face`'s section carried every face any std module `def-face`d at build time (26)
//! where its source load registers 6 — so a module resolving faces another module declares
//! worked from the image and not from source, and the only witness was a test that passed
//! on one binary and failed on another.

use brood::Interp;

/// A private cache directory for this test process, removed and recreated so a previous
/// run's artifacts cannot decide this one's verdict — see `image_matches_source.rs`.
fn own_the_cache() -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("brood-image-registrations-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create a private cache dir");
    // Safe here: this binary holds one test, so nothing else reads the environment.
    unsafe { std::env::set_var("XDG_CACHE_HOME", &dir) };
    dir
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
