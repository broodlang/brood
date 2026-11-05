//! A tool version pinned in more than one place must say the same thing everywhere.
//!
//! cargo-nextest is pinned in each CI job that installs it and in the Makefile's
//! `ensure-nextest`, and one nightly (now full.yml) job used to float on `install-action@nextest` while
//! the rest were pinned — a floating tool is an unreviewed bump on every run (0.9.144's
//! checksum failure redded CI before any test ran).

const SOURCES: [(&str, &str); 3] = [
    (
        ".github/workflows/ci.yml",
        include_str!("../../../.github/workflows/ci.yml"),
    ),
    (
        ".github/workflows/full.yml",
        include_str!("../../../.github/workflows/full.yml"),
    ),
    ("Makefile", include_str!("../../../Makefile")),
];

/// Every `cargo-nextest@X` in the workflows and the Makefile's `NEXTEST_VERSION := X`.
fn nextest_pins() -> Vec<(&'static str, String)> {
    let mut pins = Vec::new();
    for (file, text) in SOURCES {
        for line in text.lines() {
            let line = line.trim();
            if let Some(version) = line.strip_prefix("tool: cargo-nextest@") {
                pins.push((file, version.trim().to_string()));
            }
            if let Some(version) = line.strip_prefix("NEXTEST_VERSION :=") {
                pins.push((file, version.trim().to_string()));
            }
        }
    }
    pins
}

#[test]
fn every_nextest_pin_names_the_same_version() {
    let pins = nextest_pins();
    assert!(
        pins.iter().any(|(file, _)| *file == "Makefile") && pins.len() >= 4,
        "found too few nextest pins — the scan has broken: {pins:?}"
    );
    let first = &pins[0].1;
    let disagreeing: Vec<_> = pins.iter().filter(|(_, v)| v != first).collect();
    assert!(
        disagreeing.is_empty(),
        "cargo-nextest is pinned to different versions: {pins:?}"
    );
}

/// Every Makefile target is declared `.PHONY`: none builds a file of its name, and an
/// undeclared one does nothing at all once such a file or directory exists.
#[test]
fn every_makefile_target_is_phony() {
    let makefile = include_str!("../../../Makefile");
    let mut phony = std::collections::BTreeSet::new();
    let mut in_phony = false;
    for line in makefile.lines() {
        let body = match line.strip_prefix(".PHONY:") {
            Some(rest) => rest,
            None if in_phony => line,
            None => continue,
        };
        in_phony = body.trim_end().ends_with('\\');
        phony.extend(
            body.trim_end_matches('\\')
                .split_whitespace()
                .map(str::to_string),
        );
    }
    let targets: Vec<&str> = makefile
        .lines()
        .filter_map(|line| {
            let (name, _) = line.split_once(':')?;
            let is_target = !name.is_empty()
                && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && !line[name.len()..].starts_with(":=")
                && !line[name.len()..].starts_with("::=");
            is_target.then_some(name)
        })
        .collect();
    assert!(targets.len() > 30, "found only {} targets", targets.len());
    let missing: Vec<_> = targets.iter().filter(|t| !phony.contains(**t)).collect();
    assert!(
        missing.is_empty(),
        "Makefile targets missing from .PHONY: {missing:?}"
    );
}

#[test]
fn no_workflow_installs_a_floating_nextest() {
    for (file, text) in SOURCES {
        assert!(
            !text.contains("install-action@nextest"),
            "{file} installs nextest from the floating `install-action@nextest` tag — pin it"
        );
    }
}

/// The run cap is one number (`scripts/lib/run-cap.sh`); it was once hard-coded in five
/// places, so a change to it would have missed some. Every place that applies it reads it.
#[test]
fn the_run_cap_is_spelled_once() {
    let cap = include_str!("../../../scripts/lib/run-cap.sh")
        .lines()
        .find_map(|line| line.strip_prefix("RUN_CAP_KB="))
        .expect("scripts/lib/run-cap.sh defines RUN_CAP_KB");
    let readers = [
        ("Makefile", include_str!("../../../Makefile")),
        (
            "scripts/git-hooks/pre-push",
            include_str!("../../../scripts/git-hooks/pre-push"),
        ),
        (
            "scripts/tier-audit.sh",
            include_str!("../../../scripts/tier-audit.sh"),
        ),
        (
            "scripts/latency-gate.sh",
            include_str!("../../../scripts/latency-gate.sh"),
        ),
        (
            "scripts/smoke-bedit.sh",
            include_str!("../../../scripts/smoke-bedit.sh"),
        ),
    ];
    for (file, text) in readers {
        assert!(
            !text.contains(cap),
            "{file} hard-codes the run cap {cap}; read RUN_CAP_KB from scripts/lib/run-cap.sh"
        );
        assert!(
            text.contains("run-cap.sh"),
            "{file} no longer reads scripts/lib/run-cap.sh — where does its cap come from?"
        );
    }
}
