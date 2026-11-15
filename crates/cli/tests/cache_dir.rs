//! Every cache Brood writes lives under one directory, chosen by one rule
//! (`boot::cache_dir`): `$XDG_CACHE_HOME/brood`, else `$HOME/.cache/brood`, with a relative
//! or empty `XDG_CACHE_HOME` ignored as the XDG spec says.
//!
//! The rule was written out seven times — four in Rust, three in Brood — and they
//! disagreed: the Rust copies ignored a relative `XDG_CACHE_HOME`, the Brood ones (the
//! stdlib image's path among them) used it. One process could write the stdlib image where
//! the next one's boot never looked. The Brood side now asks the kernel (`%cache-dir`).

#![cfg(unix)]

use std::process::Command;

fn cache_paths(xdg: &str, home: &std::path::Path) -> String {
    let dir = home.join("work");
    std::fs::create_dir_all(&dir).unwrap();
    let program = dir.join("paths.blsp");
    std::fs::write(
        &program,
        "(io/puts (%cache-dir))\n(io/puts (stdimage/image-path))\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        .arg(&program)
        .current_dir(&dir)
        .env("XDG_CACHE_HOME", xdg)
        .env("HOME", home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "brood failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy_owned(out.stdout)
}

#[test]
fn a_relative_xdg_cache_home_is_ignored_by_every_cache() {
    let home = std::env::temp_dir().join(format!("brood-cache-dir-{}", std::process::id()));
    let expected = home.join(".cache/brood");
    for xdg in ["relative/cache", ""] {
        let out = cache_paths(xdg, &home);
        let mut lines = out.lines();
        let kernel = lines.next().unwrap_or_default();
        let image = lines.next().unwrap_or_default();
        assert_eq!(
            kernel,
            expected.to_string_lossy(),
            "XDG_CACHE_HOME={xdg:?}: the kernel's cache directory"
        );
        assert!(
            image.starts_with(&*expected.to_string_lossy()),
            "XDG_CACHE_HOME={xdg:?}: the stdlib image must live in the same directory, got {image}"
        );
    }
    let _ = std::fs::remove_dir_all(&home);
}
