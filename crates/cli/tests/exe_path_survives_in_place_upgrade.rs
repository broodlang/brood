//! `os/exe-path` stays usable after the running binary is replaced in place (KI-130).
//!
//! The primitive exists for ONE job: find something installed next to this binary, because
//! a shipped app cannot trust `PATH`. That job breaks silently the moment the binary is
//! upgraded. You cannot write over a busy executable (`ETXTBSY`), so `cargo`, `make
//! install` and every package manager unlink-and-rename instead — and from then on Linux's
//! `/proc/self/exe` reads `<path> (deleted)`. `current_exe()` hands that straight through,
//! so the answer names no file, `(file/exists? (os/exe-path))` is false, and the sibling
//! lookup finds nothing with no error to explain it.
//!
//! How it was found: `tests/introspection_test.blsp`'s `os/exe-path` case failed three
//! consecutive FULL-SUITE runs and passed 3/3 solo — the full-suite runs were the ones that
//! followed a `cargo build`, which had replaced `target/debug/nest` underneath it.
//!
//! The test copies the binary, deletes the copy while it runs, and asserts the answer is
//! still a usable path. **Sabotage-verified**: with the `strip_suffix` removed it fails with
//! the ` (deleted)` suffix in the assertion message.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

struct TempDir {
    path: PathBuf,
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn temp_dir(tag: &str) -> TempDir {
    let path = std::env::temp_dir().join(format!(
        "brood-exe-path-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("create temp dir");
    TempDir { path }
}

#[test]
fn exe_path_is_usable_after_the_binary_is_unlinked() {
    let dir = temp_dir("unlink");
    let copy = dir.path.join("brood");
    std::fs::copy(env!("CARGO_BIN_EXE_brood"), &copy).expect("copy the binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    // The program sleeps long enough for us to unlink it out from under itself, then reports
    // what it believes its own path is. `sleep` is the prelude's, in milliseconds.
    let program = dir.path.join("where.blsp");
    let mut f = std::fs::File::create(&program).unwrap();
    writeln!(
        f,
        "(let (_ (sleep 1500) p (os/exe-path)) (io/puts (str p \"|\" (file/exists? p))))"
    )
    .unwrap();
    drop(f);

    let child = Command::new(&copy)
        .arg(&program)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the copied binary");

    // Unlink while it runs — the in-place-upgrade shape. `remove_file` is what the
    // rename half of an upgrade does to the old inode.
    std::thread::sleep(std::time::Duration::from_millis(400));
    std::fs::remove_file(&copy).expect("unlink the running binary");

    let out = child.wait_with_output().expect("collect output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.trim();

    assert!(
        !line.contains("(deleted)"),
        "os/exe-path leaked the kernel's unlinked-inode marker; it names no file and the \
         sibling lookup this primitive exists for silently stops working. got: {line:?}\n\
         stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let reported = line.split('|').next().unwrap_or_default();
    assert_eq!(
        reported,
        copy.to_string_lossy(),
        "os/exe-path should name the path the binary was installed at — after a real \
         upgrade that is where the REPLACEMENT lives. got: {line:?}"
    );
}

/// The inverse, so the test above cannot be satisfied by making `os/exe-path` return
/// nonsense: with the binary intact the answer is the path AND the file is there.
#[test]
fn exe_path_names_an_existing_file_when_the_binary_is_intact() {
    let dir = temp_dir("intact");
    let program = dir.path.join("where.blsp");
    std::fs::write(
        &program,
        "(let (p (os/exe-path)) (io/puts (str p \"|\" (file/exists? p))))",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        .arg(&program)
        .output()
        .expect("run brood");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.trim();

    assert!(
        line.ends_with("|true"),
        "with the binary in place, os/exe-path must name a file that exists. got: {line:?}\n\
         stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        line.starts_with(env!("CARGO_BIN_EXE_brood")),
        "os/exe-path should name THIS binary. got: {line:?}"
    );
}
