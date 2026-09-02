//! The incremental check-result cache (ADR-119) must be keyed on the CHECKING MODE.
//!
//! A verdict depends on the mode that produced it, and the cache stored only the file's
//! mtime, its dependency fingerprint and its require-closure — none of which move when
//! `--strict` is added. So a plain `nest check` cached its verdicts and the next
//! `nest check --strict` over the same unchanged files reused them: the strict run
//! reported what the PLAIN run had found and exited 0. CI runs the two gates back to back
//! over `std/**` — the strict gate would have gone quiet the moment the plain one warmed
//! the cache, with nothing to see but a passing run.
//!
//! The probe is a value that can be a `failure` handed to a function that cannot take one
//! (ADR-310): silent under the gradual overlap rule, reported under strict.

use std::path::Path;
use std::process::Command;

struct TempDir {
    path: std::path::PathBuf,
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn project() -> TempDir {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("brood-ckmode-{}-{n}", std::process::id()));
    std::fs::create_dir_all(path.join("src")).unwrap();
    std::fs::write(
        path.join("project.blsp"),
        "(project :name \"ckmode\" :version \"0.1.0\" :source-paths [\"src\"])\n",
    )
    .unwrap();
    std::fs::write(
        path.join("src/main.blsp"),
        "(defmodule main \"d\")\n\
         (sig p (string -> (or string failure)))\n\
         (defn p (s) s)\n\
         (defn q (s) (string/length (p s)))\n",
    )
    .unwrap();
    TempDir { path }
}

fn nest(dir: &Path, args: &[&str]) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run nest");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.success(),
    )
}

#[test]
fn a_plain_check_does_not_cache_its_verdict_for_a_strict_one() {
    let proj = project();

    // Plain: the failure arm is merely wider, so the gradual overlap rule passes it.
    let (out, ok) = nest(&proj.path, &["check", "src/main.blsp"]);
    assert!(ok, "the plain check should be clean:\n{out}");
    assert!(
        !out.contains("failure"),
        "plain mode must not report the failure arm:\n{out}"
    );

    // Strict, over the same unchanged file: it must re-check rather than reuse.
    let (out, ok) = nest(&proj.path, &["check", "--strict", "src/main.blsp"]);
    assert!(
        !ok && out.contains("string | failure"),
        "the strict check must report what strict finds, not what the cached plain run \
         found:\n{out}"
    );

    // …and the strict run's entries are not the plain run's to reuse either.
    let (out, ok) = nest(&proj.path, &["check", "src/main.blsp"]);
    assert!(
        ok && !out.contains("failure"),
        "the plain check must stay clean after a strict run:\n{out}"
    );
}
