//! End-to-end tests for the startup image (ADR-218) — that a second run of the same
//! project is served *from the image* and produces the same answers as the first.
//!
//! These exist because the image was written on every cold start and then never read
//! from, and nothing failed: the modules simply loaded from source again, so every
//! observable answer stayed right while the whole mechanism was dead. Two independent
//! defects, both invisible to a unit test of the `%image-*` primitives:
//!
//! 1. `project-install-image` ran `(def *image-sections* …)` inside module `project`,
//!    which binds `project/*image-sections*` — while `%require-force`, root code, kept
//!    reading the empty root global.
//! 2. `%require-force` consulted `*module-files*` before `*image-sections*`, and
//!    a project roots its OWN modules (ADR-070), so the package branch matched first for
//!    every module of every named project.
//!
//! The only thing that catches either is running a project twice and looking at what the
//! second run *loads*, which is what these do: a `println` at a module's top level is
//! evaluated by a source load and absent from an imaged one.

use std::path::Path;
use std::process::Command;

/// A project directory of its own, so cases can run concurrently under nextest.
fn scratch(tag: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("brood-startup-image-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, src: &str) {
    std::fs::write(dir.join(rel), src).unwrap();
}

/// Run `nest run` in `dir`, returning stdout+stderr together (the loader's "Building …"
/// line goes to stderr, the program's output to stdout, and cases assert on both).
fn nest_run(dir: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .arg("run")
        .current_dir(dir)
        .output()
        .expect("spawn nest");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "nest run failed: status={:?}\n{combined}",
        out.status
    );
    combined
}

/// The second run of an unchanged project must materialise its modules from the image
/// instead of evaluating their sources — the whole point of ADR-218.
#[test]
fn a_second_run_loads_modules_from_the_image_not_from_source() {
    let dir = scratch("lazy");
    write(
        &dir,
        "project.blsp",
        "(project\n  :name demo\n  :main app)\n",
    );
    // The marker is a TOP-LEVEL side effect: an image carries the module's bindings, so
    // materialising it cannot print this. A source load must.
    write(
        &dir,
        "src/lib.blsp",
        "(defmodule lib)\n\
         (io/puts \"SOURCE-LOAD: lib\")\n\
         (defn twice (x) (* 2 x))\n",
    );
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app (:use lib))\n\
         (defn main () (io/puts (str \"ANSWER: \" (twice 21))))\n",
    );

    let cold = nest_run(&dir);
    assert!(cold.contains("SOURCE-LOAD: lib"), "cold run:\n{cold}");
    assert!(cold.contains("ANSWER: 42"), "cold run:\n{cold}");

    let warm = nest_run(&dir);
    assert!(warm.contains("ANSWER: 42"), "warm run:\n{warm}");
    assert!(
        !warm.contains("SOURCE-LOAD: lib"),
        "the second run re-evaluated the module's source, so the image was not used:\n{warm}"
    );
    // …and it did not silently rebuild the image either (that would also skip the marker
    // on a later run while doing all the work again).
    assert!(
        !warm.contains("Building demo"),
        "the second run rebuilt from source:\n{warm}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An edit must invalidate the image — the staleness key is what makes serving an image
/// safe at all, and its failure mode is running the *previous* version of the code.
#[test]
fn an_edited_source_file_invalidates_the_image() {
    let dir = scratch("stale");
    write(
        &dir,
        "project.blsp",
        "(project\n  :name demo\n  :main app)\n",
    );
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app)\n(defn main () (io/puts \"ANSWER: 1\"))\n",
    );
    assert!(nest_run(&dir).contains("ANSWER: 1"));

    // A second file's edit must invalidate too, not just the entry point's.
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app)\n(defn main () (io/puts \"ANSWER: 2\"))\n",
    );
    let after = nest_run(&dir);
    assert!(
        after.contains("ANSWER: 2"),
        "an imaged start ran the pre-edit code:\n{after}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A dependency's modules are imaged like the project's own (ADR-070/218) — and because they
/// live outside `:source-paths` (`_deps/`, or anywhere at all for a `:path` dep), their files
/// have to be in the staleness key or an imaged start would serve the dep's old code forever.
/// Both halves are asserted here: served from the image, and invalidated by an edit.
///
/// The two are one test on purpose — "imaged" without "invalidated" is the dangerous state,
/// and it is only dangerous in combination.
#[test]
fn a_path_dependency_is_imaged_and_its_edits_invalidate() {
    let dep = scratch("dep-lib");
    let dir = scratch("dep-app");
    write(&dep, "project.blsp", "(project :name libdep)\n");
    write(
        &dep,
        "src/util.blsp",
        "(defmodule util)\n\
         (io/puts \"SOURCE-LOAD: libdep/util\")\n\
         (defn double (x) (* 2 x))\n",
    );
    // `:path` is resolved against the project root, so it has to be relative — both scratch
    // dirs are siblings under the temp dir.
    write(
        &dir,
        "project.blsp",
        &format!(
            "(project\n  :name depdemo\n  :main app\n  :dependencies [[libdep :path \"../{}\"]])\n",
            dep.file_name().unwrap().to_string_lossy()
        ),
    );
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app (:use libdep/util))\n\
         (defn main () (io/puts (str \"ANSWER: \" (double 21))))\n",
    );

    let cold = nest_run(&dir);
    assert!(cold.contains("SOURCE-LOAD: libdep/util"), "cold:\n{cold}");
    assert!(cold.contains("ANSWER: 42"), "cold:\n{cold}");

    let warm = nest_run(&dir);
    assert!(warm.contains("ANSWER: 42"), "warm:\n{warm}");
    assert!(
        !warm.contains("SOURCE-LOAD: libdep/util"),
        "the dependency was re-evaluated instead of materialised from the image:\n{warm}"
    );

    // Editing the DEP — outside this project's source paths entirely — must invalidate.
    write(
        &dep,
        "src/util.blsp",
        "(defmodule util)\n\
         (io/puts \"SOURCE-LOAD: libdep/util\")\n\
         (defn double (x) (* 3 x))\n",
    );
    let edited = nest_run(&dir);
    assert!(
        edited.contains("ANSWER: 63"),
        "an imaged start ran the dependency's pre-edit code:\n{edited}"
    );
    // …and the rebuilt image serves the NEW dep code, rather than rebuilding every run.
    let after = nest_run(&dir);
    assert!(after.contains("ANSWER: 63"), "after:\n{after}");
    assert!(
        !after.contains("SOURCE-LOAD: libdep/util"),
        "the dependency stopped being imaged after its edit:\n{after}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dep);
}

/// The way an app actually names a dependency's module: by its SHORT name, `(:use util)`,
/// which finds `util.blsp` by basename on the load path and binds `util/…` at root (a
/// uniquely-named dependency module never roots — `%bundle-module-package`). The case above
/// spells it rooted, `libdep/util`, and passed throughout while every app that wrote the
/// short form reloaded its dependencies from source on every run: the image writer asked
/// only whether `util` was in `*module-files*`, whose keys are all rooted, and dropped the
/// section. Met on hatch, whose five `store` modules were ~100 ms of a 220 ms one-file
/// `nest test` (2026-11-13).
#[test]
fn a_dependency_module_named_by_its_short_name_is_imaged() {
    let dep = scratch("short-dep-lib");
    let dir = scratch("short-dep-app");
    write(&dep, "project.blsp", "(project :name shortlib)\n");
    write(
        &dep,
        "src/util.blsp",
        "(defmodule util)\n\
         (io/puts \"SOURCE-LOAD: util\")\n\
         (defn double (x) (* 2 x))\n",
    );
    write(
        &dir,
        "project.blsp",
        &format!(
            "(project\n  :name shortapp\n  :main app\n  :dependencies [[shortlib :path \"../{}\"]])\n",
            dep.file_name().unwrap().to_string_lossy()
        ),
    );
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app (:use util))\n\
         (defn main () (io/puts (str \"ANSWER: \" (double 21))))\n",
    );

    let cold = nest_run(&dir);
    assert!(cold.contains("SOURCE-LOAD: util"), "cold:\n{cold}");
    assert!(cold.contains("ANSWER: 42"), "cold:\n{cold}");

    let warm = nest_run(&dir);
    assert!(warm.contains("ANSWER: 42"), "warm:\n{warm}");
    // The precondition the absence below rests on: this run was served by the image.
    assert!(
        !warm.contains("Building shortapp"),
        "the warm run rebuilt:\n{warm}"
    );
    assert!(
        !warm.contains("SOURCE-LOAD: util"),
        "the dependency module named by its short name was re-evaluated instead of \
         materialised from the image:\n{warm}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dep);
}

/// Loading MUTATES registries (`*impls*`, `*methods*`, `*method-from*`, `*module-docs*`, …)
/// rather than creating them, so the `(reflect/global-names)` diff an image is built from cannot
/// see them and each has to be carried deliberately. They were named by hand and the list
/// went stale repeatedly — silently, because a lost registry is a wrong answer with no
/// error. The set is derived from the registry funnel now (`%registry-names`), and this is
/// the case that fails if the derivation stops reaching one.
#[test]
fn an_imaged_start_keeps_what_loading_registered() {
    let dir = scratch("registries");
    write(
        &dir,
        "project.blsp",
        "(project\n  :name demo\n  :main app)\n",
    );
    write(
        &dir,
        "src/shapes.blsp",
        "(defmodule shapes \"Docstring for the module registry.\")\n\
         (io/puts \"SOURCE-LOAD: shapes\")\n\
         (defmulti combine)\n\
         (defmethod combine [:int :int] (a b) (+ a b))\n\
         (defability Sized (size (self) :-> int))\n\
         (defrecord box (w h))\n\
         (impl Sized shapes/box (size (s) (* (get s :w) (get s :h))))\n",
    );
    // Each line reads a different registry: dispatch through `*methods*`, ability dispatch
    // through `*impls*`/`*abilities*`, the record's nominal id through `*record-ids*`,
    // conflict provenance through `*method-from*`, and the docstring through `*module-docs*`.
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app (:use shapes))\n\
         (defn main ()\n\
        \x20 (io/puts (str \"multi: \" (combine 2 3)))\n\
        \x20 (io/puts (str \"ability: \" (size (box 3 4))))\n\
        \x20 (io/puts (str \"provenance: \" (count (seq/filter (vals *method-from*) (fn (ns) (= (->string ns) \"demo/shapes\"))))))\n\
        \x20 (io/puts (str \"docs: \" (contains? *module-docs* \"demo/shapes\"))))\n",
    );

    let cold = nest_run(&dir);
    let warm = nest_run(&dir);
    assert!(
        !warm.contains("SOURCE-LOAD"),
        "precondition: the warm run should be imaged:\n{warm}"
    );
    for expected in ["multi: 5", "ability: 12", "docs: true"] {
        assert!(
            cold.contains(expected),
            "cold run lacks {expected}:\n{cold}"
        );
        assert!(
            warm.contains(expected),
            "an imaged start lost a registry — {expected} did not survive:\n{warm}"
        );
    }

    // `*method-from*` is counted for `demo/shapes`'s own entries only, and asserted as cold ==
    // warm. The registry is GLOBAL: it also holds the stdlib's methods, and WHEN those arrive
    // depends on the boot path, not on the image keeping anything — a source load of `file`
    // loads `datetime` (three `compare-to` methods) when a macro expands a qualified head, an
    // imaged one defers it to first call (ADR-335), so the whole registry read 4 cold and 1
    // warm with nothing lost. What this checks is that an imaged start does not LOSE the
    // app's provenance — the two runs agreeing on the app's entries states exactly that.
    let provenance = |out: &str| -> usize {
        out.lines()
            .find_map(|l| l.strip_prefix("provenance: "))
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or_else(|| panic!("no `provenance: N` line:\n{out}"))
    };
    let (cold_n, warm_n) = (provenance(&cold), provenance(&warm));
    assert!(
        cold_n > 0,
        "cold run recorded no method provenance:\n{cold}"
    );
    assert_eq!(
        cold_n, warm_n,
        "an imaged start lost method provenance: {cold_n} cold vs {warm_n} imaged\n{warm}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Materialising a section DEFINES a module's bindings; it does not EVALUATE its source,
/// so the `(defmodule b (:use c))` header that would `require` its own dependencies never
/// runs. An imaged start therefore built a heap with holes and died on the first call
/// across a missing edge (KI-37) — `require` has to replay the edges the original load
/// recorded (`*require-edges*`).
///
/// **The chain is inside a DEPENDENCY on purpose, and that is what makes this test worth
/// having.** `nest run`'s advisory pre-flight `check-file`s each source file, and checking a
/// file incidentally `require`s its header's deps — so a chain in the project's *own* source
/// would be materialised by the checker whether or not the loader follows edges, and the
/// test would pass with the mechanism reverted. `project-feature-file` resolves a dep's
/// modules outside `:source-paths`, so they are never checked and nothing but the edge map
/// can pull `libdep/helper` in.
///
/// Verified by sabotage: with the `*require-edges*` replay removed, the warm run dies with
/// `unbound symbol: libdep/helper/triple`.
/// A declared sig reads back AS WRITTEN after an imaged start, as after a source load. The
/// sig is stored with its type names scoped to the module that means them (`shape` becomes
/// `demo/shape/shape`), and `reflect/declared-signature` makes them bare again through the
/// declaring module's `(:use …)` list. That list was recorded only by evaluating the header,
/// which an imaged start never does, so a warm run answered the scoped spelling: bedit's M-x
/// showed `bedit/model/model any → bedit/model/model` beside every command whose module had
/// not loaded from source, and its marginalia test failed whenever no other test had loaded
/// `commands` first.
#[test]
fn an_imaged_start_reads_a_declared_sig_back_as_written() {
    let dir = scratch("declared-sig");
    write(
        &dir,
        "project.blsp",
        "(project\n  :name demo\n  :main app)\n",
    );
    write(
        &dir,
        "src/shape.blsp",
        "(defmodule shape)\n(deftype shape (or int string))\n",
    );
    write(
        &dir,
        "src/lib.blsp",
        "(defmodule lib (:use shape))\n\
         (io/puts \"SOURCE-LOAD: lib\")\n\
         (sig grow (shape -> shape))\n\
         (defn grow (x) x)\n",
    );
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app (:use lib))\n\
         (defn main () (io/puts (str \"DECLARED: \" (reflect/declared-signature 'lib/grow))))\n",
    );

    let cold = nest_run(&dir);
    assert!(cold.contains("SOURCE-LOAD: lib"), "cold run:\n{cold}");
    assert!(
        cold.contains("DECLARED: (shape -> shape)"),
        "a source load must read the sig back as written:\n{cold}"
    );

    let warm = nest_run(&dir);
    assert!(
        !warm.contains("SOURCE-LOAD: lib"),
        "the second run loaded lib from source, so it says nothing about the image:\n{warm}"
    );
    assert!(
        warm.contains("DECLARED: (shape -> shape)"),
        "an imaged start read the sig back scoped, not as written:\n{warm}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_imaged_start_follows_transitive_require_edges() {
    let dep = scratch("edge-lib");
    let dir = scratch("edge-app");
    write(&dep, "project.blsp", "(project :name libdep)\n");
    // Two levels *inside* the dependency: app reaches `util` directly, and only `util`'s own
    // header reaches `helper`. Nothing the entry point names mentions `helper` at all.
    write(
        &dep,
        "src/helper.blsp",
        "(defmodule helper)\n\
         (io/puts \"SOURCE-LOAD: libdep/helper\")\n\
         (defn triple (x) (* 3 x))\n",
    );
    write(
        &dep,
        "src/util.blsp",
        "(defmodule util (:use libdep/helper))\n\
         (io/puts \"SOURCE-LOAD: libdep/util\")\n\
         (defn double-tripled (x) (* 2 (triple x)))\n",
    );
    write(
        &dir,
        "project.blsp",
        &format!(
            "(project\n  :name edgedemo\n  :main app\n  :dependencies [[libdep :path \"../{}\"]])\n",
            dep.file_name().unwrap().to_string_lossy()
        ),
    );
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app (:use libdep/util))\n\
         (defn main () (io/puts (str \"ANSWER: \" (double-tripled 7))))\n",
    );

    let cold = nest_run(&dir);
    assert!(cold.contains("ANSWER: 42"), "cold:\n{cold}");
    assert!(cold.contains("SOURCE-LOAD: libdep/helper"), "cold:\n{cold}");

    // The load that matters: `app` is required, which materialises `libdep/util`, which must
    // in turn pull `libdep/helper` — a module no evaluated form in this run ever names.
    let warm = nest_run(&dir);
    assert!(
        warm.contains("ANSWER: 42"),
        "an imaged start did not follow the transitive require edge:\n{warm}"
    );
    assert!(
        !warm.contains("SOURCE-LOAD"),
        "precondition: the warm run should be imaged, not a source load:\n{warm}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dep);
}

/// The same hole as `an_imaged_start_follows_transitive_require_edges`, one section over.
/// A file with **no `(defmodule …)` header** establishes no namespace, so its defs are
/// root-level globals that ride the always-materialised `""` section — restored without
/// anything ever `require`ing that file, and so without its own top-level `(require …)`
/// ever running. Its edges are recorded under `""` and replayed by `project-install-image`.
#[test]
fn an_imaged_start_follows_a_headerless_files_require_edges() {
    let dir = scratch("edge-root");
    write(
        &dir,
        "project.blsp",
        "(project\n  :name rootdemo\n  :main app)\n",
    );
    write(
        &dir,
        "src/geom.blsp",
        "(defmodule geom)\n\
         (io/puts \"SOURCE-LOAD: geom\")\n\
         (defn square (x) (* x x))\n",
    );
    // No `defmodule`: `helpers` defines a ROOT global and requires `geom` at top level.
    write(
        &dir,
        "src/helpers.blsp",
        "(require-one 'rootdemo/geom)\n\
         (defn helper-area (x) (geom/square x))\n",
    );
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app)\n\
         (defn main () (io/puts (str \"ANSWER: \" (helper-area 7))))\n",
    );

    let cold = nest_run(&dir);
    assert!(cold.contains("ANSWER: 49"), "cold:\n{cold}");

    let warm = nest_run(&dir);
    assert!(
        warm.contains("ANSWER: 49"),
        "an imaged start restored a headerless file's defs without loading what they call:\n{warm}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A require CYCLE must still terminate now that materialising a section replays the
/// module's require edges — a mutually-recursive pair is the shape that spins if the
/// replay ever stops going through `require-one`'s load-once bookkeeping.
///
/// **Stated honestly: this is a behaviour test, not a gate on the `provide`/replay
/// ordering.** Reordering those two lines was tried and this test still passed, because
/// `require-one`'s `*features-loading*` marker returns immediately for this process's own
/// in-flight load and is what actually breaks the cycle. It earns its place by covering an
/// imaged cycle at all, which nothing else here does — but do not read a pass as evidence
/// about the ordering.
#[test]
fn an_imaged_start_terminates_on_a_require_cycle() {
    let dir = scratch("edge-cycle");
    write(
        &dir,
        "project.blsp",
        "(project\n  :name cyc\n  :main app)\n",
    );
    // `:use-internals … :only` is the cycle-safe import form — a refer-all into a
    // still-loading module is an error, which is a different failure from the one under test.
    write(
        &dir,
        "src/a.blsp",
        "(defmodule a (:use-internals cyc/b :only [bee]))\n\
         (defn ay () (str \"a\" (bee)))\n",
    );
    write(
        &dir,
        "src/b.blsp",
        "(defmodule b)\n\
         (require-one 'cyc/a)\n\
         (defn bee () \"b\")\n",
    );
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app (:use cyc/a))\n\
         (defn main () (io/puts (str \"ANSWER: \" (ay))))\n",
    );

    let cold = nest_run(&dir);
    assert!(cold.contains("ANSWER: ab"), "cold:\n{cold}");
    let warm = nest_run(&dir);
    assert!(
        warm.contains("ANSWER: ab"),
        "an imaged start mishandled a require cycle:\n{warm}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// KI-275: a file a module splices in with `include-str` is part of what the image holds,
/// so editing it must make the image stale. The staleness key covered the project's
/// SOURCES only, and an imaged run went on serving the old text: bedit's eval agent
/// (`(include-str "elixir/bedit_agent.exs")`) kept running a fix's previous version until
/// some `.blsp` happened to change.
#[test]
fn an_edited_include_str_file_makes_the_image_stale() {
    let dir = scratch("include");
    std::fs::create_dir_all(dir.join("priv")).unwrap();
    write(
        &dir,
        "project.blsp",
        "(project\n  :name demo\n  :main app)\n",
    );
    write(&dir, "priv/greeting.txt", "hello");
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app)\n\
         (def *greeting* (include-str \"priv/greeting.txt\"))\n\
         (defn main () (io/puts (str \"GREETING: \" *greeting*)))\n",
    );
    assert!(nest_run(&dir).contains("GREETING: hello"));
    assert!(nest_run(&dir).contains("GREETING: hello"), "the imaged run");
    write(&dir, "priv/greeting.txt", "goodbye, and a longer line");
    let third = nest_run(&dir);
    assert!(
        third.contains("GREETING: goodbye, and a longer line"),
        "an imaged run served the old included text:\n{third}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// KI-276: a project image written by a run with contracts ARMED carries the contract shims
/// as the bindings, and the staleness key did not say which mode wrote it — so the next
/// UNARMED run materialised the shims (and an armed run after an unarmed write lost its
/// contracts). bedit's suite read wrong answers from it: `nest test` (armed by default)
/// wrote the image, and the downstream smoke's unarmed `nest test` reused it.
#[test]
fn the_image_is_keyed_by_contract_mode() {
    let dir = scratch("contracts");
    write(
        &dir,
        "project.blsp",
        "(project\n  :name demo\n  :main app)\n",
    );
    // `f` in a module of its own: a contract guards the module boundary (ADR-383), so a
    // call from inside `lib` would not meet the shim in either mode.
    write(
        &dir,
        "src/lib.blsp",
        "(defmodule lib)\n\
         (sig f (int -> int))\n\
         (check-allow :type-mismatch (defn f (x) (str \"v\" x)))\n",
    );
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app (:use lib))\n\
         (defn main () (io/puts (str \"RESULT: \" (try (f 1) (catch e (get e :kind))))))\n",
    );
    let run = |armed: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_nest"))
            .arg("run")
            .current_dir(&dir)
            .env("BROOD_CONTRACTS", armed)
            .output()
            .expect("spawn nest");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert!(run("1").contains("RESULT: :contract"), "armed, cold");
    let unarmed = run("0");
    assert!(
        unarmed.contains("RESULT: v1"),
        "an unarmed run was served the armed image's contract shims:\n{unarmed}"
    );
    let armed = run("1");
    assert!(
        armed.contains("RESULT: :contract"),
        "an armed run was served the unarmed image and lost its contract:\n{armed}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `nest test FILE` installs the image LAZILY, as `nest run` does: the named file's header and
/// its first use of a name materialise what it reaches, and nothing else. It used to take the
/// tooling path, which materialises every section — on hatch 90 ms of a 220 ms one-file run,
/// most of a TDD cycle spent on modules the file never touches (2026-11-13).
///
/// `lib` reaches `helper` only through a qualified call inside a function body, so `helper`
/// is materialised on FIRST USE, not by a header — the half of lazy loading a header-only
/// project would never exercise. `other` is reached by nothing.
#[test]
fn a_named_test_file_materialises_only_what_it_reaches() {
    let dir = scratch("named-lazy");
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    write(&dir, "project.blsp", "(project\n  :name lazydemo)\n");
    write(
        &dir,
        "src/helper.blsp",
        "(defmodule helper)\n(defn inc2 (x) (+ x 2))\n",
    );
    write(
        &dir,
        "src/lib.blsp",
        "(defmodule lib)\n(defn f (x) (helper/inc2 x))\n",
    );
    write(
        &dir,
        "src/other.blsp",
        "(defmodule other)\n(defn g () :unreached)\n",
    );
    write(
        &dir,
        "tests/lib_test.blsp",
        "(defmodule lib-test (:use test) (:use lib))\n\
         (describe \"lib\"\n\
           (test \"reaches helper on first use\" (assert= 3 (f 1))))\n",
    );
    let run = || {
        let out = Command::new(env!("CARGO_BIN_EXE_nest"))
            .args(["test", "tests/lib_test.blsp"])
            .current_dir(&dir)
            .env("BROOD_IMAGE_TRACE", "1")
            .output()
            .expect("spawn nest");
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };
    let cold = run();
    assert!(cold.contains("1 tests, 1 passed"), "cold:\n{cold}");
    let warm = run();
    assert!(warm.contains("1 tests, 1 passed"), "warm:\n{warm}");
    // The precondition the absence below rests on: the warm run materialised from the image.
    assert!(
        warm.contains("[image] lazydemo/lib\n") && warm.contains("[image] lazydemo/helper\n"),
        "the warm run did not materialise what the test reaches from the image:\n{warm}"
    );
    assert!(
        !warm.contains("[image] lazydemo/other\n"),
        "a named test file materialised a module it never reaches:\n{warm}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
