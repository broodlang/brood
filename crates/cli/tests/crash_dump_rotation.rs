//! `.brood_crash_dump` stays bounded: a real `brood` whose process dies rotates a full dump
//! to `.brood_crash_dump.old` and starts again, and leaves one under the limit alone. The
//! full dump is sparse, so the test writes a few bytes rather than 16 MiB.

use std::fs;
use std::path::Path;
use std::process::Command;

mod support;
use support::temp_dir;

const LIMIT: u64 = 16 * 1024 * 1024;
const OLD_RECORD: &str = "an earlier crash\n";

/// The death is recorded before the process retires, so the `:down` proves it landed.
const DYING_PROGRAM: &str = r#"
(let (dying (spawn (error "boom")))
  (monitor dying)
  (receive ([:down _ _ _] nil) (after 20000 nil)))
"#;

fn seed_dump(dir: &Path, len: u64) {
    let path = dir.join(".brood_crash_dump");
    fs::write(&path, OLD_RECORD).expect("seed the dump");
    fs::File::options()
        .write(true)
        .open(&path)
        .and_then(|f| f.set_len(len))
        .expect("size the dump");
}

fn run_dying_program(dir: &Path) {
    let program = dir.join("dying.blsp");
    fs::write(&program, DYING_PROGRAM).expect("write the program");
    let output = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .current_dir(dir)
        .arg(&program)
        .output()
        .expect("run brood");
    assert!(
        output.status.success(),
        "brood failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn read_lossy(path: &Path) -> String {
    String::from_utf8_lossy_owned(fs::read(path).expect("read a dump"))
}

#[test]
fn a_full_dump_moves_aside_and_the_new_death_starts_a_fresh_one() {
    let dir = temp_dir("crash-dump-full");
    seed_dump(&dir.path, LIMIT);
    run_dying_program(&dir.path);

    let old = dir.path.join(".brood_crash_dump.old");
    assert!(
        read_lossy(&old).starts_with(OLD_RECORD),
        "the old record moved"
    );
    assert_eq!(fs::metadata(&old).expect("the old dump").len(), LIMIT);

    let current = read_lossy(&dir.path.join(".brood_crash_dump"));
    assert!(
        current.contains("boom"),
        "the death is in the new dump: {current}"
    );
    assert!(
        !current.contains(OLD_RECORD.trim()),
        "the new dump starts empty"
    );
}

#[test]
fn a_dump_under_the_limit_is_appended_to_and_not_moved() {
    let dir = temp_dir("crash-dump-under");
    seed_dump(&dir.path, LIMIT - 1);
    run_dying_program(&dir.path);

    assert!(!dir.path.join(".brood_crash_dump.old").exists());
    let current = read_lossy(&dir.path.join(".brood_crash_dump"));
    assert!(
        current.starts_with(OLD_RECORD),
        "the record stayed: {} bytes",
        current.len()
    );
    assert!(current.contains("boom"), "the death was appended");
}

#[test]
fn rotating_replaces_the_last_old_dump() {
    let dir = temp_dir("crash-dump-replace");
    fs::write(dir.path.join(".brood_crash_dump.old"), "the one before").expect("seed .old");
    seed_dump(&dir.path, LIMIT);
    run_dying_program(&dir.path);

    let old = read_lossy(&dir.path.join(".brood_crash_dump.old"));
    assert!(old.starts_with(OLD_RECORD), "the newer dump took its place");
}
