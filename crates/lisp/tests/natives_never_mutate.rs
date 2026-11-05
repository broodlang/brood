//! **Brood data is immutable, and no primitive may change that** (ADR-026, CLAUDE.md:
//! "there are no data-mutation primitives and none may ever be added").
//!
//! The rule is about behaviour, not names, so this test checks the behaviour: it calls every
//! native primitive the image binds with freshly built aggregates — list, vector, map, set,
//! string, rope, nested, ratio, decimal — in one, two and three-argument shapes, and fails if
//! any argument prints differently afterwards. A `vector-set!` or a transient builder that
//! writes through its argument fails here whatever it is called, which a name pattern cannot
//! promise. The arguments are built from a run-time number, so none is a literal constant two
//! calls could share.
//!
//! Natives that act on the world outside the heap (exit, block on input, open windows, touch
//! the clipboard or terminal, spawn, dial out, write files) are skipped — by EXACT name, so a
//! new primitive is probed unless someone decides, here, that it must not be. The run happens
//! in a scratch directory, so a probed file primitive only ever writes there.
//!
//! The one mutable structure is `Value::Table`; a table is not in the argument pool, so table
//! primitives only ever see aggregates they reject.
//!
//! Alongside, the names the rule forbids must stay unbound — as globals and as special forms.

use brood::Interp;

/// Natives the probe must not call: each acts on something outside the heap. Not a list of
/// what may mutate — the mutation rule holds for these too; they are simply unsafe to call
/// with junk arguments in a test process.
const EFFECTFUL: &[&str] = &[
    // ends or stalls the process (`%force-panic` exists in debug-assertion builds only)
    "exit",
    "%halt",
    "%force-panic",
    "%hibernate",
    "%receive",
    "%read-all",
    "%read-first",
    "%read-line-start",
    // the user's desktop: windows, sound, clipboard, terminal
    "%audio-beep",
    "%clipboard-get",
    "%clipboard-image",
    "%clipboard-set",
    "%clipboard-set-image",
    "%primary-get",
    "%primary-set",
    "%gui-bg!",
    "%gui-fg!",
    "%gui-cell-size",
    "%gui-close",
    "%gui-drag-move",
    "%gui-drag-resize",
    "%gui-draw",
    "%gui-focus",
    "%gui-font!",
    "%gui-font-register",
    "%gui-fullscreen!",
    "%gui-grab-cursor",
    "%gui-held-key",
    "%gui-key-repeats",
    "%gui-icon!",
    "%gui-input!",
    "%gui-inset!",
    "%gui-line-height!",
    "%gui-maximize!",
    "%gui-minimize!",
    "%gui-open",
    "%gui-size",
    "%gui-size-px",
    "%gui-sound",
    "%gui-sound-free",
    "%gui-text-aa!",
    "%gui-text-contrast!",
    "%gui-texture",
    "%gui-texture-free",
    "%gui-title!",
    "%term-draw",
    "%term-emit",
    "%term-enter",
    "%term-leave",
    "%term-poll",
    "%term-raw-enter",
    "%term-raw-leave",
    "%term-size",
    "%pty-resize",
    "%pty-spawn",
    // output the test runner would show as noise
    "%eprint",
    "%write-err",
    "%write-out",
    "*err*",
    "*out*",
    // processes, threads and subprocesses
    "%spawn",
    "%spawn-link",
    "%spawn-monitor",
    "%spawn-named",
    "%offload",
    "%os-cmd",
    "%proc-close",
    "%proc-send",
    "%proc-set-binary",
    "%proc-signal",
    "%proc-spawn",
    "%run-process",
    "%run-program-file",
    // the network and other nodes
    "%disconnect",
    "%monitor-node",
    "%demonitor-node",
    "%node-also-listen",
    "%node-connect",
    "%node-listen",
    "%tcp-close",
    "%tcp-connect",
    "%tcp-controlling-process",
    "%tcp-listen",
    "%tcp-local-port",
    "%tcp-send",
    "%tcp-set-binary",
    "%tcp-set-idle-timeout",
    "%tcp-set-stall-timeout",
    "%tls-listen",
    "%tls-request",
    "%tls-self-signed",
    "%git-changed-files",
    "%git-clone",
    "%git-list-tags",
    "%git-resolve-ref",
    // the filesystem beyond the scratch directory's own files
    "%file-swap",
    "%image-write",
    "%rm-rf",
    // evaluation, loading, and runtime-wide switches other natives' calls would trip over
    "%isolate",
    "%isolate-discard-loads",
    "%load-module-source",
    "%load-string",
    "%load-journal-open",
    "%load-journal-close",
    "%install-interrupt-handler",
    "%restore-interrupt-handler",
    "%profile-start",
    "%profile-stop",
    "%gc-collect",
    "%gc-trace",
    "%runtime-collect",
    "%wasm-call",
    "%wasm-close",
    "%wasm-exports",
    "%wasm-load",
];

/// The names ADR-026 rules out. Bound as anything, they would be the feature under its
/// usual name.
const FORBIDDEN: &[&str] = &[
    "set!",
    "setq",
    "while",
    "set-car!",
    "set-cdr!",
    "vector-set!",
    "string-set!",
    "aset",
    "transient",
    "persistent!",
    "assoc!",
    "dissoc!",
    "conj!",
    "disj!",
    "pop!",
    "atom",
    "swap!",
    "reset!",
    "compare-and-set!",
    "volatile!",
    "vswap!",
    "vreset!",
    "ref-set",
    "alter",
    "commute",
    "box",
    "set-box!",
    "make-cell",
    "cell-set!",
];

const PROBE: &str = r#"
(def skip (set/set (map (string/split *immut-skip* ",") symbol)))

(defn fresh (n)
  [(list n (+ n 1) (+ n 2))
   (vector n (+ n 1) (+ n 2))
   (hash-map :a n :b (+ n 1))
   (conj #{} n (+ n 1))
   (str "ab" n)
   (%string->rope (str "line" n "\nmore"))
   (vector (hash-map :k (vector n n)) (list (str n)))
   (/ (+ n 3) (+ n 4))
   (decimal/of (+ n 1.25))])

(def scalars [0 1 :k "x"])

(defn arg-lists (n)
  (let (aggs (fresh n))
    (append
      (map aggs (fn (a) [a]))
      (mapcat aggs (fn (a) (map scalars (fn (s) [a s]))))
      (mapcat aggs (fn (a) (map scalars (fn (s) [s a]))))
      (mapcat aggs (fn (a) (mapcat scalars (fn (s) (map scalars (fn (t) [a s t]))))))
      (mapcat aggs (fn (a) (map (fresh (+ n 10)) (fn (b) [a b])))))))

(defn probe (name f acc)
  (fold (arg-lists 5) acc (fn ([bad ok] args)
    (let (before (map args pr-str)
          returned (check-allow :discarded-catch (try (do (apply f args) true) (catch _ false)))
          after (map args pr-str))
      [(if (= before after) bad (conj bad (str name " " (pr-str before) " -> " (pr-str after))))
       (if returned (+ ok 1) ok)]))))

(def names (seq/filter (reflect/global-names)
             (fn (s) (and (= :native (type-of (reflect/eval s))) (not (contains? skip s))))))

(def *immut-result*
  (let ([bad ok] (fold names [[] 0] (fn (acc s) (probe s (reflect/eval s) acc))))
    [(count names) ok bad]))
"#;

#[test]
fn no_native_changes_an_argument() {
    // The probe calls file primitives with junk names; keep whatever they write out of the
    // repository. This binary has one test, so the process-wide `set_current_dir` is safe.
    let scratch = std::env::temp_dir().join(format!("brood-immut-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("create the scratch directory");
    std::env::set_current_dir(&scratch).expect("cd to the scratch directory");

    let mut interp = Interp::new();
    interp
        .eval_str(&format!("(def *immut-skip* {:?})", EFFECTFUL.join(",")))
        .expect("bind the skip list");
    interp.eval_str(PROBE).expect("the probe runs");
    let mut read = |form: &str| {
        let v = interp.eval_str(form).expect("read the probe's result");
        brood::syntax::printer::print(&interp.heap, v)
    };
    let probed: usize = read("(nth *immut-result* 0)")
        .parse()
        .expect("probed count");
    let ok: usize = read("(nth *immut-result* 1)").parse().expect("ok count");
    let bad = read("(nth *immut-result* 2)");
    let _ = std::env::set_current_dir(std::env::temp_dir());
    let _ = std::fs::remove_dir_all(&scratch);

    // The floors keep the test honest: a probe that bound nothing, or whose calls all
    // raised before reaching the primitive, would report no mutation too.
    assert!(
        probed >= 300,
        "only {probed} natives probed — the probe lost its targets"
    );
    assert!(
        ok >= 2_000,
        "only {ok} calls returned — the probe is not reaching the primitives"
    );
    assert_eq!(bad, "[]", "a native changed its argument (ADR-026):\n{bad}");
}

#[test]
fn the_forbidden_mutation_names_stay_unbound() {
    let mut interp = Interp::new();
    let specials = interp
        .eval_str("(map (reflect/special-forms) str)")
        .expect("list special forms");
    let specials = brood::syntax::printer::print(&interp.heap, specials);
    let globals: std::collections::HashSet<String> = interp
        .heap
        .global_symbols()
        .into_iter()
        .map(brood::core::value::symbol_name)
        .collect();
    let mut found = Vec::new();
    for name in FORBIDDEN {
        if globals.contains(*name) {
            found.push(format!("global `{name}`"));
        }
        if specials.contains(&format!("\"{name}\"")) {
            found.push(format!("special form `{name}`"));
        }
    }
    assert!(
        specials.contains("\"def\""),
        "the special-form listing is empty or changed shape: {specials}"
    );
    assert!(
        found.is_empty(),
        "ADR-026 forbids these, and they are bound: {found:?}"
    );
}
