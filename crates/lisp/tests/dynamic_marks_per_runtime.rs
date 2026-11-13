//! **The `defdyn` marks belong to a runtime, not to the OS process** (KI-224).
//!
//! `%isolate` snapshots the dynamic-variable set before its thunk and restores it wholesale
//! afterwards. While that set was a process-wide static, two runtimes in one process (cargo
//! test threads, an embedder with several `Interp`s) shared it, so one runtime's restore
//! dropped the marks another had added meanwhile — and `stdimage/build` refused with "root
//! global(s) belong to no module … fuzzy/*workers*" when a sibling runtime built at the same
//! time. Both directions are pinned here, with the interleaving forced by file handshakes
//! rather than left to timing: runtime A sits inside `%isolate` while runtime B declares.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use brood::Interp;

/// A fresh, empty handshake directory for one test.
fn handshake_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("brood-ki224-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create the handshake directory");
    directory
}

/// Block until `path` exists — the Rust side of the handshake.
fn await_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The Brood side of the handshake: park until `path` exists.
const AWAIT_FILE: &str = "(defn ki224-await (path)
  (if (file/exists? path) :ok (do (proc/sleep 2) (ki224-await path))))";

fn eval(interp: &mut Interp, source: &str) -> String {
    let value = interp
        .eval_str(source)
        .unwrap_or_else(|error| panic!("{source}: {error}"));
    interp.print(value)
}

/// Runtime A runs `body` inside `%isolate`, announcing `entered` once inside and leaving only
/// after `release` exists; afterwards it evaluates `after` and returns that result.
fn isolate_in_thread(
    entered: PathBuf,
    release: PathBuf,
    body: &'static str,
    after: &'static str,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut runtime = Interp::new();
        eval(&mut runtime, AWAIT_FILE);
        eval(
            &mut runtime,
            &format!(
                "(%isolate (fn () {body} (file/spit {:?} \"\") (ki224-await {:?})))",
                entered.display().to_string(),
                release.display().to_string()
            ),
        );
        eval(&mut runtime, after)
    })
}

/// A's isolate restore must not drop a mark B added while A was inside.
#[test]
fn an_isolate_restore_keeps_another_runtimes_new_mark() {
    let directory = handshake_directory("keep");
    let (entered, release) = (directory.join("entered"), directory.join("release"));
    let runtime_a = isolate_in_thread(entered.clone(), release.clone(), ":idle", ":done");
    let mut runtime_b = Interp::new();
    await_file(&entered);
    eval(&mut runtime_b, "(defdyn *ki224-kept* 1)");
    std::fs::write(&release, "").expect("release runtime A");
    assert_eq!(runtime_a.join().expect("runtime A"), ":done");
    // A has restored its snapshot (taken before B's defdyn). B's mark must survive it.
    assert_eq!(
        eval(&mut runtime_b, "(reflect/dynamic? '*ki224-kept*)"),
        "true",
        "another runtime's %isolate restore dropped this runtime's defdyn mark (KI-224)"
    );
    assert_eq!(
        eval(&mut runtime_b, "(binding (*ki224-kept* 2) *ki224-kept*)"),
        "2"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// A mark made inside A's isolate never reaches B, and is gone from A after the restore.
///
/// The thunk runs `defdyn`'s run-time half rather than `defdyn` itself: the macro ALSO marks
/// at expansion time (so a `binding` later in the file sees it), and expanding the `%isolate`
/// form happens before the isolate takes its snapshot — a mark the isolate never owned.
#[test]
fn a_mark_inside_an_isolate_stays_in_its_runtime_and_is_rolled_back() {
    let directory = handshake_directory("leak");
    let (entered, release) = (directory.join("entered"), directory.join("release"));
    let runtime_a = isolate_in_thread(
        entered.clone(),
        release.clone(),
        "(%declare-dynamic '*ki224-isolated*) (def *ki224-isolated* 1)",
        "(reflect/dynamic? '*ki224-isolated*)",
    );
    let mut runtime_b = Interp::new();
    await_file(&entered);
    let seen_by_b = eval(&mut runtime_b, "(reflect/dynamic? '*ki224-isolated*)");
    std::fs::write(&release, "").expect("release runtime A");
    let seen_by_a_after = runtime_a.join().expect("runtime A");
    assert_eq!(
        seen_by_b, "false",
        "a defdyn in one runtime marked the name dynamic in another (KI-224)"
    );
    assert_eq!(
        seen_by_a_after, "false",
        "%isolate kept a defdyn mark made inside its thunk"
    );
    let _ = std::fs::remove_dir_all(&directory);
}
