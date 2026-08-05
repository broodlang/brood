//! `nest update-tooling` refreshes the AI-assistant files `nest new` drops into a
//! project — the language reference `docs/brood-for-claude.md` and the
//! `.claude/skills/writing-brood` skill — from the installed `brood` build. They
//! are baked in via `%builtin-doc`, so they drift as the language evolves; this
//! command re-syncs them. Scaffold a project, stale the reference and delete the
//! skill, refresh, and assert both are back to the shipped content.

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

fn tempdir() -> TempDir {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("brood-uptool-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    TempDir { path }
}

fn nest(dir: &Path, args: &[&str]) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run nest");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (text, out.status.success())
}

#[test]
fn update_tooling_refreshes_the_reference_and_skill() {
    let tmp = tempdir();
    let (out, ok) = nest(&tmp.path, &["new", "demo"]);
    assert!(ok, "nest new failed: {out}");
    let proj = tmp.path.join("demo");
    let reference = proj.join("docs/brood-for-claude.md");
    let skill = proj.join(".claude/skills/writing-brood/SKILL.md");

    // A newly-scaffolded project already ships current tooling; corrupt/remove it.
    std::fs::write(&reference, "STALE — the old --name privacy convention").unwrap();
    std::fs::remove_file(&skill).unwrap();

    let (out, ok) = nest(&proj, &["update-tooling"]);
    assert!(ok, "nest update-tooling failed: {out}");

    // The reference is back to the shipped content (and teaches the current
    // def-site privacy, not the removed `--` convention).
    let refreshed = std::fs::read_to_string(&reference).unwrap();
    assert!(
        refreshed.starts_with("# Brood — a quick reference for Claude"),
        "reference not refreshed: {}",
        &refreshed[..refreshed.len().min(60)]
    );
    assert!(
        refreshed.contains("defn-"),
        "refreshed reference should teach def-site privacy"
    );
    // The deleted skill is restored.
    assert!(skill.exists(), "writing-brood SKILL.md was not restored");
    assert!(!std::fs::read_to_string(&skill).unwrap().is_empty());
}

#[test]
fn update_tooling_outside_a_project_errors() {
    let tmp = tempdir(); // bare dir, no project.blsp
    let (out, ok) = nest(&tmp.path, &["update-tooling"]);
    assert!(!ok, "expected failure outside a project");
    assert!(
        out.contains("no project.blsp"),
        "expected a no-project message, got: {out}"
    );
}
