//! **`BROOD_PERF_MAP=1` names native code for `perf`.** Each arm the JIT installs is appended to
//! `/tmp/perf-<pid>.map` as `address size name`, the file `perf report` reads to put a name on
//! JIT code, so a kernel-sampled profile attributes native time to Brood functions with nothing in
//! the program changed. Without the flag no file is written.
//!
//! Asserted on the real `brood` binary: a function hot enough to tier appears in the map under its
//! own name, with an address and a size `perf` can use, and an unflagged run leaves no map.

use std::process::Command;

const PROGRAM: &str = r#"
(defn perf-map-hot (i acc) (if (>= i 300000) acc (perf-map-hot (+ i 1) (+ acc (* 2 i)))))
(io/puts (perf-map-hot 0 0))
"#;

/// Run the program; returns the child's pid and its stdout.
fn run(flag: bool) -> (u32, String) {
    let path = std::env::temp_dir().join(format!("brood-perf-map-{}.blsp", std::process::id()));
    std::fs::write(&path, PROGRAM).expect("write program");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_TIER", "2")
        .env_remove("BROOD_PERF_MAP");
    if flag {
        cmd.env("BROOD_PERF_MAP", "1");
    }
    let child = cmd
        .arg(&path)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("run brood");
    let pid = child.id();
    let out = child.wait_with_output().expect("wait for brood");
    let _ = std::fs::remove_file(&path);
    (pid, String::from_utf8_lossy_owned(out.stdout))
}

#[test]
fn a_hot_function_is_named_in_the_perf_map() {
    let (pid, stdout) = run(true);
    assert_eq!(stdout.trim(), "89999700000", "the program's own answer");
    let map_path = format!("/tmp/perf-{pid}.map");
    let map =
        std::fs::read_to_string(&map_path).expect("BROOD_PERF_MAP=1 wrote no /tmp/perf-<pid>.map");
    let _ = std::fs::remove_file(&map_path);
    let line = map
        .lines()
        .find(|line| line.ends_with("brood:perf-map-hot"))
        .unwrap_or_else(|| panic!("the hot function is not in the map:\n{map}"));
    let mut fields = line.split(' ');
    let address = u64::from_str_radix(fields.next().unwrap(), 16).expect("a hex address");
    let size = u64::from_str_radix(fields.next().unwrap(), 16).expect("a hex size");
    assert!(
        address > 0 && size > 0,
        "an address and a size perf can use: {line}"
    );
}

#[test]
fn without_the_flag_no_map_is_written() {
    let (pid, stdout) = run(false);
    assert_eq!(stdout.trim(), "89999700000");
    let map_path = format!("/tmp/perf-{pid}.map");
    let written = std::path::Path::new(&map_path).exists();
    let _ = std::fs::remove_file(&map_path);
    assert!(!written, "a run without BROOD_PERF_MAP wrote {map_path}");
}
