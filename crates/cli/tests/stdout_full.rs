//! **Output that cannot be written is an error, not a silent success** (KI-244).
//!
//! `brood report.blsp > out.txt` on a full disk dropped every line and exited 0, so a
//! script's caller saw success and a truncated file. `/dev/full` fails every write with
//! `ENOSPC`, which is the full disk without filling one.

use std::process::{Command, Stdio};

fn run_into_full_stdout(script: &str) -> (Option<i32>, String) {
    let directory = std::env::temp_dir().join(format!(
        "brood-ki244-{}-{}",
        script.len(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create the scratch directory");
    std::fs::write(directory.join("main.blsp"), script).expect("write the script");
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("open /dev/full");
    let output = Command::new(env!("CARGO_BIN_EXE_brood"))
        .arg("main.blsp")
        .current_dir(&directory)
        .stdout(Stdio::from(full))
        .stderr(Stdio::piped())
        .output()
        .expect("run brood");
    let _ = std::fs::remove_dir_all(&directory);
    (
        output.status.code(),
        String::from_utf8_lossy_owned(output.stderr),
    )
}

#[test]
fn a_run_whose_output_is_lost_fails() {
    let (status, stderr) = run_into_full_stdout(r#"(io/puts "the report")"#);
    assert!(
        stderr.contains("cannot write to stdout"),
        "nothing said the output was lost:\n{stderr}"
    );
    assert_ne!(status, Some(0), "the run claimed success:\n{stderr}");
}

#[test]
fn a_failed_write_is_catchable() {
    let (status, stderr) = run_into_full_stdout(
        r#"(io/puts "caught:" (try (do (io/puts "x") :written) (catch e (error-message e))) :to *err*)"#,
    );
    assert!(
        stderr.contains("caught: cannot write to stdout"),
        "the failed write was not raised where it happened:\n{stderr}"
    );
    assert_eq!(status, Some(0), "{stderr}");
}
