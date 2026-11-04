# Build and CI — why the configuration is shaped as it is

The rationale behind settings in `Cargo.toml` and the workflows, with the measurements that
justified them. The config files themselves keep a one-line pointer here, so they stay
readable as configuration. The measurements are dated: re-take one before relying on its
number.

## The test and dev profiles are optimized (`Cargo.toml` `[profile.test]`, `[profile.dev]`)

Brood's test suite is an interpreter exercising itself, so it is exactly the workload an
unoptimized build punishes most. Measured 2026-08-27 on one commit and the same ~1200
in-language cases: **933 s** for `brood_suite_passes` under the stock (opt-level 0) test
profile, against **58 s** for the identical suite on the release binary — a **16x** tax that
was entirely the profile. `.config/nextest.toml`'s budget history recorded the symptom: the
cap on that one wrapper went 300 s → 600 s → 1200 s → 2700 s while the release path did the
same work in 88 s.

- **`opt-level = 2`, not 3:** on this workload 3 buys little over 2 and costs noticeably more
  compile time, which is the whole price being paid.
- **`debug-assertions = true`, stated explicitly** so an edit cannot drop it: optimized +
  assertions armed keeps the per-deref GC tripwire and the heap verifier live at release
  speed (CLAUDE.md § Debug tooling). The tests must run with them armed — they are how a
  use-after-GC is caught at the bad deref instead of as a distant SIGSEGV.
- **`profile.dev` matches `profile.test`** because `cargo test` builds the crate under test
  with `test` but its path dependencies with `dev`, and `crates/lisp` is a dependency of every
  other crate's test binaries.
- **`debug = "line-tables-only"`.** Full DWARF made each test binary ~500 MB and
  `target/debug` 30 GB. Measured 2026-08-27, `cargo test --workspace --no-run` from clean:
  full DWARF 3m44s / 30 GB, line tables 2m28s / 20 GB (understating the gain: the second run
  also rebuilt the third-party graph the first reused). Line tables keep what this repo reads
  — `file:line` in a backtrace (`RUST_BACKTRACE=1` is defaulted in both binaries' `main`, and
  `cli_support::install_crash_dump` writes `.brood_crash_dump`) and function names for
  `gdb --batch -ex run -ex bt`.

## Clippy is its own CI job (`ci.yml` `lint`)

The reason is serialisation, not shared artifacts. `cargo clippy` runs `clippy-driver`, whose
outputs a real build cannot reuse (verified 2026-08-27: with the feature sets matched exactly,
the next `cargo test` still recompiled the whole graph). In one job that unshareable
~120–140 s sat in front of every test step; split, the jobs run concurrently —
`max(lint, test)` rather than `lint + test`. Full `--all-features` lint coverage is kept:
narrowing clippy to the test features would have bought nothing, for the reason above, and
would have stopped linting every gui/audio/sanitizer path.

## The release workflow's cache is restore-only (`release.yml`, `save-if: false`)

A repository gets 10 GB of Actions cache, and GitHub evicts least-recently-used entries past
it. On 2026-08-27 this repo sat at 10.53 GB across 18 caches, ~6.4 GB of it the release
workflow (four cross-compile targets times several stale copies) against 2.7 GB for the three
CI caches that run on every push — so the tag-only workflow was evicting the per-push ones,
and CI paid cold builds while appearing cached. `save-if: false` keeps the restore and stops
the release workflow claiming budget: releases are rare and already minutes of
cross-compilation, while CI runs on every push and is where a cold build costs someone
waiting.

## macOS and Windows run a smoke set, not the suite (`macos-check`, `windows-smoke`)

The suite runs on Linux only: most of it is the language, which does not change with the OS,
and a macOS minute costs ten Linux ones. What does change — paths, files, processes, the pty,
the clipboard — is run on both by `scripts/ci/platform-smoke.sh`, with a `nest` built there
with the gui, headless. The script lists each file and why it is in the set; the portable half
(`tests/platform_test.blsp`, `buffer_test`, `strings_test`, `encoding_test`, `clipboard_test`)
runs on both, the POSIX-userland half (`path_test`, `file_test`, `io_test`, `system_test`,
`proc_test`) on macOS only, since it assumes `sh`, `pwd` and `/tmp`. Both jobs also run the gui
key translation unit tests (`host::gui::backend::input`, ADR-541's Option-as-Meta and AltGr).

What that proves, per OS (2026-11-04):

| | macOS | Windows |
|---|---|---|
| home, temp, cwd, absolute paths (ADR-592) | ✅ | ✅ (drive roots, `/` form) |
| CRLF / BOM bytes untranslated; the editor buffer keeps both | ✅ | ✅ |
| names with spaces and non-ASCII | ✅ | ✅ |
| `os/which`; a command line through the platform's shell (`os/shell-cmd`) | ✅ `sh -c` | ✅ `cmd.exe /d /s /c`, the line raw: quotes, pipes, `&&` (KI-364) |
| a timeout, C-g or `os/close` stops the child's children too (`os/cmd`, `os/spawn`) | ✅ process group | ✅ Job Object (ADR-592) |
| a pty (`os/spawn-pty`) and POSIX signals | ✅ | ❌ no pty — KI-369 (raises naming ConPTY); no POSIX signals |
| clipboard reads against a real clipboard | ✅ | ✅ |
| owner-only files (`file/spit-private`) | ✅ mode 0600 | ✅ a one-entry DACL, read back with `icacls` (KI-365) |
| key translation (Option / AltGr) | ✅ unit tests | ✅ unit tests |
| `os/detach` (and `os/open`'s opener started detached) | ✅ | ✅ |
| a window, font discovery, file watching | not run (headless) | not run (headless) |

What the first runs found: a test asserted `/tmp` where macOS's `pwd` says `/private/tmp`
(fixed in the test); two clipboard reads at once answer `Busy` on any machine that has a
clipboard — by design, so the reading tests run `:isolated`; and, from bedit's smoke job on
Windows, `os/cmd` there killed only its child, leaving a grandchild running and holding the
pipes — the Job Object of ADR-592.
