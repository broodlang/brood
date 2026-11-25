//! **Native code whose name was bound when it compiled, and is not now, takes the VM's lazy
//! path instead of raising `unbound`** (ADR-803).
//!
//! `%isolate-discard-loads` rolls a module's load back. A function compiled while `seq` was
//! loaded then calls `seq/keep` with `seq` gone: the VM loads it again (`global_miss`), and
//! native code raised `unbound symbol: seq/keep`. Closures holding a literal of their own
//! going native (ADR-803) is what first put such a call in native code: the registry walk in
//! `image_registrations_match_source` failed that way. Both native miss sites are covered:
//!
//! - a TAIL call's head (and a global read in value position) deopts (`global-miss-loads`):
//!   no safepoint there, so the VM's re-run does the load;
//! - a NON-TAIL call's miss loads synchronously in `jit_dispatch_call`, a safepoint.
//!
//! The program is that test's registry walk, which reproduces the miss where every smaller
//! shape tried did not (the call inline caches have to be invalidated between rounds by real
//! module loads). Each variant must print what it prints with `BROOD_NO_JIT_CLOSURES=1`, where
//! the walk stays on the VM. Red with either fix removed (checked by switching each off).

use std::path::PathBuf;
use std::process::Command;

mod support;
use support::{TempDir, temp_dir};

const SNAPSHOT: &str = r#"
    (do
      (defn- %reg-owner (r path kv)
        (cond
          (= r '*module-docs*) (first kv)
          (= r '*impl-from*) (->string (second kv))
          (= r '*impls*) (let (f (get *impl-from* [(first (first path)) (second (first path)) (second path)]))
                           (when f (->string f)))
          ;; `[ability op id] → {tier [fn ns]}` (ADR-432): each tier's own value names its
          ;; namespace, so attribute the inner entries by it and descend from the top.
          (= r '*impl-tiers*) (when (= (count path) 2)
                                (let (f (second (second kv))) (when f (->string f))))
          else (let (w (%registry-writer r path)) (when w (->string w)))))
      (defn- %mine? (m owner) (or (nil? owner) (= owner m)))
      (defn- %reg-lines (m r v)
        (cond
          ;; A LIST registry's members, each with who added it and the op that did (ADR-595):
          ;; the image replays a member at the end of the list its source load put it.
          (list? v)
            (sort
              (seq/keep (%registry-members r)
                (fn (row)
                  ;; Owned members only: one no op credited (a whole-list swap — `repl`'s
                  ;; commands go in through `%swap-registry!`) rides the list's own value,
                  ;; and the two arms' baselines differ in which modules are already loaded,
                  ;; so an unowned member says nothing about the module being compared.
                  (let (owner (when (get row 1) (->string (get row 1))))
                    (when (and owner (= owner m))
                      (str "  " (->string r) " member " (pr-str (get row 0))
                           " " (pr-str (get row 2)) " <- " (or owner "-")))))))
          (not (map? v)) (list)
          else
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
                         else (list)))))))))
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

/// Where the walk's inner `seq/keep` sits in its closure: last (a tail call), or under
/// `identity` (a non-tail call).
fn program(tail: bool) -> String {
    let walk = if tail {
        SNAPSHOT.to_string()
    } else {
        SNAPSHOT
            .replacen(
                "(seq/keep (%map-pairs v1)",
                "(identity (seq/keep (%map-pairs v1)",
                1,
            )
            .replacen("(or o2 \"-\"))))))", "(or o2 \"-\")))))))", 1)
    };
    format!("(io/puts (str \"answer \" (count {walk})))\n")
}

fn run(dir: &TempDir, tail: bool, closures: bool) -> String {
    let file: PathBuf = dir.path.join(if tail { "tail.blsp" } else { "call.blsp" });
    std::fs::write(&file, program(tail)).expect("write fixture");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg(&file)
        .env("BROOD_NO_CHECK", "1")
        .env_remove("BROOD_TIER");
    if closures {
        cmd.env_remove("BROOD_NO_JIT_CLOSURES");
    } else {
        cmd.env("BROOD_NO_JIT_CLOSURES", "1");
    }
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run brood");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn check(tail: bool) {
    let dir = temp_dir("jit-isolate-lazy-miss");
    let baseline = run(&dir, tail, false);
    let native = run(&dir, tail, true);
    let answer = |out: &str| {
        out.lines()
            .find(|line| line.starts_with("answer "))
            .map(str::to_string)
    };
    let expected =
        answer(&baseline).unwrap_or_else(|| panic!("baseline did not finish:\n{baseline}"));
    assert_eq!(
        answer(&native).as_deref(),
        Some(expected.as_str()),
        "the walk with its closures native did not finish as the VM's did:\n{native}"
    );
}

#[test]
fn a_tail_call_to_a_name_an_isolate_unloaded_takes_the_lazy_path() {
    check(true);
}

#[test]
fn a_non_tail_call_to_a_name_an_isolate_unloaded_loads_it() {
    check(false);
}
