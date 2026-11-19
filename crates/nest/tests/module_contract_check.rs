//! A module contract is checked where it is implemented, across files (ADR-761).
//!
//! `defbehaviour` and `(:implements …)` are gone: a contract a module must satisfy is a
//! `defability`, and the module defines a value and `impl`s the ability for it. The point of
//! the contract is that leaving out a required op is a check finding — but the ability is
//! declared in one module (a framework's) and implemented in another (the app's), and the
//! checker read only the abilities the CHECKED FILE declared, so an `impl` of an imported
//! ability passed in silence. This runs `nest check` over exactly that split.

mod support;
use std::process::Command;
use support::temp_dir;

const CONTRACT: &str = "(defmodule contract/viewable)\n\
                        (defability Viewable\n  \
                          (view-mount (self params))\n  \
                          (view-render (self model))\n  \
                          (view-tick (self model) model))\n";

/// Three views: one missing a required op, one naming the ability qualified with a
/// wrong arity, and one that meets the contract and leaves the provided op to its default.
const VIEWS: &str = "(defmodule views/screens (:use contract/viewable))\n\
                     (defrecord missing-view ())\n\
                     (impl Viewable missing-view (view-mount (_ params) params))\n\
                     (defrecord wrong-view ())\n\
                     (impl contract/viewable/Viewable wrong-view\n  \
                       (view-mount (_ params) params)\n  \
                       (view-render (_ model extra) model))\n\
                     (defrecord good-view ())\n\
                     (impl Viewable good-view\n  \
                       (view-mount (_ params) params)\n  \
                       (view-render (_ model) model))\n";

#[test]
fn an_impl_of_an_ability_declared_in_another_module_is_checked_against_it() {
    let temp = temp_dir("module-contract-check");
    let root = &temp.path;
    std::fs::create_dir_all(root.join("src/contract")).unwrap();
    std::fs::create_dir_all(root.join("src/views")).unwrap();
    std::fs::write(
        root.join("project.blsp"),
        "(project :name \"contracts\" :version \"0.1.0\" :source-paths [\"src\"])\n",
    )
    .unwrap();
    std::fs::write(root.join("src/contract/viewable.blsp"), CONTRACT).unwrap();
    std::fs::write(root.join("src/views/screens.blsp"), VIEWS).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_nest"))
        .arg("check")
        .current_dir(root)
        .env("BROOD_NO_CHECK_CACHE", "1")
        .output()
        .expect("run nest check");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let findings: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("screens.blsp") && line.contains("[conformance]"))
        .collect();

    assert!(
        findings
            .iter()
            .any(|line| line.contains(":3:") && line.contains("missing op `view-render`")),
        "the impl missing a required op was not reported:\n{text}"
    );
    assert!(
        findings.iter().any(|line| line.contains(":5:")
            && line.contains("op `view-render` takes 2 arg(s), this impl has 3")),
        "the qualified impl's arity mismatch was not reported:\n{text}"
    );
    assert_eq!(
        findings.len(),
        2,
        "the view that meets the contract drew a finding (the provided op is optional):\n{text}"
    );
    assert!(
        !output.status.success(),
        "a contract finding must fail the check"
    );
}

/// One kind's ops split across two modules — the connection's in one, the editor's fold in
/// another, bedit's LSP-request layering — meet the contract between them: neither `impl`
/// alone is incomplete. Each `impl` form was checked on its own, so both halves were
/// reported; the contract is per id, and a project check has loaded both modules.
#[test]
fn a_contract_met_by_impls_in_two_modules_is_not_reported() {
    let temp = temp_dir("module-contract-split");
    let root = &temp.path;
    std::fs::create_dir_all(root.join("src/contract")).unwrap();
    std::fs::create_dir_all(root.join("src/requests")).unwrap();
    std::fs::write(
        root.join("project.blsp"),
        "(project :name \"split\" :version \"0.1.0\" :source-paths [\"src\"])\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/contract/request.blsp"),
        "(defmodule contract/request)\n\
         (defability Request (request-method (self)) (request-fold (self model)))\n\
         (defrecord hover ())\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/requests/wire.blsp"),
        "(defmodule requests/wire (:use contract/request))\n\
         (impl Request contract/request/hover (request-method (_) \"textDocument/hover\"))\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/requests/fold.blsp"),
        "(defmodule requests/fold (:use contract/request) (:use requests/wire))\n\
         (impl Request contract/request/hover (request-fold (_ model) model))\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_nest"))
        .arg("check")
        .current_dir(root)
        .env("BROOD_NO_CHECK_CACHE", "1")
        .output()
        .expect("run nest check");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !text.contains("[conformance]"),
        "a contract met across two modules was reported as incomplete:\n{text}"
    );
    assert!(output.status.success(), "{text}");
}
