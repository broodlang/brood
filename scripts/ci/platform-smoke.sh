#!/usr/bin/env bash
# The macOS / Windows smoke set: the Brood test files that exercise the PLATFORM layer —
# paths, files, processes, the pty, the clipboard, text width — run by a `nest` built on
# that platform. Not the suite: the suite runs on Linux, and most of it is the language,
# which does not change with the OS. These are the files where the OS shows through.
#
#   scripts/ci/platform-smoke.sh path/to/nest
#
# Every file is here for a reason, given beside it. A file that cannot run on a platform
# is left out THERE with the reason, never skipped silently inside the run.
set -euo pipefail

nest="${1:?usage: platform-smoke.sh NEST}"

# No window on a runner: the gui backend answers as if no display were attached, which is
# what the clipboard and gui tests check on purpose.
export BROOD_GUI_HEADLESS=1

# Runs on every platform: written to be portable, or touching no OS-specific surface.
portable=(
  # The platform layer in ONE portable file: home and temp directories, absolute paths
  # (a drive letter on Windows), files with spaces in their names, CRLF and BOM bytes
  # through spit/slurp, os/which and os/cmd spawning the platform's own shell.
  tests/platform_test.blsp
  # The pseudo-terminal through the platform's shell: output, one channel, the exit code,
  # :env / :cwd, the size and resize, the input mode (nil on Windows), an interactive
  # shell, close. A Unix pty, or ConPTY on Windows (KI-369).
  tests/pty_test.blsp
  # The editor buffer's file round trip: CRLF kept, a byte-order mark kept, a file's
  # coding read back as it was written — the bugs Emacs has had per OS.
  tests/buffer_test.blsp
  # string/display-width (a CJK or emoji glyph is two cells) and the string builtins the
  # renderer measures with.
  tests/strings_test.blsp
  # UTF-8 / UTF-16 / base64 / hex codecs over bytes: what a file's coding goes through.
  tests/encoding_test.blsp
  # The clipboard seam with no display: nil, never a raise (the editor's kill ring
  # stands on it).
  tests/clipboard_test.blsp
  # file/lock across two programs: flock on Unix, LockFileEx on Windows — the same answers.
  tests/file_lock_test.blsp
)

# Unix only: these assume a POSIX userland (`sh`, `pwd`, `/tmp` as a directory, a pty).
# On Windows they are not run — `tests/platform_test.blsp` covers what of them applies.
unix_only=(
  # path/* against $HOME, `~user`, and `/`-rooted paths.
  tests/path_test.blsp
  # file/* over a real directory: atomic spit, symlinks, permissions.
  tests/file_test.blsp
  # ports and the io layer over files.
  tests/io_test.blsp
  # os/cmd: cwd, env, stdin, timeouts, shell quoting — through `sh`.
  tests/system_test.blsp
  # os/spawn and os/spawn-pty: a child's stdout, signals, a pty's termios modes and
  # descriptors (tests/pty_test.blsp is the portable half of the pty).
  tests/proc_test.blsp
)

case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) files=("${portable[@]}") ;;
  *) files=("${portable[@]}" "${unix_only[@]}") ;;
esac

echo "platform smoke on $(uname -s): ${files[*]}"
"$nest" test "${files[@]}"
