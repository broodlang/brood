//! A module's load must not depend on when the JIT tiers something up.
//!
//! Tiering pre-loads the modules a hot arm names (`preload_arm_globals`, ADR-335 item 4), so
//! a native read never meets an unbound global. Run from INSIDE a module load, that pre-load
//! started a second, unrelated load at whatever call crossed the tiering threshold: its
//! defines were journalled as the enclosing module's, and what the module "introduced"
//! depended on JIT timing. The stdlib image builder attributes root globals by exactly that
//! measure, and under contracts' extra calls the prelude's `get` tiered up inside
//! `project`'s load and pulled `string` into it (`stdimage_test` "a root global is
//! attributed to the module that defines it", red on main 2026-10-21).
//!
//! Asserted on what a program reaches: `require-one` of a module whose own load makes a
//! function hot, where that function's cold branch names a module nobody has loaded. The
//! cold module must still be unloaded afterwards — and calling the branch must load it.
//!
//! The cold branch names `zlib/gzip` as an OPERAND, not a call head. With no stdlib image
//! (`BROOD_NO_STDIMAGE=1`, which CI's tree-walker job sets and this child inherits) a
//! qualified call head loads its module at EXPANSION, by design (`derive.rs`, ADR-335: only
//! a head the image vouches is a function may wait for its call) — so a head would load
//! `zlib` before any tier-up and the question would depend on the boot path. An operand is
//! lazy on both paths, and the tier-up pre-load still sees it (`Inst::Global`).
//!
//! Each tier runs on BOTH boot paths in every job, so a boot-path dependence fails wherever
//! the suite runs rather than only in the one job that boots from source. The imaged run
//! boots from source too when no image was built for this binary; it cannot fail falsely.

use std::process::Command;

const HOTMOD: &str = r#"(defmodule hotmod)
(defn pick (x) (if (= x -1) (let (gzip zlib/gzip) (gzip "a")) x))
(defn spin (n acc) (if (= n 0) acc (spin (dec n) (+ acc (pick n)))))
(def total (spin 50000 0))
"#;

const PROGRAM: &str = r#"(require-one 'hotmod)
(io/puts (str "after-require zlib=" (contains? *features* "zlib")))
(io/puts (str "total=" hotmod/total))
(io/puts (str "cold-branch-loads=" (and (bytes? (hotmod/pick -1)) (contains? *features* "zlib"))))
"#;

fn run(tier: &str, imaged: bool) -> (String, String) {
    let dir = std::env::temp_dir().join(format!(
        "brood-tier-up-loads-{}-{tier}-{imaged}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("program.blsp"), PROGRAM).expect("write program");
    std::fs::write(dir.join("hotmod.blsp"), HOTMOD).expect("write module");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.current_dir(&dir)
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_TIER", tier)
        // The eager policy loads every recorded module at drain: the question is the lazy one.
        .env_remove("BROOD_NO_LAZY_LOAD");
    if imaged {
        cmd.env_remove("BROOD_NO_STDIMAGE");
    } else {
        cmd.env("BROOD_NO_STDIMAGE", "1");
    }
    let out = cmd.arg("program.blsp").output().expect("run brood");
    let _ = std::fs::remove_dir_all(&dir);
    (
        String::from_utf8_lossy_owned(out.stdout),
        String::from_utf8_lossy_owned(out.stderr),
    )
}

fn loads_nothing_extra_at(tier: &str, imaged: bool) {
    let (stdout, stderr) = run(tier, imaged);
    let context = || format!("tier {tier}, imaged {imaged}\nstdout:\n{stdout}\nstderr:\n{stderr}");
    // The summary lines are present, so the program ran to the end.
    assert!(
        stdout.contains("total=1250025000"),
        "the module did not load:\n{}",
        context()
    );
    assert!(
        stdout.contains("after-require zlib=false"),
        "loading `hotmod` also loaded `zlib`, which only a cold branch names — a tier-up \
         pre-load ran inside the module's load:\n{}",
        context()
    );
    assert!(
        stdout.contains("cold-branch-loads=true"),
        "calling the cold branch did not load its module:\n{}",
        context()
    );
}

#[test]
fn a_tier_up_inside_a_module_load_loads_no_other_module_with_the_jit() {
    loads_nothing_extra_at("2", true);
}

#[test]
fn a_tier_up_inside_a_module_load_loads_no_other_module_with_the_jit_from_source() {
    loads_nothing_extra_at("2", false);
}

#[test]
fn a_module_load_loads_only_what_it_requires_on_the_vm() {
    loads_nothing_extra_at("1", true);
}

#[test]
fn a_module_load_loads_only_what_it_requires_on_the_vm_from_source() {
    loads_nothing_extra_at("1", false);
}
