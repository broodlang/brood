//! **Every combination of boot artifacts must leave the same state as no artifacts at all.**
//!
//! The per-artifact differentials each compare one cache against source: `image_matches_source`
//! for a materialised stdlib module, `prelude_image_matches_source` for an imaged boot. Both
//! are sound, and neither can see the bugs this repo actually gets, because **none of those
//! bugs lived inside one artifact**:
//!
//! - **KI-105** — prelude image x stdlib image: an imaged boot restored a *snapshot* of a
//!   previous stdlib install, so section reads landed at stale offsets in a file that still
//!   existed and still parsed. Reported as `unbound symbol: io/puts` on a tree where nothing
//!   was wrong with `io`.
//! - **KI-106** — project image x prelude image: the registry-name set was not carried, so a
//!   multi-file `nest check` lost every derived multimethod mirror.
//! - **KI-72** — stdlib image x autoload stubs: a section replaced a stub before the module's
//!   own privates were bound, and a racing process died on a name that exists.
//!
//! A differential over one artifact is structurally blind to all three. This one runs the
//! product: three prelude states x two stdlib-image states, each compared against the arm
//! with nothing cached.
//!
//! **Each cell proves it is the cell it claims.** The recurring way a test in this area
//! passes for the wrong reason is by quietly falling back — an image that misses leaves the
//! arm on the source path wearing the image's name, and the differential then compares source
//! with source and agrees. `%boot-source` reports which of the three prelude paths actually
//! ran, so a cell that did not reach its own state fails as a *setup* failure with the cause
//! named, rather than passing vacuously. That is the same rule as asserting a summary line is
//! present rather than that failures are absent.
//!
//! **Deliberately out of scope: the project image.** Exercising it needs a scaffolded project
//! per cell, and it already has a dedicated guard in `project_image_registries.rs`. The seam
//! it shares with the prelude image — KI-106 — is covered here through the registry set,
//! which is the fact that was lost.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

/// How the prelude is made to arrive. The three real boot paths, forced.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Prelude {
    /// No artifacts consulted at all: read and evaluate the prelude every time.
    Source,
    /// ADR-138's expanded-text cache, with the image declined.
    TextCache,
    /// ADR-314's prelude image — the default.
    Image,
}

impl Prelude {
    /// What `%boot-source` must report for a cell that genuinely reached this state.
    fn expected_boot_source(self) -> &'static str {
        match self {
            Prelude::Source => ":source",
            Prelude::TextCache => ":boot-cache",
            Prelude::Image => ":prelude-image",
        }
    }
    fn tag(self) -> &'static str {
        match self {
            Prelude::Source => "source",
            Prelude::TextCache => "text",
            Prelude::Image => "image",
        }
    }
}

struct Cell {
    prelude: Prelude,
    /// Whether a current stdlib image exists in this cell's cache.
    stdlib_image: bool,
}

impl Cell {
    fn name(&self) -> String {
        format!(
            "prelude={} stdlib-image={}",
            self.prelude.tag(),
            self.stdlib_image
        )
    }
}

/// A private cache for one cell. Every artifact this test reasons about lives in
/// `XDG_CACHE_HOME`, so owning it is what makes a cell a cell rather than a reading of
/// whatever the developer's machine had.
fn cell_cache(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brood-matrix-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create cell cache");
    dir
}

fn base_command(cache: &Path, cell: &Cell) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.env("XDG_CACHE_HOME", cache)
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        // Pin the engine for the same reason the prelude differential does: the boot path is
        // under test, not the evaluator, and CI runs a tree-walker job.
        .env("BROOD_TIER", "1")
        .env_remove("BROOD_VM")
        .env_remove("BROOD_NO_JIT")
        .env_remove("BROOD_COVERAGE")
        // Clear BOTH spellings of every artifact switch before setting any, so an ambient
        // value from the developer's shell cannot decide which cell this is.
        .env_remove("BROOD_PRELUDE_IMAGE")
        .env_remove("BROOD_NO_PRELUDE_IMAGE")
        .env_remove("BROOD_NO_BOOT_CACHE")
        .env_remove("BROOD_NO_STDIMAGE");
    match cell.prelude {
        // No cache of either kind: every run re-reads and re-evaluates.
        Prelude::Source => {
            cmd.env("BROOD_NO_BOOT_CACHE", "1");
        }
        Prelude::TextCache => {
            cmd.env("BROOD_NO_PRELUDE_IMAGE", "1");
        }
        Prelude::Image => {}
    }
    if !cell.stdlib_image {
        cmd.env("BROOD_NO_STDIMAGE", "1");
    }
    support::dies_with_parent(&mut cmd);
    cmd
}

/// Build this cell's stdlib image, in its own cache. The runtime never builds one itself.
fn build_stdlib_image(cache: &Path, cell: &Cell, dir: &Path) {
    let prog = dir.join("build-image.blsp");
    std::fs::write(&prog, "(require-one 'stdimage) (stdimage/build)\n").expect("write builder");
    let mut cmd = base_command(cache, cell);
    // The builder must be able to WRITE the image whatever the cell's read policy is.
    cmd.env_remove("BROOD_NO_STDIMAGE");
    let out = cmd.arg(&prog).output().expect("run the stdlib image builder");
    assert!(
        out.status.success(),
        "building the stdlib image failed for {}:\n{}{}",
        cell.name(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// Run the fingerprint once. Returns (stdout, stderr).
fn run_once(cache: &Path, cell: &Cell, program: &Path) -> (String, String) {
    let out = base_command(cache, cell)
        .arg(program)
        .output()
        .expect("run brood");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Run a cell to its steady state: the first invocation writes whatever artifacts the cell
/// is entitled to, the second reads them. The second is the measurement — a cold boot is a
/// different cell from a warm one, which is the distinction this whole file exists to keep.
fn fingerprint(cache: &Path, cell: &Cell, program: &Path) -> String {
    let _warm = run_once(cache, cell, program);
    let (out, err) = run_once(cache, cell, program);

    // The cell must BE the cell. A missed artifact silently leaves the arm on the source
    // path, and a differential that compares source with source agrees with itself.
    let want = cell.prelude.expected_boot_source();
    assert!(
        err.contains(&format!("BOOT {want}")),
        "cell `{}` did not reach its own state: expected `BOOT {want}` on stderr.\n\
         Without this the cell would fall back and the comparison would pass vacuously.\n\
         stderr:\n{err}",
        cell.name(),
    );

    // Claim the fingerprint is PRESENT, never merely that two cells agree: two empty strings
    // agree, and an earlier differential in this repo passed a sabotage for exactly that.
    let header = out
        .lines()
        .find(|l| l.starts_with("GLOBALS "))
        .unwrap_or_else(|| {
            panic!(
                "cell `{}` printed no GLOBALS header — the fingerprint program did not run, \
                 so this cell proves nothing.\nstdout:\n{out}\nstderr:\n{err}",
                cell.name()
            )
        });
    let n: usize = header["GLOBALS ".len()..].trim().parse().unwrap_or(0);
    assert!(
        n > 500,
        "cell `{}` dumped only {n} globals; the prelude has ~1050, so it is truncated and a \
         diff over it proves nothing",
        cell.name()
    );
    assert!(
        out.lines().any(|l| l.starts_with("REGISTRIES ")),
        "cell `{}` printed no REGISTRIES line — KI-106 was a disagreement there with every \
         per-global attribute identical, so a fingerprint without it is blind to that class",
        cell.name()
    );

    // Skip the install's own bookkeeping. These record which artifacts THIS process loaded,
    // so they differ between cells by design — that is their entire content, and item 2 of
    // this cleanup deliberately made the fact MORE visible. One definition, in the runtime:
    // see `brood::INSTALL_BOOKKEEPING` for why they are skipped and where the facts they
    // carry are asserted positively instead.
    out.lines()
        .skip_while(|l| !l.starts_with("REGISTRIES "))
        .filter(|l| {
            let name = l.split_whitespace().next().unwrap_or(l);
            !brood::INSTALL_BOOKKEEPING.contains(&name)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_artifact_combination_boots_to_the_same_state() {
    let dir = std::env::temp_dir().join(format!("brood-matrix-work-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create work dir");
    let program = dir.join("fingerprint.blsp");
    let mut body = support::STATE_DUMP.to_string();
    // Announce the path taken on STDERR, so it cannot perturb the fingerprint on stdout.
    body.push_str("\n(io/puts (str \"BOOT \" (%boot-source)) :to *err*)\n");
    std::fs::write(&program, body).expect("write fingerprint program");

    let cells: Vec<Cell> = [Prelude::Source, Prelude::TextCache, Prelude::Image]
        .into_iter()
        .flat_map(|p| {
            [false, true].into_iter().map(move |s| Cell {
                prelude: p,
                stdlib_image: s,
            })
        })
        .collect();

    let mut baseline: Option<(String, String)> = None;
    for cell in &cells {
        let cache = cell_cache(&format!("{}-{}", cell.prelude.tag(), cell.stdlib_image));
        if cell.stdlib_image {
            build_stdlib_image(&cache, cell, &dir);
        }
        let got = fingerprint(&cache, cell, &program);
        match &baseline {
            // The first cell is (source, no stdlib image): nothing cached, nothing
            // materialised, so it is the state every other cell must reproduce.
            None => baseline = Some((cell.name(), got)),
            Some((base_name, base)) => {
                if *base != got {
                    let first_diff = base
                        .lines()
                        .zip(got.lines())
                        .find(|(a, b)| a != b)
                        .map(|(a, b)| format!("  baseline: {a}\n  cell    : {b}"))
                        .unwrap_or_else(|| {
                            format!(
                                "  (no differing line; lengths {} vs {})",
                                base.lines().count(),
                                got.lines().count()
                            )
                        });
                    panic!(
                        "artifact combination changed the boot state.\n\
                         baseline `{base_name}` vs cell `{}`:\n{first_diff}",
                        cell.name()
                    );
                }
            }
        }
        let _ = std::fs::remove_dir_all(&cache);
    }
    assert!(baseline.is_some(), "the matrix ran no cells");
    let _ = std::fs::remove_dir_all(&dir);
}
