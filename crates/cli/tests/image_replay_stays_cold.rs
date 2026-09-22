//! **Materialising a module from the stdlib image must not make prelude code hot.**
//!
//! Found under KI-182: `(math/max 1 2)` compiled one JIT arm — `not` — on every run, on the
//! binary before the KI as well as after. The image replay of a module's ability impls
//! (`%replay-std-impls!`) went through `%register-impl`, which re-ran the arity diagnostic per
//! impl in Brood (`%impl-fn-arity` → `take-while`, an `nth` walk): ~13 `not` calls per impl,
//! 164 on `math`'s twelve, past the tier threshold, so every short run that touched `math` or
//! `datetime` instantiated Cranelift at boot to compile `not`. The replay now takes the
//! registration proper (`%install-impl`); the diagnostic ran when the image was written.
//!
//! The probe rebinds `not` under `%load-module-source`'s reserved-name exemption to count its
//! calls, materialises `math` through a lazy reference, and prints the count. Two assertions,
//! because each alone can pass vacuously: the count stays well under the tier threshold (128),
//! and `BROOD_JIT_DUMP_IR=1` prints no lowered arm. `[image] math` on stderr proves the load
//! really was a replay — with no image the replay never runs and the count is trivially small.
//! The probe prints through the kernel's `%write-out`, not `io/puts`: loading `io` (and
//! `string` behind it) on top would make `nth` itself hot across three replays, which is a
//! program doing real work, not the replay's own cost.

use std::path::PathBuf;
use std::process::Command;

mod support;

struct TempDir {
    path: PathBuf,
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

const PROGRAM: &str = "\
(def %not-total 0)\n\
(%load-module-source \"(def not (fn (x) (do (def %not-total (+ %not-total 1)) (if x false true))))\" \"not-probe\")\n\
(def x math/max)\n\
(def %after-math %not-total)\n\
(%write-out (str \"not-calls-during-math-load: \" %after-math \"\\n\"))\n";

#[test]
fn materialising_math_from_the_image_keeps_not_cold() {
    // No replay without an image: CI's tree-walker job sets `BROOD_NO_STDIMAGE=1` to cover
    // the source path, and there this test has nothing to measure.
    if std::env::var_os("BROOD_NO_STDIMAGE").is_some() {
        return;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = TempDir {
        path: std::env::temp_dir()
            .join(format!("brood-replay-cold-{}-{nanos}", std::process::id())),
    };
    std::fs::create_dir_all(&dir.path).expect("create temp dir");
    std::fs::write(dir.path.join("program.blsp"), PROGRAM).expect("write program");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg("program.blsp")
        .current_dir(&dir.path)
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_IMAGE_TRACE", "1")
        .env("BROOD_JIT_DUMP_IR", "1")
        .env_remove("BROOD_CONTRACTS");
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run brood");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the program should run:\n{stdout}{stderr}"
    );
    assert!(
        stderr.lines().any(|l| l.trim() == "[image] math"),
        "the load must be a replay from the stdlib image, or this test measures nothing:\n{stderr}"
    );
    let count: usize = stdout
        .lines()
        .find_map(|l| l.strip_prefix("not-calls-during-math-load: "))
        .and_then(|n| n.trim().parse().ok())
        .expect("the probe prints its count");
    assert!(
        count < 64,
        "materialising math called `not` {count} times — the impl replay is re-running the \
         arity diagnostic (tier threshold is 128; it read 164 before the fix)"
    );
    let lowered = stderr.lines().filter(|l| l.starts_with("[jit-ir]")).count();
    assert_eq!(
        lowered, 0,
        "no arm should reach the JIT during a module's replay from the image:\n{stderr}"
    );
}
