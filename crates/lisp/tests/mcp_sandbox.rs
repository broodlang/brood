//! The `nest mcp` write/edit sandbox must be **symlink-escape-proof** (ADR-146
//! follow-on): a project-relative, `..`-free path that resolves OUT of the tree
//! through a symlinked directory is rejected, not just the lexical
//! absolute/`~`/`..` cases. Enforced by `canonicalize` (real-path resolution)
//! in `mcp-project-path`.
//!
//! Unix-only (creates a symlink). Top-level `eval_str` may reference the
//! `mcp-project-path` private — the live-hacking hatch (ADR-146).

#![cfg(unix)]

use brood::Interp;

fn tempdir(tag: &str) -> std::path::PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("brood-mcp-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Escape the path for a Brood string literal.
fn lit(p: &std::path::Path) -> String {
    format!(
        "\"{}\"",
        brood::introspect::escape_brood_string(&p.to_string_lossy())
    )
}

#[test]
fn write_sandbox_rejects_a_symlink_escape() {
    let proj = tempdir("proj");
    let outside = tempdir("outside");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(proj.join("src/real.txt"), "x").unwrap();
    // A symlinked directory INSIDE the project pointing OUTSIDE it — the lexical
    // check (no `..`, not absolute) passes, but the real target escapes.
    std::os::unix::fs::symlink(&outside, proj.join("evil")).unwrap();

    let mut interp = Interp::new();
    interp
        .eval_str(&format!(
            "(require-one 'mcp) (def *project-root* {})",
            lit(&proj)
        ))
        .unwrap();

    // A genuine in-project path resolves fine.
    let ok = interp
        .eval_str("(try (do (mcp/mcp-project-path \"src/real.txt\") :ok) (catch e :blocked))")
        .unwrap();
    assert_eq!(
        interp.print(ok),
        ":ok",
        "an in-project path must be allowed"
    );

    // A new (not-yet-existing) in-project path is also fine.
    let ok2 = interp
        .eval_str("(try (do (mcp/mcp-project-path \"src/new.blsp\") :ok) (catch e :blocked))")
        .unwrap();
    assert_eq!(
        interp.print(ok2),
        ":ok",
        "a new in-project path must be allowed"
    );

    // The symlink escape must be blocked — its real target is outside the root.
    let blocked = interp
        .eval_str("(try (do (mcp/mcp-project-path \"evil/passwd\") :ok) (catch e :blocked))")
        .unwrap();
    assert_eq!(
        interp.print(blocked),
        ":blocked",
        "a symlinked-dir escape must be rejected"
    );

    let _ = std::fs::remove_dir_all(&proj);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn canonicalize_resolves_symlinks_and_nonexistent_tails() {
    let base = tempdir("canon");
    let outside = tempdir("canon-out");
    std::fs::create_dir_all(base.join("d")).unwrap();
    std::fs::write(base.join("d/f.txt"), "x").unwrap();
    std::os::unix::fs::symlink(&outside, base.join("link")).unwrap();

    let mut interp = Interp::new();

    // A symlinked dir is followed to its real target.
    let via_link = interp
        .eval_str(&format!(
            "(canonicalize (str {} \"/link/inside.txt\"))",
            lit(&base)
        ))
        .unwrap();
    let real_outside = std::fs::canonicalize(&outside).unwrap();
    assert_eq!(
        interp.print(via_link),
        format!("\"{}/inside.txt\"", real_outside.to_string_lossy()),
        "a symlinked prefix must resolve to its real target"
    );

    // A `..` in an existing path is normalized.
    let dotdot = interp
        .eval_str(&format!(
            "(canonicalize (str {} \"/d/../d/f.txt\"))",
            lit(&base)
        ))
        .unwrap();
    let real_base = std::fs::canonicalize(&base).unwrap();
    assert_eq!(
        interp.print(dotdot),
        format!("\"{}/d/f.txt\"", real_base.to_string_lossy())
    );

    // `..` in the NON-EXISTENT tail, behind a symlink, must resolve too — else a
    // bare `starts_with` sandbox check is escapable. `link/x/../../../out` climbs
    // out of the symlink target entirely; the result must NOT be under `base`.
    let escape = interp
        .eval_str(&format!(
            "(canonicalize (str {} \"/link/x/../../../../escaped\"))",
            lit(&base)
        ))
        .unwrap();
    let escaped = interp.print(escape);
    assert!(
        !escaped.contains("/link/") && !escaped.contains(".."),
        "canonicalize must fully resolve `..` in the tail (no literal `..`, no symlink name): {escaped}"
    );

    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&outside);
}
