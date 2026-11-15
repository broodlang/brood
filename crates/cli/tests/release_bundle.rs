//! End-to-end release-bundle test (ADR-038): append an archive to the prebuilt
//! `brood` binary, then run the result as its own process and assert it boots
//! the *embedded* `:main` — from a cwd with no project and no sources on disk.
//!
//! This exercises the `brood`-side half of `nest release`: footer detection on
//! `current_exe`, mounting the archive, resolving an app module from the bundle
//! through `require` (via the extended `%builtin-module`), and dispatching to
//! `project-release/run-bundle`. The `nest`-side collection is covered by the unit tests
//! in `crates/lisp/src/bundle.rs` plus this manual archive construction.

use std::process::Command;

mod support;

/// Build `[brood][archive][footer]` for a two-module app and return its path,
/// alongside a separate empty directory to run it from.
fn write_app(
    tag: &str,
    manifest: &str,
    modules: &[(&str, &str)],
) -> (std::path::PathBuf, std::path::PathBuf) {
    let brood = env!("CARGO_BIN_EXE_brood");
    let base = std::fs::read(brood).expect("read brood binary");
    let owned: Vec<(String, String)> = modules
        .iter()
        .map(|(n, s)| (n.to_string(), s.to_string()))
        .collect();
    let archive = brood::bundle::serialize(manifest, &owned, None);

    let dir = std::env::temp_dir().join(format!("brood-release-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let app = dir.join("app");
    brood::bundle::write_release(&base, &archive, &app).expect("write release binary");

    // Run from an empty subdir: proves nothing is read from the project tree.
    let run_cwd = dir.join("clean");
    std::fs::create_dir_all(&run_cwd).unwrap();
    (app, run_cwd)
}

/// Run a freshly written bundle, retrying `ETXTBSY`.
///
/// The four tests here run as parallel threads of ONE process, and each writes its own
/// `app` binary and then execs it. `Command` forks, and a fork taken while another
/// thread still holds its `app` open for writing leaves the child holding that write fd
/// until it execs — and Linux refuses to exec a file any process has open for writing
/// (`ETXTBSY`). The window is microseconds, which is why this failed roughly one run in
/// five rather than every time. Nothing here is wrong with the bundle: the same binary
/// runs on the next attempt, so retry it rather than serialising the whole file.
fn run_bundle(cmd: &mut Command) -> std::process::Output {
    for attempt in 0..50 {
        match cmd.output() {
            Ok(out) => return out,
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(10 * (attempt + 1)));
            }
            Err(e) => panic!("run bundled app: {e:?}"),
        }
    }
    panic!("run bundled app: still ETXTBSY after 50 attempts");
}

#[test]
fn bundled_brood_boots_embedded_main_with_cross_module_use() {
    let (app, cwd) = write_app(
        "main",
        "(project :name \"t\" :version \"0\")",
        &[
            // `main` uses `lib` — proves cross-module `require`/`:use` resolves
            // out of the embedded archive, not the disk load-path.
            (
                "main",
                "(defmodule main (:use lib))\n(defn main () (io/puts (greet)))",
            ),
            ("lib", "(defmodule lib)\n(defn greet () \"embedded-ok\")"),
        ],
    );
    let mut cmd = Command::new(&app);
    cmd.current_dir(&cwd);
    support::dies_with_parent(&mut cmd);
    let out = run_bundle(&mut cmd);
    assert!(
        out.status.success(),
        "exit: {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "embedded-ok");
    let _ = std::fs::remove_dir_all(app.parent().unwrap());
}

#[test]
fn bundled_deps_with_same_module_name_coexist_rooted() {
    // Dev/release parity for package-rooting (ADR-070): two dependencies each provide a
    // `parser` module (and a `util` module). At dev time they are distinct globals
    // `alpha/parser` / `beta/parser`; the bundle must keep them apart too. Each dep module
    // is embedded under its ROOTED key, and `main` calls both — proving neither clobbered
    // the other and that each `parser`'s intra-package `(:use util)` rooted to its OWN
    // `util` (alpha→"A", beta→"B"), not the other dep's.
    let manifest = "(project :name \"t\" :version \"0\" :dependencies [[alpha :path \"a\"] [beta :path \"b\"]])";
    let modules: &[(&str, &str)] = &[
        (
            "main",
            "(defmodule main)\n(defn main () (io/puts (str (alpha/parser/tag) \"|\" (beta/parser/tag))))",
        ),
        // `parser` in each dep uses its OWN `util` via a bare intra-package `(:use util)`,
        // which must root to `alpha/util` / `beta/util` under each dep's package context.
        (
            "alpha/parser",
            "(defmodule parser (:use util))\n(defn tag () (str \"alpha:\" (util-tag)))",
        ),
        ("alpha/util", "(defmodule util)\n(defn util-tag () \"A\")"),
        (
            "beta/parser",
            "(defmodule parser (:use util))\n(defn tag () (str \"beta:\" (util-tag)))",
        ),
        ("beta/util", "(defmodule util)\n(defn util-tag () \"B\")"),
    ];
    let (app, cwd) = write_app("same-name-deps", manifest, modules);
    let mut cmd = Command::new(&app);
    cmd.current_dir(&cwd);
    support::dies_with_parent(&mut cmd);
    let out = run_bundle(&mut cmd);
    assert!(
        out.status.success(),
        "exit: {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "alpha:A|beta:B"
    );
    // The same bundle with its startup image (ADR-484): the rooted dependency modules are
    // sectioned under their rooted names and stay apart when materialised.
    embed_image(&app, &cwd, manifest, manifest, modules);
    assert_eq!(run_ok(&app, &cwd, &[], false).trim(), "alpha:A|beta:B");
    let check = run_ok(&app, &cwd, &[brood::bundle::BUNDLE_BOOT_CHECK_ARG], false);
    assert!(check.contains("from the startup image"), "{check}");
    let _ = std::fs::remove_dir_all(app.parent().unwrap());
}

#[test]
fn bundled_app_receives_argv() {
    let (app, cwd) = write_app(
        "argv",
        "(project :name \"t\" :version \"0\")",
        &[(
            "main",
            "(defmodule main)\n(defn main (& args) (io/puts (str \"argv:\" args)))",
        )],
    );
    let mut cmd = Command::new(&app);
    cmd.args(["alpha", "beta"]).current_dir(&cwd);
    support::dies_with_parent(&mut cmd);
    let out = run_bundle(&mut cmd);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "argv:(alpha beta)"
    );
    let _ = std::fs::remove_dir_all(app.parent().unwrap());
}

#[test]
fn bundled_bare_reference_to_unique_dep_module_resolves() {
    // A dependency's uniquely-named module referenced by its BARE `(defmodule)` name — the idiom
    // every real app uses (hive's `(:alias repo)`), and the way a dev run binds it (found by
    // basename on the load-path, never rooted). The bundle embeds it under its rooted key
    // `store/repo`; `:bundled-packages` records that `store` ships it (bundle-collect bakes that
    // set, since a transitive dependency is absent from the manifest's `:dependencies`). With a
    // unique name it must bind BARE so the bare reference resolves — this crash-looped with
    // `require: cannot find module 'repo'` before rooting became collision-only (ADR-070).
    let (app, cwd) = write_app(
        "bare-dep-ref",
        "(project :name \"t\" :version \"0\" :bundled-packages [\"store\"])",
        &[
            (
                "main",
                "(defmodule main (:alias repo))\n(defn main () (io/puts (repo/tag)))",
            ),
            (
                "store/repo",
                "(defmodule repo)\n(defn tag () \"bare-dep-ok\")",
            ),
        ],
    );
    let mut cmd = Command::new(&app);
    cmd.current_dir(&cwd);
    support::dies_with_parent(&mut cmd);
    let out = run_bundle(&mut cmd);
    assert!(
        out.status.success(),
        "exit: {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "bare-dep-ok");
    let _ = std::fs::remove_dir_all(app.parent().unwrap());
}

/// `nest release -o <path>` over a binary that is RUNNING: Linux refuses to open a busy
/// executable for writing (`ETXTBSY`, "Text file busy"), which is exactly what
/// `make install` while the app is open used to hit. Writing beside the target and
/// renaming over it succeeds — the old inode keeps running — and the next start is the
/// new release, while the running one is untouched.
#[test]
fn a_release_replaces_a_running_binary() {
    let (app, cwd) = write_app(
        "busy",
        "(project :name busy :main app)",
        &[(
            "app",
            "(defmodule app \"\")\n(defn main () (do (proc/sleep 4000) (io/puts \"v1\")))",
        )],
    );
    // the parallel-tests fork race `run_bundle` documents applies to the spawn too
    let child = (0..50)
        .find_map(|attempt| {
            match Command::new(&app)
                .current_dir(&cwd)
                .stdout(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => Some(c),
                Err(e) if e.raw_os_error() == Some(26) => {
                    std::thread::sleep(std::time::Duration::from_millis(10 * (attempt + 1)));
                    None
                }
                Err(e) => panic!("start the first release: {e:?}"),
            }
        })
        .expect("start the first release");
    // let it get past exec — from here the file is a busy executable
    std::thread::sleep(std::time::Duration::from_millis(300));

    let base = std::fs::read(env!("CARGO_BIN_EXE_brood")).expect("read brood binary");
    let archive = brood::bundle::serialize(
        "(project :name busy :main app)",
        &[(
            "app".to_string(),
            "(defmodule app \"\")\n(defn main () (io/puts \"v2\"))".to_string(),
        )],
        None,
    );
    brood::bundle::write_release(&base, &archive, &app)
        .expect("a release must replace a running binary");

    let second = run_bundle(Command::new(&app).current_dir(&cwd));
    assert_eq!(String::from_utf8_lossy(&second.stdout).trim(), "v2");
    let first = child
        .wait_with_output()
        .expect("the running release finishes");
    assert_eq!(
        String::from_utf8_lossy(&first.stdout).trim(),
        "v1",
        "the running one is untouched"
    );
    // no temporary left beside the target
    let leftovers: Vec<_> = std::fs::read_dir(app.parent().unwrap())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains("nest-release-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_bundle_with_a_corrupt_archive_is_refused_not_run_as_plain_brood() {
    // A valid footer over an archive that does not parse. Falling through to plain
    // `brood` would start the REPL (or run argv as a file) in place of the app.
    let brood = env!("CARGO_BIN_EXE_brood");
    let base = std::fs::read(brood).expect("read brood binary");
    let mut archive = brood::bundle::serialize("(project :name \"t\")", &[], None);
    archive[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
    let dir = std::env::temp_dir().join(format!("brood-release-corrupt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let app = dir.join("app");
    brood::bundle::write_release(&base, &archive, &app).expect("write release binary");
    let out = run_bundle(Command::new(&app).arg("--version"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a corrupt bundle ran: {stderr}");
    assert!(
        stderr.contains("corrupt release bundle"),
        "stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ADR-436: a release's boot check resolves an application `:main` and never starts it —
/// it runs on the machine that built the release, where a start would bind the app's
/// ports. Run without the flag, the SAME binary starts the application, so the missing
/// `STARTED` below is the boot check's doing, not an application that cannot start.
#[test]
fn bundled_boot_check_resolves_an_application_without_starting_it() {
    let (app, cwd) = write_app(
        "application",
        "(project :name \"t\" :version \"0\")",
        &[(
            "main",
            "(defmodule main)\n\
             (defn- shut-down-when-running ()\n\
               (if (proc/whereis :application)\n\
                 (application/shutdown)\n\
                 (do (proc/sleep 10) (shut-down-when-running))))\n\
             (application/defapp main\n\
               {:start (fn () (io/puts \"STARTED\") (spawn (shut-down-when-running)) :root)})",
        )],
    );

    let mut cmd = Command::new(&app);
    cmd.arg(brood::bundle::BUNDLE_BOOT_CHECK_ARG)
        .current_dir(&cwd);
    support::dies_with_parent(&mut cmd);
    let out = run_bundle(&mut cmd);
    let stdout = String::from_utf8_lossy_owned(out.stdout);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("an application, not started"),
        "the boot check must say what it resolved: {stdout}"
    );
    assert!(
        !stdout.contains("STARTED"),
        "the boot check started the application: {stdout}"
    );

    let mut cmd = Command::new(&app);
    cmd.current_dir(&cwd);
    support::dies_with_parent(&mut cmd);
    let out = run_bundle(&mut cmd);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "STARTED");
    let _ = std::fs::remove_dir_all(app.parent().unwrap());
}

/// What `nest release` does after writing a bundle (ADR-484): run it with
/// `--brood-build-image`, then rewrite it with the image it wrote after its modules. The
/// image is built for `image_manifest`'s archive, which is `manifest`'s unless a test is
/// building a mismatched one on purpose. Returns what the build run printed.
fn embed_image(
    app: &std::path::Path,
    cwd: &std::path::Path,
    manifest: &str,
    image_manifest: &str,
    modules: &[(&str, &str)],
) -> String {
    let owned: Vec<(String, String)> = modules
        .iter()
        .map(|(n, s)| (n.to_string(), s.to_string()))
        .collect();
    let base = std::fs::read(env!("CARGO_BIN_EXE_brood")).expect("read brood binary");
    let image_path = app.with_file_name("app.image");
    if image_manifest != manifest {
        let archive = brood::bundle::serialize(image_manifest, &owned, None);
        brood::bundle::write_release(&base, &archive, app).expect("write release binary");
    }
    let mut cmd = Command::new(app);
    cmd.arg(brood::bundle::BUNDLE_BUILD_IMAGE_ARG)
        .arg(&image_path)
        .current_dir(cwd);
    support::dies_with_parent(&mut cmd);
    let out = run_bundle(&mut cmd);
    assert!(
        out.status.success(),
        "the image build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let image = std::fs::read(&image_path).expect("the build wrote the image");
    let archive = brood::bundle::serialize(manifest, &owned, Some(&image));
    brood::bundle::write_release(&base, &archive, app).expect("write imaged release binary");
    String::from_utf8_lossy_owned(out.stdout)
}

/// Run `app` (with `args`, and `BROOD_NO_BUNDLE_IMAGE` when `without_image`), asserting it
/// exits 0, and answer its stdout.
fn run_ok(
    app: &std::path::Path,
    cwd: &std::path::Path,
    args: &[&str],
    without_image: bool,
) -> String {
    let mut cmd = Command::new(app);
    cmd.args(args).current_dir(cwd);
    if without_image {
        cmd.env("BROOD_NO_BUNDLE_IMAGE", "1");
    }
    support::dies_with_parent(&mut cmd);
    let out = run_bundle(&mut cmd);
    assert!(
        out.status.success(),
        "exit: {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy_owned(out.stdout)
}

/// A bundle's modules, each announcing at load time that its SOURCE was evaluated — a
/// module materialised from the image evaluates nothing, so its line is missing. `lib`
/// carries registrations (a record, an ability and its impl) that `main` dispatches through;
/// `store/repo` is a dependency module reached by its bare name; `other.blsp` declares a
/// module of a different name, so the image has no section under the name it is required
/// by and it must load from source.
const IMAGED_APP: &[(&str, &str)] = &[
    (
        "main",
        "(defmodule main (:use lib) (:alias repo))\n\
         (defn main () (io/puts (str \"area:\" (area (square 3)) \" repo:\" (repo/tag) \" other:\" (different/tag))))",
    ),
    (
        "lib",
        "(defmodule lib)\n\
         (io/puts \"evaluated: lib\")\n\
         (defrecord square (side))\n\
         (defability Shape (area (self)))\n\
         (impl Shape lib/square (area (s) (* (get s :side) (get s :side))))",
    ),
    (
        "store/repo",
        "(defmodule repo)\n(io/puts \"evaluated: repo\")\n(defn tag () \"dep\")",
    ),
    (
        "other",
        "(defmodule different)\n(io/puts \"evaluated: other\")\n(defn tag () \"fallback\")",
    ),
];

const IMAGED_MANIFEST: &str =
    "(project :name \"t\" :version \"0\" :bundled-packages [\"store\"] :built-at 1)";

/// ADR-484: a bundle carrying its startup image materialises its modules — the project's
/// own and a dependency's — instead of evaluating their source; their registrations arrive
/// with them (the ability dispatch on the record works); a module the image has no section
/// for falls back to its source; and the boot check exercises the same image path.
#[test]
fn an_imaged_bundle_materialises_its_modules_instead_of_evaluating_them() {
    let (app, cwd) = write_app("imaged", IMAGED_MANIFEST, IMAGED_APP);
    let built = embed_image(&app, &cwd, IMAGED_MANIFEST, IMAGED_MANIFEST, IMAGED_APP);
    assert!(
        built.contains("evaluated: lib"),
        "the build loads from source: {built}"
    );

    let out = run_ok(&app, &cwd, &[], false);
    assert!(
        out.contains("area:9 repo:dep other:fallback"),
        "the imaged app runs: {out}"
    );
    assert!(
        !out.contains("evaluated: lib"),
        "lib came from source: {out}"
    );
    assert!(
        !out.contains("evaluated: repo"),
        "the dep came from source: {out}"
    );
    assert!(
        out.contains("evaluated: other"),
        "a module with no section must load from its source: {out}"
    );

    let check = run_ok(&app, &cwd, &[brood::bundle::BUNDLE_BOOT_CHECK_ARG], false);
    assert!(
        check.contains("from the startup image"),
        "the boot check takes the image path: {check}"
    );
    assert!(!check.contains("evaluated: lib"), "{check}");

    // The same binary with the image turned off is the source boot, line for line.
    let source = run_ok(&app, &cwd, &[], true);
    assert!(
        source.contains("evaluated: lib") && source.contains("evaluated: repo"),
        "{source}"
    );
    assert!(
        source.contains("area:9 repo:dep other:fallback"),
        "{source}"
    );
    let _ = std::fs::remove_dir_all(app.parent().unwrap());
}

/// An image stamped for another release (here: another manifest) is never installed — the
/// bundle loads every module from source and runs as before, rather than materialising
/// bindings built from different code.
#[test]
fn a_bundle_image_built_for_another_release_is_ignored() {
    let (app, cwd) = write_app("imaged-stale", IMAGED_MANIFEST, IMAGED_APP);
    let other = "(project :name \"t\" :version \"0\" :bundled-packages [\"store\"] :built-at 2)";
    embed_image(&app, &cwd, IMAGED_MANIFEST, other, IMAGED_APP);
    let out = run_ok(&app, &cwd, &[], false);
    assert!(out.contains("evaluated: lib"), "{out}");
    assert!(out.contains("area:9 repo:dep other:fallback"), "{out}");
    let check = run_ok(&app, &cwd, &[brood::bundle::BUNDLE_BOOT_CHECK_ARG], false);
    assert!(check.contains("from source"), "{check}");
    let _ = std::fs::remove_dir_all(app.parent().unwrap());
}

/// ADR-606: a bundle knows its root project's own modules, as `setup` does from disk — the
/// project's modules load UNROOTED in a bundle and no package context names them, so a host
/// checking a plugin's module names against `reflect/project-modules` (bedit refusing a
/// package module named like one of its own) used to read `[]` in the released program. The
/// list `bundle-collect` bakes into the manifest is what the bundle answers; a dependency's
/// modules are not the project's.
#[test]
fn a_bundle_answers_its_own_project_modules() {
    let modules: &[(&str, &str)] = &[
        (
            "main",
            "(defmodule main (:use lib) (:alias repo))\n\
             (defn main () (io/puts (pr-str [(reflect/project-modules) (repo/tag)])))",
        ),
        ("lib", "(defmodule lib)\n(defn greet () 1)"),
        ("store/repo", "(defmodule repo)\n(defn tag () \"dep\")"),
    ];
    // the manifest `bundle-collect` writes: the project's modules by their `(defmodule …)` names
    let baked = "(project :name \"t\" :version \"0\" :bundled-packages [\"store\"] \
                 :project-modules [\"main\" \"lib\"])";
    let (app, cwd) = write_app("project-modules", baked, modules);
    assert_eq!(
        run_ok(&app, &cwd, &[], false).trim(),
        "[[\"lib\" \"main\"] \"dep\"]"
    );
    let _ = std::fs::remove_dir_all(app.parent().unwrap());
    // a bundle built before the field: its embedded keys that are no dependency's
    let (app, cwd) = write_app(
        "project-modules-old",
        "(project :name \"t\" :version \"0\" :bundled-packages [\"store\"])",
        modules,
    );
    assert_eq!(
        run_ok(&app, &cwd, &[], false).trim(),
        "[[\"lib\" \"main\"] \"dep\"]"
    );
    let _ = std::fs::remove_dir_all(app.parent().unwrap());
}
