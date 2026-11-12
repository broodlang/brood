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
//! The probe is a `nil | string` handed to a function wanting a `string`: consistent under
//! the gradual overlap rule (some materialisation fits), reported under strict, which reads
//! a positively-known bound by inclusion. It must be a shape only STRICT reports — a
//! failure arm is not, since ADR-316 reports one in both modes.

mod support;
use support::{TempDir, nest, temp_dir};

fn project() -> TempDir {
    let temp = temp_dir("ckmode");
    let path = &temp.path;
    std::fs::create_dir_all(path.join("src")).unwrap();
    std::fs::write(
        path.join("project.blsp"),
        "(project :name \"ckmode\" :version \"0.1.0\" :source-paths [\"src\"])\n",
    )
    .unwrap();
    std::fs::write(
        path.join("src/main.blsp"),
        "(defmodule main \"d\")\n\
         (sig p (string -> (or nil string)))\n\
         (defn p (s) s)\n\
         (defn q (s) (%string-length (p s)))\n",
    )
    .unwrap();
    temp
}

#[test]
fn a_plain_check_does_not_cache_its_verdict_for_a_strict_one() {
    let proj = project();

    // Plain: the `nil` arm is merely wider, so the gradual overlap rule passes it.
    let (out, ok) = nest(&proj.path, &["check", "src/main.blsp"]);
    assert!(ok, "the plain check should be clean:\n{out}");
    assert!(
        !out.contains("nil | string"),
        "plain mode must not report a merely-wider bound:\n{out}"
    );

    // Strict, over the same unchanged file: it must re-check rather than reuse.
    let (out, ok) = nest(&proj.path, &["check", "--strict", "src/main.blsp"]);
    assert!(
        !ok && out.contains("nil | string"),
        "the strict check must report what strict finds, not what the cached plain run \
         found:\n{out}"
    );

    // …and the strict run's entries are not the plain run's to reuse either.
    let (out, ok) = nest(&proj.path, &["check", "src/main.blsp"]);
    assert!(
        ok && !out.contains("nil | string"),
        "the plain check must stay clean after a strict run:\n{out}"
    );
}
