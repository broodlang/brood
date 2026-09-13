//! **An imaged module registers before it binds** (KI-134's second window, ADR-339).
//!
//! A source load has this order by construction: in `std/queue.blsp` the `(impl Conjable …)`
//! precedes `(defn list-> …)`, so no process can call `list->` and find `conj` undispatchable.
//! The stdlib-image branch of `require-one` used to materialise the whole module's BINDINGS
//! first and replay its registrations and impls afterwards — opening that window for every
//! name at once. Under lazy loading (ADR-335) a concurrent process reaches a freshly-bound
//! `queue/list->` through an ordinary global hit, with no `require-one` and no wait, and dies
//! on `conj: adding to a record takes a [k v] pair or a map`: seven of thirty three-file runs
//! under load, with no `%isolate` restore anywhere near — which is how it was mistaken for
//! the rollback window this KI started as.
//!
//! The race itself cannot be pinned deterministically; the ORDER can. `BROOD_IMAGE_TRACE`
//! prints one line when a module's registrations have been replayed and another when its
//! bindings land, so this asserts the first precedes the second for `queue`. Vacuous — and it
//! says so — when the binary boots without a stdlib image (then `queue` loads from source and
//! neither line prints), which is the state on a fresh checkout until `nest stdimage` runs;
//! the nextest setup script builds the image, so the suite run is not vacuous.

use std::process::Command;

#[test]
fn an_imaged_module_registers_its_impls_before_it_binds_its_names() {
    let path = std::env::temp_dir().join(format!("brood-image-order-{}.blsp", std::process::id()));
    std::fs::write(&path, "(io/puts (queue/peek (queue/list-> (list 7 8))))\n")
        .expect("write program");
    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_IMAGE_TRACE", "1")
        .env_remove("BROOD_NO_STDIMAGE")
        .arg(&path)
        .output()
        .expect("run brood");
    let _ = std::fs::remove_file(&path);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains('7'),
        "the program did not run:\n{stdout}\n{stderr}"
    );
    let lines: Vec<&str> = stderr.lines().collect();
    let registered = lines
        .iter()
        .position(|l| l.starts_with("[image] queue registered "));
    let bound = lines.iter().position(|l| l.trim_end() == "[image] queue");
    match (registered, bound) {
        (None, None) => {
            eprintln!(
                "image_publication_order: `queue` did not materialise from an image (none \
                 installed for this binary) — vacuous"
            );
        }
        (Some(r), Some(b)) => assert!(
            r < b,
            "queue's bindings landed (line {b}) before its registrations were replayed \
             (line {r}) — a concurrent global hit can dispatch against a module that is \
             not yet dispatchable:\n{stderr}"
        ),
        (r, b) => panic!("half a materialisation traced: registered={r:?} bound={b:?}\n{stderr}"),
    }
}
