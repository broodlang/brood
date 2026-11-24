//! `nest update-tooling` refreshes the AI-assistant files `nest new` drops into a
//! project — the language reference `docs/brood-for-claude.md` and the
//! `.claude/skills/writing-brood` skill — from the installed `brood` build. They
//! are baked in via `%builtin-doc`, so they drift as the language evolves; this
//! command re-syncs them. Scaffold a project, stale the reference and delete the
//! skill, refresh, and assert both are back to the shipped content.

mod support;
use support::{nest, temp_dir};

#[test]
fn update_tooling_refreshes_the_reference_and_skill() {
    let tmp = temp_dir("uptool");
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
    let tmp = temp_dir("uptool"); // bare dir, no project.blsp
    let (out, ok) = nest(&tmp.path, &["update-tooling"]);
    assert!(!ok, "expected failure outside a project");
    assert!(
        out.contains("no project.blsp"),
        "expected a no-project message, got: {out}"
    );
}

#[test]
fn update_tooling_works_from_a_subdirectory() {
    // Both the CLI's `require_project` and Brood's `project-find-root` walk up from
    // cwd, so the command must resolve the project root when run from a nested dir
    // (e.g. `cd src && nest update-tooling`), writing to the ROOT's docs/.claude.
    let tmp = temp_dir("uptool");
    let (out, ok) = nest(&tmp.path, &["new", "demo"]);
    assert!(ok, "nest new failed: {out}");
    let proj = tmp.path.join("demo");
    let reference = proj.join("docs/brood-for-claude.md");
    std::fs::write(&reference, "STALE").unwrap();

    let (out, ok) = nest(&proj.join("src"), &["update-tooling"]); // run from src/
    assert!(ok, "nest update-tooling from a subdir failed: {out}");
    assert!(
        std::fs::read_to_string(&reference)
            .unwrap()
            .starts_with("# Brood"),
        "the project ROOT's reference was not refreshed from a subdir"
    );
}

#[test]
fn update_tooling_merges_the_mcp_server_and_the_syntax_hook_without_clobbering() {
    // ADR-789: `.mcp.json` gains the `brood` server and `.claude/settings.json` the
    // `nest check-syntax --claude-hook` hook, beside whatever the project already has, and a
    // second run changes nothing.
    let tmp = temp_dir("uptool");
    let (out, ok) = nest(&tmp.path, &["new", "demo"]);
    assert!(ok, "nest new failed: {out}");
    let proj = tmp.path.join("demo");
    let mcp = proj.join(".mcp.json");
    let settings = proj.join(".claude/settings.json");
    // `nest new` writes both.
    assert!(std::fs::read_to_string(&mcp).unwrap().contains("\"brood\""));
    assert!(
        std::fs::read_to_string(&settings)
            .unwrap()
            .contains("nest check-syntax --claude-hook")
    );

    std::fs::write(
        &mcp,
        "{\"mcpServers\": {\"other\": {\"command\": \"x\"}}}\n",
    )
    .unwrap();
    std::fs::write(
        &settings,
        "{\"permissions\": {\"allow\": [\"Bash(ls)\"]}, \"hooks\": {\"PostToolUse\": \
         [{\"matcher\": \"Bash\", \"hooks\": [{\"type\": \"command\", \"command\": \"echo\"}]}]}}\n",
    )
    .unwrap();
    let (out, ok) = nest(&proj, &["update-tooling"]);
    assert!(ok, "nest update-tooling failed: {out}");
    let merged_mcp = std::fs::read_to_string(&mcp).unwrap();
    assert!(
        merged_mcp.contains("\"other\"") && merged_mcp.contains("\"brood\""),
        "{merged_mcp}"
    );
    let merged = std::fs::read_to_string(&settings).unwrap();
    assert!(
        merged.contains("Bash(ls)") && merged.contains("\"echo\""),
        "{merged}"
    );
    assert_eq!(
        merged.matches("nest check-syntax --claude-hook").count(),
        1,
        "{merged}"
    );

    let (out, ok) = nest(&proj, &["update-tooling"]);
    assert!(ok, "second nest update-tooling failed: {out}");
    assert!(out.contains(".mcp.json (unchanged)"), "{out}");
    assert!(out.contains("settings.json (unchanged)"), "{out}");
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), merged);
    assert_eq!(std::fs::read_to_string(&mcp).unwrap(), merged_mcp);
}
