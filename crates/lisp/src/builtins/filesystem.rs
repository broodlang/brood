//! The filesystem primitives: paths, directories, whole-file reads and writes (text and
//! bytes), metadata, and the atomic `%file-swap`. Policy — `file/*`, `path/*` — is Brood.

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value, kw};
use crate::error::{LispError, LispResult};

use super::args::{arg, expect_int, expect_string};
use super::bytes::{bytes_to_value, flatten_iolist};

/// The native `body`, named `native`, as a file call a responsive process is told of when
/// it held the process (ADR-517's file addendum, `process::responsive::file_call`): a read,
/// write, stat or listing can wait on a network mount or a spun-down disk. `paths` is how
/// many leading arguments are paths, for the report's detail.
macro_rules! watched {
    ($native:literal, $body:path, $paths:literal) => {{
        fn watched(
            args: &[crate::core::value::Value],
            env: crate::core::value::EnvId,
            heap: &mut crate::core::heap::Heap,
        ) -> crate::error::LispResult {
            crate::process::responsive::file_call(heap, $native, args, $paths, |heap| {
                $body(args, env, heap)
            })
        }
        watched
    }};
}
pub(super) use watched;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::{Arity, Tag};
    use crate::types::{Sig, Ty};
    // filesystem — mechanism for the Brood module system + project test runner
    primitives.def(
        "file/cwd",
        Arity::exact(0),
        Sig::nullary(string),
        &[],
        "The current working directory.\n\n    (file/cwd)   ;~> \"/home/me/project\"",
        cwd,
    );
    // Where this binary lives — for finding what was installed beside it (a shipped app
    // cannot trust PATH; a desktop launch's PATH often lacks ~/.local/bin).
    primitives.def(
        "%exe-path",
        Arity::exact(0),
        Sig::nullary(string.union(nil_ty)),
        &[],
        "The absolute path of the running executable, or nil when the platform won't say (a sandbox with no /proc/self/exe equivalent). For locating something installed ALONGSIDE this binary: a shipped app cannot assume PATH — a desktop launch inherits the session's, which routinely lacks ~/.local/bin — so \"the runtime that installed me is my sibling\" is the reliable lookup. Nil rather than an error, because asking where you live is opportunistic.",
        exe_path);
    primitives.def(
        "file/exists?",
        Arity::exact(1),
        Sig::new(vec![string], bool_ty),
        &["path"],
        "Whether path exists.\n\n    (file/exists? \"/nonexistent-brood-xyz\")   ;=> false",
        watched!("file/exists?", file_exists, 1),
    );
    primitives.def(
        "%canonicalize",
        Arity::exact(1),
        Sig::new(vec![string], string.union(nil_ty)),
        &["path"],
        "The real absolute path of `path` with symlinks and ./.. resolved. Works for a not-yet-existing target (the longest existing ancestor is resolved, then the remaining components appended). Relative paths are taken against the cwd. nil only if the cwd itself can't be read. Use it to make path sandboxing symlink-escape-proof.",
        watched!("%canonicalize", path_canonicalize, 1));
    primitives.def(
        "file/dir?",
        Arity::exact(1),
        Sig::new(vec![string], bool_ty),
        &["path"],
        "Whether path is a directory.\n\n    (file/dir? \"/tmp\")   ;=> true",
        watched!("file/dir?", is_dir, 1),
    );
    primitives.def(
        "file/ls",
        Arity::exact(1),
        Sig::new(vec![string], Ty::list_of(string)),
        &["path"],
        "The entry names directly under directory path, sorted.\n\n    (file/ls \"src\")   ;~> (\"main.blsp\" \"util.blsp\")",
        watched!("file/ls", list_dir, 1),
    );
    primitives.def(
        "file/mkdir",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        &["path"],
        "Create a directory and any missing parents (like mkdir -p).\n\n    (file/mkdir \"build/out\")   ;~> nil, with build/ and build/out/ created",
        watched!("file/mkdir", make_dir, 1),
    );
    primitives.def(
        "file/mkdir-private",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        &["path"],
        "Create a directory and any missing parents (like mkdir -p), the directory itself owner-only whether it was made now or was already there — where an app keeps what other users must not read (an editor's sidecars of files edited as root, ADR-588), as file/spit-private is for a file. On Unix that is mode 0700, and no one else can reach anything inside, whatever its own mode. On Windows it is NOT the same guard: the directory gets a protected one-entry DACL (the current user) that what is made inside afterwards inherits, but every user holds the bypass-traverse-checking right, so a directory's DACL does not stop a file inside being opened by its path — a file that keeps entries of its own (made there before, or moved in) stays as open as they say. Write each secret with file/spit-private, which makes the file itself owner-only.\n\n    (file/mkdir-private \"state/auto-save\")   ;~> nil, state/auto-save made owner-only (mode 0700 on Unix)",
        watched!("file/mkdir-private", make_private_dir, 1),
    );
    primitives.def(
        "file/spit",
        Arity::exact(2),
        Sig::new(vec![string, iolist], nil_ty),
        &["path", "s"],
        "Write s (any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139)) to the file at path, replacing any existing file.\n\n    (file/spit \"notes.txt\" \"hello\")   ;~> nil",
        watched!("file/spit", spit, 1));
    primitives.def(
        "file/replace",
        Arity::exact(2),
        Sig::new(vec![string, iolist], nil_ty),
        &["path", "s"],
        "Replace the file at path with s (any iolist) so that a crash, a power cut or a full disk leaves the old file or the whole new one, never a torn file: s goes to a sibling temp file, is flushed to the disk, and is renamed over path. It keeps what the rename would lose: the file's mode, owner and group; a symlink stays a symlink, its TARGET replaced. It writes in place, as file/spit does and with the same risk, when a rename cannot keep the file as it was: a file with other hard links, one this user cannot give the same owner, extended attributes, a device, or a directory that takes no new entries. A full disk raises and leaves the old file. On Windows the temp file takes the old one's place by ReplaceFileW, which keeps its ACL and attributes; a file another program holds open is waited on for about a second, then refused with that reason, the file unchanged (ADR-671). For the file a person keeps (an editor's save, a tool rewriting source); file/spit-atomic is for a file other programs read while it changes.\n\n    (file/replace \"notes.txt\" \"text\\n\")   ;~> nil, notes.txt holds the text, its mode kept",
        watched!("file/replace", replace, 1));
    primitives.def(
        "file/spit-append",
        Arity::exact(2),
        Sig::new(vec![string, iolist], nil_ty),
        &["path", "s"],
        "Append s (any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139)) to the file at path, creating it if absent (unlike file/spit, which truncates). Returns nil. Opens in append mode so each write lands at end-of-file — the OS-atomic append that makes a log safe to write from several processes at once. The string sibling of append-bytes.\n\n    (file/spit-append \"app.log\" [\"started\" 10])   ;~> nil, the line appended",
        watched!("file/spit-append", spit_append, 1));
    // Atomic compare-and-swap of a file's whole contents, serialised across
    // processes. The mechanism a safe read-modify-write needs when the "modify" is
    // Brood code (`nest add` editing project.blsp) — see `io::file_swap`.
    primitives.def(
        "%file-swap",
        Arity::exact(4),
        Sig::new(vec![string, string, string, string], bool_ty),
        &["lock-path", "data-path", "expected", "new"],
        "Replace the entire contents of data-path with new, but ONLY if they currently equal expected; returns true when swapped, false when they differ (re-read, recompute, retry). Serialised across processes by a blocking exclusive lock on lock-path (a separate file — the data file is replaced by rename, so a lock on it would exclude nobody), and crash-atomic (temp file + rename, so a crash leaves the old contents intact). A missing data-path reads as \"\", so the same call creates it. The mechanism behind a safe read-modify-write whose modify step is Brood code.",
        watched!("%file-swap", file_swap, 1));
    primitives.def(
        "file/slurp",
        Arity::exact(1),
        Sig::new(vec![string], string),
        &["path"],
        "Read the whole file at path into a string (does not evaluate it). UTF-8; throws on a non-text file — use file/slurp-bytes for binary.\n\n    (file/slurp \"notes.txt\")   ;~> \"hello\"",
        watched!("file/slurp", slurp, 1));
    primitives.def(
        "file/slurp-bytes",
        Arity::exact(1),
        Sig::new(vec![string], bytes_ty),
        &["path"],
        "Read the whole file at path as a bytes value. The byte-faithful read file/slurp can't be (file/slurp is UTF-8 and throws on a non-text file). Pairs with hash/sha256-bytes / hash/sha256-raw and the encoding byte variants — e.g. hashing a binary asset.\n\n    (file/slurp-bytes \"logo.png\")   ;~> the file's bytes",
        watched!("file/slurp-bytes", slurp_bytes, 1));
    primitives.def(
        "file/spit-bytes",
        Arity::exact(2),
        Sig::new(vec![string, any], nil_ty),
        &["path", "bytes"],
        "Write any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139) to path byte-faithfully, replacing any existing file. Returns nil. The binary write-side counterpart to file/slurp-bytes (file/spit is UTF-8 string-only) — materialises a received image / archive / any binary asset to disk.\n\n    (file/spit-bytes \"copy.png\" (file/slurp-bytes \"logo.png\"))   ;~> nil",
        watched!("file/spit-bytes", spit_bytes, 1));
    primitives.def(
        "file/spit-bytes-append",
        Arity::exact(2),
        Sig::new(vec![string, any], nil_ty),
        &["path", "bytes"],
        "Append any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139) — to the file at path, byte-faithfully, creating it if absent. Returns nil. The append counterpart of file/spit-bytes (which truncates), as file/spit-append is to file/spit: lets a large payload be streamed to disk chunk-by-chunk (e.g. spooling a file upload) without ever holding it whole in memory.\n\n    (file/spit-bytes-append \"upload.bin\" chunk)   ;~> nil, chunk appended",
        watched!("file/spit-bytes-append", append_bytes, 1));
    primitives.def(
        "file/mtime",
        Arity::exact(1),
        Sig::new(vec![string], int.union(nil_ty)),
        &["path"],
        "Last-modified time of path as epoch-milliseconds, or nil if the file is missing. Cheap (stat) — pair with `load` to drive a hot-reloader.\n\n    (file/mtime \"project.blsp\")   ;~> 1761213600000, or nil when missing",
        watched!("file/mtime", file_mtime, 1));
    primitives.def(
        "file/set-mtime",
        Arity::exact(2),
        Sig::new(vec![string, int], nil_ty),
        &["path", "ms"],
        "Set the last-modified time of the file at path to ms epoch-milliseconds — what `file/mtime` then answers. Returns nil; throws if the file is missing or cannot be changed. The portable `touch -t`: a test that needs a file older than another (an auto-save sidecar, a stale cache) on every OS, with no `touch` program.\n\n    (file/set-mtime \"notes.txt\" 1577836800000)   ;~> nil, notes.txt now dated 2020-01-01",
        watched!("file/set-mtime", set_file_mtime, 1));
    primitives.def(
        "file/size",
        Arity::exact(1),
        Sig::new(vec![string], int.union(nil_ty)),
        &["path"],
        "Size of the file at path in bytes, or nil if it is missing.\n\n    (file/size \"notes.txt\")   ;~> 5, or nil when missing",
        watched!("file/size", file_size, 1),
    );
    primitives.def(
        "file/stat",
        Arity::exact(1),
        Sig::new(vec![string], map_ty.union(nil_ty)),
        &["path"],
        "Metadata for path in ONE stat as a map {:dir? :size :mtime :atime :symlink? :exec? :mode :nlink :uid :gid :owner :group}, or nil if missing. :symlink? reads the link itself (lstat); the rest follow it. :mtime/:atime are epoch-ms last-modified/last-access (nil if unreadable; :atime may be coarse under relatime/noatime mounts); :exec? is the owner-execute bit; :mode is the unix permission bits (0 off-unix); :nlink the hard-link count; :uid/:gid the numeric ids; :owner/:group their resolved names (the numeric id as a string if unresolved). Everything an `ls -l` row + a recency sort needs in one syscall.\n\n    (file/stat \"notes.txt\")   ;~> {:dir? false, :size 5, :mtime 1761213600000, …}",
        watched!("file/stat", file_stat, 1));
    primitives.def(
        "file/read-link",
        Arity::exact(1),
        Sig::new(vec![string], string.union(nil_ty)),
        &["path"],
        "The target the symlink at path stores, as written when it was made — not resolved, so a relative target stays relative and a dangling link still answers. nil when path is not a symlink or is missing, as file/stat answers nil for a missing path. What `ls -l` prints after `->`.\n\n    (file/read-link \"/nonexistent-brood-xyz\")   ;=> nil",
        watched!("file/read-link", read_link, 1));
    primitives.def(
        "file/symlink",
        Arity::range(2, 3),
        Sig::with_optional(vec![string, string], vec![map_ty.union(nil_ty)], nil_ty.union(Ty::of(Tag::Failure))),
        &["target", "path", "opts"],
        "Make path a symlink to target, stored as given (a relative target is read from the link's own directory, as `ln -s` makes it). Returns nil, or a failure saying why: path already exists (a dangling link too), the directory refuses it, or — on Windows — this account may not make symlinks. With {:replace true} an existing path is replaced ATOMICALLY: the link is made under a temp name beside it and renamed over, so a reader sees the old entry or the new link, never neither; a real directory at path is refused, not replaced. On Windows a target that is a directory makes a directory link (symlink_dir), anything else, a missing target included, a file link (symlink_file); making either needs Developer Mode or the SeCreateSymbolicLinkPrivilege, and the failure says so (ADR-697).\n\n    (failure? (file/symlink \"x\" \"/nonexistent-brood-xyz/link\"))   ;=> true",
        watched!("file/symlink", make_symlink, 2));
    primitives.def(
        "file/chmod",
        Arity::exact(2),
        Sig::new(vec![string, int], nil_ty.union(Ty::of(Tag::Failure))),
        &["path", "mode"],
        "Set the permission bits of path to mode (an int, `0o755`; the low 12 bits, setuid/setgid/sticky included), following a symlink as chmod(1) does. Returns nil, or a failure saying why (missing, not this user's file). What file/stat's :mode and :exec? read back. On Windows a file has no permission bits, only a read-only attribute: it is set when mode lacks the owner-write bit (0o200) and cleared when mode has it; the rest of mode is ignored, and file/stat's :mode stays 0 there (ADR-697).\n\n    (failure? (file/chmod \"/nonexistent-brood-xyz\" 0o644))   ;=> true",
        watched!("file/chmod", change_mode, 1));
    primitives.def(
        "file/lock",
        Arity::exact(1),
        Sig::new(vec![string], bool_ty.union(Ty::of(Tag::Failure))),
        &["path"],
        "Take an exclusive advisory lock on the file at path (created when missing) and hold it for as long as this program runs, or until file/unlock: true once it is held (already holding it is true too), or a failure saying why — another program holds it, or the file cannot be opened. The operating system drops the lock when the program exits, however it exits, so a lock still held means the program that took it is still running: what a program announcing itself in a shared directory locks, and what file/locked? asks others about. Advisory: it stops another file/lock, not a read or a write.\n\n    (file/lock (path/join (file/cwd) \"target/brood-lock-doc\"))   ;~> true",
        watched!("file/lock", lock_file, 1));
    primitives.def(
        "file/unlock",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        &["path"],
        "Release the lock file/lock took on path, so another program may take it. nil, held or not.\n\n    (file/unlock \"/nonexistent-brood-xyz\")   ;=> nil",
        watched!("file/unlock", unlock_file, 1));
    primitives.def(
        "file/locked?",
        Arity::exact(1),
        Sig::new(vec![string], bool_ty),
        &["path"],
        "Whether some program — this one included — holds a file/lock on path: false when no program does, or the file is missing. Asking holds a shared lock on the file for an instant, so a file/lock racing the question may be refused once.\n\n    (file/locked? \"/nonexistent-brood-xyz\")   ;=> false",
        watched!("file/locked?", file_locked, 1));
    primitives.def(
        "file/rm",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        &["path"],
        "Remove the file at path. Idempotent (nil if already absent); errors on a real I/O failure.\n\n    (file/rm \"notes.txt\")   ;~> nil",
        watched!("file/rm", delete_file, 1));
    primitives.def(
        "file/rmdir",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        &["path"],
        "Remove a directory and everything under it (recursive). Idempotent (nil if already absent); errors on a real I/O failure.\n\n    (file/rmdir \"build\")   ;~> nil",
        watched!("file/rmdir", delete_dir, 1));
    primitives.def(
        "file/rename",
        Arity::exact(2),
        Sig::new(vec![string, string], nil_ty),
        &["from", "to"],
        "Rename/move file `from` to `to`. Returns nil; errors on failure.\n\n    (file/rename \"draft.txt\" \"final.txt\")   ;~> nil",
        watched!("file/rename", rename_file, 2),
    );
    // The recycle bin (Windows): the one trash `file/trash` cannot reach from Brood, since
    // it is the shell's, not a directory. Everywhere else `file/trash` moves the file itself.
    primitives.def(
        "%file-recycle",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        &["path"],
        "Move the file or directory at absolute path into the Windows recycle bin (the shell's own operation, so Explorer can restore it), silently: no confirmation, no progress, no error dialog. Returns nil; errors on failure, and on every other system, which has no recycle bin — `file/trash` is the portable call.\n\n    (%file-recycle \"C:\\\\Users\\\\me\\\\old.txt\")   ;~> nil on Windows",
        watched!("%file-recycle", recycle_path, 1),
    );
    primitives.def(
        "file/cp",
        Arity::exact(2),
        Sig::new(vec![string, string], nil_ty),
        &["from", "to"],
        "Copy file `from` to `to` (replacing `to`), preserving contents and permissions. Binary-safe (unlike slurp+spit). Returns nil; errors on failure.\n\n    (file/cp \"logo.png\" \"logo-copy.png\")   ;~> nil",
        watched!("file/cp", copy_file, 2));
}

// ---------- filesystem ----------
// Mechanism only: existence / directory reflection so the Brood module system and
// the project test runner can resolve load paths and discover test files. Path
// manipulation and all policy live in Brood (`std/prelude.blsp`, `std/tool/project.blsp`).

/// `(file/cwd)` — the process's current working directory as a string.
/// `(%exe-path)` — the absolute path of the RUNNING executable, or nil when the platform
/// won't say (a sandbox with no `/proc/self/exe`-equivalent). Nil rather than an error: a
/// program asking where it lives is asking opportunistically, and the answer is allowed to
/// be "cannot tell".
///
/// What it is for: locating something installed ALONGSIDE this binary. A shipped app cannot
/// assume `PATH` — a desktop launch inherits the session's, which routinely lacks
/// `~/.local/bin` — so "the runtime that installed me is my sibling" is the reliable lookup,
/// and it needs this. (myedit's eval sandbox spawns a Brood runtime for its child; from a
/// dash-launched editor, PATH alone finds nothing.)
///
/// Linux's `" (deleted)"` marker is stripped — see [`strip_deleted_marker`]; without that
/// the answer stops being usable the moment the binary is upgraded in place.
pub(super) fn exe_path(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match std::env::current_exe() {
        Ok(p) => Ok(heap.alloc_string(strip_deleted_marker(&host_path_text(&p)))),
        Err(_) => Ok(Value::nil()),
    }
}

/// A path the host handed back (the cwd, the executable, a canonical path), as the text
/// Brood's `path` module reads (ADR-592). On Windows that is `/` separators — Win32 takes
/// them, and `path` splits on nothing else — and no `\\?\` verbatim prefix, which
/// `fs::canonicalize` adds and most programs a path is handed to refuse. Elsewhere the
/// path as it is: a `\` there is an ordinary character of a file name.
pub(super) fn host_path_text(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        windows_path_text(&text)
    } else {
        text.into_owned()
    }
}

/// [`host_path_text`]'s Windows rule, over text so it is tested on every host.
fn windows_path_text(text: &str) -> String {
    let unprefixed = if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{share}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(text).to_string()
    };
    unprefixed.replace('\\', "/")
}

/// Strip Linux's `" (deleted)"` marker from a `/proc/self/exe` readlink.
///
/// The kernel appends it when the running binary's inode has been UNLINKED — which is not
/// an exotic state, it is what every in-place upgrade does: you cannot write over a busy
/// executable (`ETXTBSY`), so `cargo`, `make install`, `cp` and every package manager
/// unlink-and-rename instead. From that moment `current_exe()` returns
/// `/usr/local/bin/brood (deleted)`, a string that names no file — so `(file/exists? …)` on
/// it is false and the sibling lookup this primitive exists for silently stops working,
/// with no error anyone can see.
///
/// Stripping is not merely cosmetic, it is the MORE correct answer: after an upgrade the
/// replacement binary sits at exactly that path, so the un-suffixed string is the live
/// install, while the suffixed one is a description of a deleted inode. If nothing is there
/// any more the caller gets a path that does not exist, which is the honest result and the
/// one `file/exists?` can act on.
///
/// Found via `introspection_test.blsp`'s `os/exe-path` case failing three consecutive
/// full-suite runs and never solo — the runs that followed a `cargo build` (KI-130).
fn strip_deleted_marker(path: &str) -> &str {
    path.strip_suffix(" (deleted)").unwrap_or(path)
}

pub(super) fn cwd(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match std::env::current_dir() {
        Ok(p) => Ok(heap.alloc_string(&host_path_text(&p))),
        Err(e) => Err(LispError::runtime(format!("file/cwd: {}", e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

/// `(file/exists? path)` — true if a file or directory exists at `path`.
pub(super) fn file_exists(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/exists?", arg(args, 0))?;
    Ok(Value::boolean(std::path::Path::new(&path).exists()))
}

/// `(%canonicalize path)` — the real absolute path of `path` with **symlinks and
/// `.`/`..` fully resolved**. Works for a not-yet-existing target: the longest
/// existing prefix is `fs::canonicalize`d (which resolves every symlink in it,
/// the POSIX-correct way — a `..` after a symlink resolves against the symlink's
/// target, not lexically), then the non-existent tail (which has no symlinks) is
/// resolved lexically against that canonical prefix (`..` pops, `.` drops). So
/// the result never contains a `..`/`.` and is safe for a plain `starts_with`
/// sandbox check. Relative paths are taken against the cwd. Returns nil only if
/// the cwd can't be read. Backs symlink-escape-proof path sandboxing
/// (`std/tool/mcp.blsp`).
pub(super) fn path_canonicalize(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::path::{Component, Path, PathBuf};
    let path = expect_string(heap, "os/canonicalize", arg(args, 0))?;
    let raw = Path::new(&path);
    let abs = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(raw),
            Err(_) => return Ok(Value::nil()),
        }
    };
    // Apply the (symlink-free) non-existent tail components to `base` with real
    // `..`/`.` semantics — the tail can't contain symlinks (it doesn't exist),
    // so lexical resolution against the canonical `base` is correct.
    let apply_tail = |mut base: PathBuf, tail: &[Component]| -> PathBuf {
        for c in tail {
            match c {
                Component::ParentDir => {
                    base.pop();
                }
                Component::CurDir => {}
                other => base.push(other.as_os_str()),
            }
        }
        base
    };
    // Find the longest existing PREFIX (by component count) and canonicalize it;
    // fs::canonicalize needs the whole path to exist, so shrink until it does.
    // For an absolute path this always succeeds by k=1 (the root). Rebuilding the
    // prefix each step is O(n²) in components, but paths are short.
    let comps: Vec<Component> = abs.components().collect();
    for k in (1..=comps.len()).rev() {
        let mut prefix = PathBuf::new();
        for c in &comps[..k] {
            prefix.push(c.as_os_str());
        }
        if let Ok(real) = std::fs::canonicalize(&prefix) {
            let out = apply_tail(real, &comps[k..]);
            return Ok(heap.alloc_string(&host_path_text(&out)));
        }
    }
    // Nothing (not even the root) canonicalized — a broken mount. Fall back to a
    // purely lexical normalization so callers still get a stable, `..`-free path.
    let out = apply_tail(PathBuf::new(), &comps);
    Ok(heap.alloc_string(&host_path_text(&out)))
}

/// `(file/dir? path)` — true if `path` exists and is a directory.
pub(super) fn is_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/dir?", arg(args, 0))?;
    Ok(Value::boolean(std::path::Path::new(&path).is_dir()))
}

/// `(file/ls path)` — the entry names (not full paths) directly under a
/// directory, sorted for determinism. Errors if `path` isn't a readable directory.
pub(super) fn list_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/ls", arg(args, 0))?;
    let mut names: Vec<String> = match std::fs::read_dir(&path) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(e) => {
            return Err(LispError::runtime(format!("file/ls: {}: {}", path, e))
                .with_code(crate::error::error_codes::FILE_IO));
        }
    };
    names.sort();
    let mut items = Vec::with_capacity(names.len());
    for n in &names {
        items.push(heap.alloc_string(n));
    }
    Ok(heap.list(items))
}

/// `(file/mkdir path)` — create `path` and any missing parents (like `mkdir -p`).
/// Returns nil. Used by the project scaffolder (`nest new`).
pub(super) fn make_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/mkdir", arg(args, 0))?;
    std::fs::create_dir_all(&path).map_err(|e| {
        LispError::runtime(format!("file/mkdir: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(file/mkdir-private path)` — `file/mkdir`, then the directory made owner-only: 0700 on
/// Unix; on Windows a protected one-entry DACL that what is made inside inherits (KI-365) —
/// not 0700's guard there, since bypass traverse checking lets a file inside be opened by
/// its path whatever the directory says. Returns nil.
pub(super) fn make_private_dir(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/mkdir-private", arg(args, 0))?;
    make_dir(args, env, heap)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
            LispError::runtime(format!("file/mkdir-private: {}: {}", path, e))
                .with_code(crate::error::error_codes::FILE_IO)
        })?;
    }
    #[cfg(windows)]
    crate::host::owner_only::restrict_to_owner(std::path::Path::new(&path), true).map_err(|e| {
        LispError::runtime(format!("file/mkdir-private: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    #[cfg(not(any(unix, windows)))]
    let _ = path;
    Ok(Value::nil())
}

/// `(file/spit path content)` — write `content` (a string) to `path`, replacing any
/// existing file. Returns nil. The write-side counterpart to `load`.
pub(super) fn spit(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/spit", arg(args, 0))?;
    // Content is any iolist (ADR-139): describe the file as a tree of
    // strings/bytes and it is flattened exactly once, here at the write.
    let mut content = Vec::new();
    flatten_iolist(heap, "file/spit", arg(args, 1), &mut content)?;
    std::fs::write(&path, content).map_err(|e| {
        LispError::runtime(format!("file/spit: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(file/replace path content)` — see the docstring where it is defined, and ADR-621.
pub(super) fn replace(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/replace", arg(args, 0))?;
    let mut content = Vec::new();
    flatten_iolist(heap, "file/replace", arg(args, 1), &mut content)?;
    replace_file(std::path::Path::new(&path), &content, |file, bytes| {
        std::io::Write::write_all(file, bytes)
    })
    .map_err(|e| {
        LispError::runtime(format!("file/replace: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// The file `path` really names: a symlink's target, since the link is what must survive.
/// A path that does not exist yet is itself. A dangling link is the one `None`: only
/// writing through it (as `open` does) creates what it points at.
#[cfg(unix)]
fn replace_target(path: &std::path::Path) -> Option<std::path::PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(target) => Some(target),
        Err(_) if std::fs::symlink_metadata(path).is_ok() => None,
        Err(_) => Some(path.to_path_buf()),
    }
}

/// Whether the file has an extended attribute a sibling temp file would not get. A
/// `security.selinux` label is not one: the directory gives the new file the same label,
/// and counting it would make every save on such a system non-atomic.
#[cfg(target_os = "linux")]
fn has_extended_attributes(path: &std::path::Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(name) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return true;
    };
    // SAFETY: `name` is NUL-terminated and live; a null buffer of size 0 only asks for the
    // size the list needs.
    let size = unsafe { libc::listxattr(name.as_ptr(), std::ptr::null_mut(), 0) };
    if size <= 0 {
        return false;
    }
    let mut names = vec![0_u8; size as usize];
    // SAFETY: `name` is NUL-terminated; `names` is a live, writable buffer of exactly
    // `names.len()` bytes (a list grown since the first call fails with ERANGE, not overflows).
    let read = unsafe { libc::listxattr(name.as_ptr(), names.as_mut_ptr().cast(), names.len()) };
    if read <= 0 {
        return true;
    }
    names[..read as usize]
        .split(|byte| *byte == 0)
        .any(|attribute| !attribute.is_empty() && attribute != b"security.selinux")
}

#[cfg(all(unix, not(target_os = "linux")))]
fn has_extended_attributes(_: &std::path::Path) -> bool {
    false
}

/// A new file beside `target`, its name fixed-length so a name at the filesystem's limit
/// still has a sibling. Owner-only while it waits for an old file's mode (`mode` 0600); a new
/// file's is `open`'s own, 0666 under the umask.
#[cfg(unix)]
fn replace_temp(
    target: &std::path::Path,
    mode: u32,
) -> std::io::Result<(std::fs::File, std::path::PathBuf)> {
    use std::os::unix::fs::OpenOptionsExt;
    replace_temp_with(target, |temp| {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(temp)
    })
}

/// A new file beside `target`, its name fixed-length. It takes the directory's inherited
/// ACL, as any new file there would; `ReplaceFileW` then gives it the old file's.
#[cfg(windows)]
fn replace_temp(target: &std::path::Path) -> std::io::Result<(std::fs::File, std::path::PathBuf)> {
    replace_temp_with(target, |temp| {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temp)
    })
}

/// The name a temp file beside `target` gets, tried until `open` makes one that was not
/// there: `.brood-replace-PID-N`. A crash before the replace leaves one behind.
#[cfg(any(unix, windows))]
fn replace_temp_with(
    target: &std::path::Path,
    open: impl Fn(&std::path::Path) -> std::io::Result<std::fs::File>,
) -> std::io::Result<(std::fs::File, std::path::PathBuf)> {
    let directory = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let mut last_error = std::io::ErrorKind::AlreadyExists.into();
    for attempt in 0..64 {
        let temp = directory.join(format!(
            ".brood-replace-{}-{}",
            std::process::id(),
            stamp.wrapping_add(attempt)
        ));
        match open(&temp) {
            Ok(file) => return Ok((file, temp)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => last_error = error,
            Err(error) => return Err(error),
        }
    }
    Err(last_error)
}

/// A directory that takes no new entries is why a temp file could not be made and the one
/// reason to go on in place. Anything else, a full disk above all, would meet the same
/// failure there, after the truncate has destroyed the only copy.
#[cfg(any(unix, windows))]
fn directory_refuses_entries(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
    )
}

/// Give `file` the owner, group and mode of `old`: chown first, since it clears the
/// setuid bits a chmod sets. False when this user may not, which only a write in place
/// can respect.
#[cfg(unix)]
fn replace_keep_identity(file: &std::fs::File, old: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, fchown};
    fchown(file, Some(old.uid()), Some(old.gid())).is_ok()
        && file
            .set_permissions(std::fs::Permissions::from_mode(old.permissions().mode()))
            .is_ok()
}

/// What [`replace`] does. `write` puts the bytes in the temp file; a test hands in one that
/// fails, as a full disk does.
#[cfg(unix)]
fn replace_file(
    path: &std::path::Path,
    content: &[u8],
    write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let Some(target) = replace_target(path) else {
        return std::fs::write(path, content);
    };
    let old = std::fs::metadata(&target).ok();
    let in_place = old.as_ref().is_some_and(|meta| {
        !meta.is_file() || meta.nlink() > 1 || has_extended_attributes(&target)
    });
    if in_place {
        return std::fs::write(&target, content);
    }
    // a rename needs only the directory's permission: a file this user may not write stays
    // refused, as a write to it would be
    if old.is_some() {
        std::fs::OpenOptions::new().write(true).open(&target)?;
    }
    let mode = if old.is_some() { 0o600 } else { 0o666 };
    let (mut file, temp) = match replace_temp(&target, mode) {
        Ok(temp) => temp,
        Err(error) if directory_refuses_entries(&error) => {
            return std::fs::write(&target, content);
        }
        Err(error) => return Err(error),
    };
    if let Some(meta) = &old
        && !replace_keep_identity(&file, meta)
    {
        let _ = std::fs::remove_file(&temp);
        return std::fs::write(&target, content);
    }
    // A failure here (a full disk) must leave the old file: no fallback to a write in place.
    let written = write(&mut file, content).and_then(|()| file.sync_all());
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    drop(file);
    if std::fs::rename(&temp, &target).is_err() {
        // a bind-mounted file cannot be renamed over; the bytes are safe to write now
        let _ = std::fs::remove_file(&temp);
        return std::fs::write(&target, content);
    }
    // the rename is not durable until the directory is on disk; a filesystem that cannot
    // sync a directory still did the rename
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        let _ = std::fs::File::open(parent).and_then(|directory| directory.sync_all());
    }
    Ok(())
}

/// How long a Windows replace waits for another program to let go of the file, between
/// tries: about a second in all. A virus scanner, an indexer or a backup tool holds a file
/// just written for a moment; a program that keeps it open is told so instead.
#[cfg(windows)]
const REPLACE_RETRY_DELAYS_MS: [u64; 8] = [10, 20, 40, 80, 160, 250, 250, 250];

#[cfg(windows)]
fn wide_path(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(windows)]
fn win32_code(error: &std::io::Error) -> Option<u32> {
    error.raw_os_error().map(|code| code as u32)
}

/// Another program has the file open in a way that refuses this step. `replace_step` is
/// whether the step is the replace itself, where `ERROR_ACCESS_DENIED` is also what a file
/// open without `FILE_SHARE_DELETE`, or one pending deletion, answers.
#[cfg(windows)]
fn held_by_another_program(error: &std::io::Error, replace_step: bool) -> bool {
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION,
        ERROR_UNABLE_TO_MOVE_REPLACEMENT, ERROR_UNABLE_TO_REMOVE_REPLACED,
    };
    match win32_code(error) {
        Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION) => true,
        Some(
            ERROR_UNABLE_TO_REMOVE_REPLACED
            | ERROR_UNABLE_TO_MOVE_REPLACEMENT
            | ERROR_ACCESS_DENIED,
        ) => replace_step,
        _ => false,
    }
}

/// `step` tried again while another program holds the file, for about a second; the last
/// answer when it never lets go.
#[cfg(windows)]
fn with_sharing_retry<T>(
    replace_step: bool,
    mut step: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    let mut outcome = step();
    for delay in REPLACE_RETRY_DELAYS_MS {
        match &outcome {
            Err(error) if held_by_another_program(error, replace_step) => {
                std::thread::sleep(std::time::Duration::from_millis(delay));
                outcome = step();
            }
            _ => break,
        }
    }
    outcome
}

/// The error a replace that another program never allowed ends with: the reason, and that
/// the file is as it was.
#[cfg(windows)]
fn held_open_error(error: std::io::Error) -> std::io::Error {
    let waited: u64 = REPLACE_RETRY_DELAYS_MS.iter().sum();
    std::io::Error::new(
        error.kind(),
        format!(
            "another program has the file open and will not let it be replaced \
             (waited {} ms); the file is unchanged: {}",
            waited, error
        ),
    )
}

/// How many names the file has (hard links), through the handle the write check opened;
/// one when Windows does not say.
#[cfg(windows)]
fn replace_link_count(file: &std::fs::File) -> u32 {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    // SAFETY: BY_HANDLE_FILE_INFORMATION is plain data, so all-zero is a valid value.
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is `file`'s, open for the call; `information` is live and writable.
    let answered = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) };
    if answered == 0 {
        1
    } else {
        information.nNumberOfLinks
    }
}

/// A write in place: the file truncated and written, as `file/spit` does. An existing file
/// is opened as it is (`TRUNCATE_EXISTING`), so a hidden or system file, which
/// `CREATE_ALWAYS` refuses, is written too. Another program holding the file makes the open
/// fail before anything is truncated.
#[cfg(windows)]
fn replace_in_place(path: &std::path::Path, content: &[u8]) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path).is_err() {
        return std::fs::write(path, content);
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)?;
    std::io::Write::write_all(&mut file, content)
}

/// `MoveFileExW`: the temp file renamed over `target`, the rename on the disk before it
/// returns.
#[cfg(windows)]
fn replace_by_move(temp: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let from = wide_path(temp);
    let to = wide_path(target);
    // SAFETY: both paths are NUL-terminated wide strings (`wide_path`), live for the call.
    let moved = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// `ReplaceFileW`: the temp file takes `target`'s place and keeps what the old file had —
/// its ACL, attributes, creation time, short name and alternate streams.
#[cfg(windows)]
fn replace_by_replace_file(
    temp: &std::path::Path,
    target: &std::path::Path,
) -> std::io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW};
    let replaced = wide_path(target);
    let replacement = wide_path(temp);
    // SAFETY: both paths are NUL-terminated wide strings (`wide_path`), live for the call;
    // the backup name and the two reserved pointers are null, as allowed.
    let done = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_IGNORE_MERGE_ERRORS,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if done == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// A filesystem with no `ReplaceFileW` (FAT, some network shares) answers one of these.
#[cfg(windows)]
fn replace_file_unsupported(error: &std::io::Error) -> bool {
    use windows_sys::Win32::Foundation::{
        ERROR_CALL_NOT_IMPLEMENTED, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER,
        ERROR_NOT_SUPPORTED,
    };
    matches!(
        win32_code(error),
        Some(
            ERROR_INVALID_FUNCTION
                | ERROR_NOT_SUPPORTED
                | ERROR_INVALID_PARAMETER
                | ERROR_CALL_NOT_IMPLEMENTED
        )
    )
}

/// What [`replace`] does on Windows (ADR-671): a synced temp file beside the file, put in
/// its place by `ReplaceFileW`, which keeps the old file's ACL, attributes and creation
/// time; `MoveFileExW` for a file that is not there yet, or a filesystem without
/// `ReplaceFileW`. Another program holding the file is waited on for about a second, then
/// the replace fails with that reason, the old file and no temp file left.
#[cfg(windows)]
fn replace_file(
    path: &std::path::Path,
    content: &[u8],
    write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_UNABLE_TO_MOVE_REPLACEMENT_2};
    // a symlink stays one: its target is what gets replaced; a dangling one is written
    // through, which creates what it names
    let target = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => match std::fs::canonicalize(path) {
            Ok(target) => target,
            Err(_) => return std::fs::write(path, content),
        },
        _ => path.to_path_buf(),
    };
    let old = std::fs::metadata(&target).ok();
    if old.as_ref().is_some_and(|meta| !meta.is_file()) {
        return replace_in_place(&target, content);
    }
    if old.is_some() {
        // a replace needs only DELETE on the file and the directory's permission: a file this
        // user may not write (read-only, or its ACL) stays refused, as a write to it would be
        let check = with_sharing_retry(false, || {
            std::fs::OpenOptions::new().write(true).open(&target)
        })
        .map_err(|error| {
            if held_by_another_program(&error, false) {
                held_open_error(error)
            } else {
                error
            }
        })?;
        // a file with other hard links would lose them to a replace: they keep the old text
        if replace_link_count(&check) > 1 {
            drop(check);
            return replace_in_place(&target, content);
        }
    }
    let (mut file, temp) = match replace_temp(&target) {
        Ok(temp) => temp,
        Err(error) if directory_refuses_entries(&error) => {
            return replace_in_place(&target, content);
        }
        Err(error) => return Err(error),
    };
    // A failure here (a full disk) must leave the old file: no fallback to a write in place.
    // `sync_all` is FlushFileBuffers.
    let written = write(&mut file, content).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    let outcome = if old.is_none() {
        with_sharing_retry(true, || replace_by_move(&temp, &target))
    } else {
        match with_sharing_retry(true, || replace_by_replace_file(&temp, &target)) {
            Err(error) if replace_file_unsupported(&error) => {
                with_sharing_retry(true, || replace_by_move(&temp, &target))
            }
            Err(error) if win32_code(&error) == Some(ERROR_UNABLE_TO_MOVE_REPLACEMENT_2) => {
                // the old file was moved aside and the new one could not take its name: the
                // temp file is the only whole copy, so it is finished by a move or kept
                return with_sharing_retry(true, || replace_by_move(&temp, &target)).map_err(
                    |error| {
                        std::io::Error::new(
                            error.kind(),
                            format!(
                                "the old file was moved aside and the new text could not take \
                                 its name; the new text is in {}: {}",
                                temp.display(),
                                error
                            ),
                        )
                    },
                );
            }
            other => other,
        }
    };
    let Err(error) = outcome else {
        return Ok(());
    };
    let _ = std::fs::remove_file(&temp);
    if old.is_some() && win32_code(&error) == Some(ERROR_ACCESS_DENIED) {
        // the write check passed, so this user may write the file but not replace it (its
        // ACL grants no DELETE): a write in place, as on Unix for a file whose owner cannot
        // be kept. Another program holding the file would fail that open, truncating nothing.
        return replace_in_place(&target, content);
    }
    if held_by_another_program(&error, true) {
        return Err(held_open_error(error));
    }
    Err(error)
}

#[cfg(not(any(unix, windows)))]
fn replace_file(
    path: &std::path::Path,
    content: &[u8],
    _: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    std::fs::write(path, content)
}

/// `(file/spit-append path content)` — append `content` (a string) to the file at
/// `path`, creating it if absent. Returns nil. Unlike `spit` (which truncates),
/// this opens in append mode, so each call's write lands at end-of-file — the
/// atomic-append the OS guarantees for an `O_APPEND` handle, which is what makes a
/// log file safe to write from several processes concurrently. The string sibling
/// of `append-bytes`.
pub(super) fn spit_append(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::io::Write;
    let path = expect_string(heap, "file/spit-append", arg(args, 0))?;
    // Content is any iolist (ADR-139) — one flatten, one O_APPEND write.
    let mut content = Vec::new();
    flatten_iolist(heap, "file/spit-append", arg(args, 1), &mut content)?;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .map_err(|e| {
            LispError::runtime(format!("file/spit-append: {}: {}", path, e))
                .with_code(crate::error::error_codes::FILE_IO)
        })?;
    f.write_all(&content).map_err(|e| {
        LispError::runtime(format!("file/spit-append: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(file/spit-bytes path bytes)` — write a byte sequence (a `bytes` value, a vector,
/// or a list of byte ints 0–255) to `path` byte-faithfully, replacing any
/// existing file. Returns nil. The binary write-side counterpart to `slurp-bytes`:
/// `spit` is UTF-8 string-only and would reject (or corrupt) raw bytes, so this is
/// what materialises a received image / archive / any binary asset to disk.
pub(super) fn spit_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/spit-bytes", arg(args, 0))?;
    // Any iolist (ADR-139) — a strict superset of the old bytes/vector/list-of-ints
    // surface (byte ints are iolist leaves), plus strings (UTF-8) and nesting.
    let mut bytes = Vec::new();
    flatten_iolist(heap, "file/spit-bytes", arg(args, 1), &mut bytes)?;
    std::fs::write(&path, &bytes).map_err(|e| {
        LispError::runtime(format!("file/spit-bytes: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

pub(super) fn append_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::io::Write;
    let path = expect_string(heap, "file/spit-bytes-append", arg(args, 0))?;
    // Any iolist (ADR-139) — see `spit_bytes`.
    let mut bytes = Vec::new();
    flatten_iolist(heap, "file/spit-bytes-append", arg(args, 1), &mut bytes)?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| {
            LispError::runtime(format!("file/spit-bytes-append: {}: {}", path, e))
                .with_code(crate::error::error_codes::FILE_IO)
        })?;
    f.write_all(&bytes).map_err(|e| {
        LispError::runtime(format!("file/spit-bytes-append: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(file/slurp path)` — read the whole file at `path` and return it as a string. The
/// read-side counterpart to `spit`; unlike `load` it does not evaluate, so the
/// doc tooling can inspect a module's source (e.g. its leading docstring form).
pub(super) fn slurp(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/slurp", arg(args, 0))?;
    let content = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("file/slurp: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(heap.alloc_string(&content))
}

/// `(file/slurp-bytes path)` — read the whole file at `path` as a bytes value. The
/// byte-faithful read `slurp` can't be: `slurp` is UTF-8 and throws
/// on a non-text file, whereas this reads any bytes (images, archives, a binary
/// asset to hash via `hash/sha256-bytes`). Pairs with `hash/sha256-bytes` /
/// `hash/sha256-raw` and the `encoding` byte variants.
pub(super) fn slurp_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/slurp-bytes", arg(args, 0))?;
    let bytes = std::fs::read(&path).map_err(|e| {
        LispError::runtime(format!("file/slurp-bytes: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(bytes_to_value(&bytes, heap))
}

/// `(file-size path)` — the size of `path` in bytes, or nil if it's missing.
/// GC-safe: the arg is copied to an owned `String` up front and the result is a
/// scalar — no `Value` handle is held across an allocation or eval (and a builtin
/// never fires GC mid-execution; see `docs/memory-model.md`).
pub(super) fn file_size(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/size", arg(args, 0))?;
    match std::fs::metadata(&path) {
        Ok(meta) => Ok(Value::int(meta.len() as i64)),
        Err(_) => Ok(Value::nil()),
    }
}

/// `(file/rm path)` — remove the file at `path`. Idempotent (nil if already
/// absent); errors on a real I/O failure (e.g. it's a directory, or permission).
pub(super) fn delete_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/rm", arg(args, 0))?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Value::nil()),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::nil()),
        Err(e) => Err(LispError::runtime(format!("file/rm: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

/// `(delete-dir path)` — remove a directory and everything under it. The
/// recursive sibling of `delete-file`; idempotent (nil if already absent),
/// errors on a real I/O failure. The mechanism behind test-fixture teardown.
pub(super) fn delete_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/rmdir", arg(args, 0))?;
    match std::fs::remove_dir_all(&path) {
        Ok(()) => Ok(Value::nil()),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::nil()),
        Err(e) => Err(LispError::runtime(format!("file/rmdir: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

/// `(rename-file from to)` — rename/move `from` to `to` (replacing `to` if it
/// exists, per the platform). Returns nil; errors on failure.
pub(super) fn rename_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let from = expect_string(heap, "file/rename", arg(args, 0))?;
    let to = expect_string(heap, "file/rename", arg(args, 1))?;
    std::fs::rename(&from, &to).map_err(|e| {
        LispError::runtime(format!("file/rename: {} -> {}: {}", from, to, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(%file-recycle path)` — move `path` into the recycle bin: `SHFileOperationW`'s delete
/// with `FOF_ALLOWUNDO`, the documented way to recycle (the shell restores it), with every
/// dialog off. `IFileOperation` is its successor but needs COM set up on the calling
/// thread; this needs nothing. Not Windows: an error, as there is no recycle bin.
pub(super) fn recycle_path(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%file-recycle", arg(args, 0))?;
    recycle(&path).map_err(|reason| {
        LispError::runtime(format!("%file-recycle: {}: {}", path, reason))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

#[cfg(windows)]
fn recycle(path: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };
    if !std::path::Path::new(path).is_absolute() {
        // the shell resolves a relative path against ITS idea of the directory
        return Err("not an absolute path".to_string());
    }
    // pFrom is a LIST of paths, each NUL-terminated, the list ended by a second NUL
    let from: Vec<u16> = path.encode_utf16().chain([0, 0]).collect();
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
        ..Default::default()
    };
    // SAFETY: `operation` is a valid SHFILEOPSTRUCTW whose `pFrom` is the double-NUL
    // terminated list above, alive for the call; the other pointers are null, as allowed.
    let status = unsafe { SHFileOperationW(&mut operation) };
    if status != 0 {
        Err(format!("the shell refused it (code {status:#x})"))
    } else if operation.fAnyOperationsAborted != 0 {
        Err("the shell aborted it".to_string())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn recycle(_: &str) -> Result<(), String> {
    Err("no recycle bin on this system (file/trash moves to the desktop trash)".to_string())
}

/// `(copy-file from to)` — copy the file `from` to `to` (replacing `to` if it
/// exists), preserving the contents byte-for-byte and the permission bits.
/// Returns nil; errors on failure. The binary-safe counterpart to a `slurp`+`spit`
/// (which is UTF-8 string I/O and would corrupt non-text files / drop the mode).
pub(super) fn copy_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let from = expect_string(heap, "file/cp", arg(args, 0))?;
    let to = expect_string(heap, "file/cp", arg(args, 1))?;
    std::fs::copy(&from, &to).map_err(|e| {
        LispError::runtime(format!("file/cp: {} -> {}: {}", from, to, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(file-mtime path)` — last-modified time of `path` as epoch-milliseconds, or
/// `nil` if the file is missing or its mtime can't be read. A cheap `stat`, not a
/// read — pairs with `load` to drive a hot-reloader: poll `file-mtime`, reload
/// only when it changes. Resolution is platform-dependent (typically nanoseconds
/// on Linux, truncated to ms here).
pub(super) fn file_mtime(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/mtime", arg(args, 0))?;
    let Ok(meta) = std::fs::metadata(&path) else {
        return Ok(Value::nil());
    };
    let Ok(modified) = meta.modified() else {
        return Ok(Value::nil());
    };
    let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH) else {
        return Ok(Value::nil());
    };
    Ok(Value::int(since.as_millis() as i64))
}

/// `(file/set-mtime path ms)` — set `path`'s last-modified time to `ms` epoch-milliseconds
/// (negative for before 1970). By path, as `touch -t` dates a file: a read-only file or a
/// directory its caller owns is dated too, and no `touch` program is run.
pub(super) fn set_file_mtime(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/set-mtime", arg(args, 0))?;
    let ms = expect_int(heap, "file/set-mtime", arg(args, 1))?;
    let failed = |e: std::io::Error| {
        LispError::runtime(format!("file/set-mtime: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    };
    let offset = std::time::Duration::from_millis(ms.unsigned_abs());
    let when = if ms >= 0 {
        std::time::UNIX_EPOCH.checked_add(offset)
    } else {
        std::time::UNIX_EPOCH.checked_sub(offset)
    }
    .ok_or_else(|| LispError::runtime(format!("file/set-mtime: {ms} ms is out of range")))?;
    set_modified_at(&path, ms, when).map_err(failed)?;
    Ok(Value::nil())
}

/// Set `path`'s modification time as `touch -t` does: by PATH, needing only that the caller
/// may change the file's times (it owns it), never write access — a 0444 file, or a
/// directory, is dated like any other. Unix: `utimensat`, the access time left alone.
#[cfg(unix)]
fn set_modified_at(path: &str, ms: i64, _when: std::time::SystemTime) -> std::io::Result<()> {
    let name = std::ffi::CString::new(path)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "a NUL in the path"))?;
    let times = [
        libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        },
        libc::timespec {
            tv_sec: ms.div_euclid(1000) as libc::time_t,
            tv_nsec: (ms.rem_euclid(1000) * 1_000_000) as _,
        },
    ];
    // SAFETY: a NUL-terminated path and a two-entry timespec array, both live for the call.
    let result = unsafe { libc::utimensat(libc::AT_FDCWD, name.as_ptr(), times.as_ptr(), 0) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Windows: the file opened for FILE_WRITE_ATTRIBUTES alone — which a read-only file grants
/// its owner — with backup semantics so a directory opens too; then its time set.
#[cfg(windows)]
fn set_modified_at(path: &str, _ms: i64, when: std::time::SystemTime) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .access_mode(windows_sys::Win32::Storage::FileSystem::FILE_WRITE_ATTRIBUTES)
        .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    file.set_modified(when)
}

#[cfg(not(any(unix, windows)))]
fn set_modified_at(path: &str, _ms: i64, when: std::time::SystemTime) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .set_modified(when)
}

/// `(file-stat path)` — one `stat` for `path` as a map, or `nil` if it is missing.
/// Collapses the `dir?` / `file-size` / `file-mtime` trio (each its own syscall)
/// into a single metadata read — the shape a directory lister (dired) wants per
/// entry. `:symlink?` and `:mode` describe the link itself (`symlink_metadata`),
/// while `:dir?` / `:size` / `:mtime` follow it (a symlink to a directory reports
/// `:dir? true` so it's navigable, yet `:symlink? true` so it can be marked). Off
/// unix there are no permission bits, so `:mode` is 0 and `:exec?` is false.
pub(super) fn file_stat(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/stat", arg(args, 0))?;
    // lstat for the link's own nature; stat (follows) for size/mtime/dir?-of-target.
    let Ok(lmeta) = std::fs::symlink_metadata(&path) else {
        return Ok(Value::nil());
    };
    let symlink = lmeta.file_type().is_symlink();
    // Follow the link for the navigable facts; fall back to the link itself for a
    // dangling symlink (so a broken link still lists rather than vanishing).
    let meta = std::fs::metadata(&path).unwrap_or(lmeta);

    let epoch_ms = |t: std::io::Result<std::time::SystemTime>| {
        t.ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| Value::int(d.as_millis() as i64))
            .unwrap_or(Value::nil())
    };
    let mtime = epoch_ms(meta.modified());
    let atime = epoch_ms(meta.accessed());

    #[cfg(unix)]
    let (mode, exec, nlink, uid, gid) = {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let m = meta.permissions().mode();
        (
            m as i64 & 0o7777,
            m & 0o111 != 0,
            meta.nlink() as i64,
            meta.uid(),
            meta.gid(),
        )
    };
    #[cfg(not(unix))]
    let (mode, exec, nlink, uid, gid) = (0_i64, false, 1_i64, 0_u32, 0_u32);

    // Owner/group names (getpwuid/getgrgid), falling back to the numeric id as a string.
    let owner = uid_name(uid).unwrap_or_else(|| uid.to_string());
    let group = gid_name(gid).unwrap_or_else(|| gid.to_string());
    let owner_v = heap.alloc_string(&owner);
    let group_v = heap.alloc_string(&group);
    let pairs = vec![
        (kw("dir?"), Value::boolean(meta.is_dir())),
        (kw("size"), Value::int(meta.len() as i64)),
        (kw("mtime"), mtime),
        (kw("atime"), atime),
        (kw("symlink?"), Value::boolean(symlink)),
        (kw("exec?"), Value::boolean(exec)),
        (kw("mode"), Value::int(mode)),
        (kw("nlink"), Value::int(nlink)),
        (kw("uid"), Value::int(uid as i64)),
        (kw("gid"), Value::int(gid as i64)),
        (kw("owner"), owner_v),
        (kw("group"), group_v),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// `(file/read-link path)` — the target the symlink at `path` stores, unresolved (`readlink`,
/// not `realpath`), or nil when `path` is missing or not a symlink — the shape `file/stat`
/// has for a missing path, since "is this a link?" is a question, not an error. A link that
/// cannot be read (a permission refusal on its directory) raises.
pub(super) fn read_link(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/read-link", arg(args, 0))?;
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => {}
        _ => return Ok(Value::nil()),
    }
    let target = std::fs::read_link(&path).map_err(|e| {
        LispError::runtime(format!("file/read-link: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(heap.alloc_string(&target.to_string_lossy()))
}

/// `(file/symlink target path opts?)` — make `path` a symlink to `target`, or answer a failure
/// saying why not: an outcome a caller expects to meet (the name is taken, Windows refuses the
/// privilege), so a value rather than a raise, as `file/trash` answers. `{:replace true}`
/// replaces an existing entry atomically (a temp link beside it, renamed over).
pub(super) fn make_symlink(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let target = expect_string(heap, "file/symlink", arg(args, 0))?;
    let path = expect_string(heap, "file/symlink", arg(args, 1))?;
    let replace = match arg(args, 2) {
        Value::Map(opts) => heap
            .map_get(opts, kw("replace"))
            .is_some_and(crate::eval::truthy),
        Value::Nil => false,
        other => return Err(LispError::wrong_type(heap, "file/symlink", "map", other)),
    };
    let made = if replace {
        symlink_replacing(&target, std::path::Path::new(&path))
    } else {
        symlink_at(&target, std::path::Path::new(&path))
    };
    Ok(match made {
        Ok(()) => Value::nil(),
        Err(error) => heap.alloc_failure(&format!(
            "file/symlink: {} -> {}: {}",
            path,
            target,
            symlink_refusal(&error)
        )),
    })
}

/// `link` made a symlink to `target`, as stored. Unix: `symlink(2)`.
#[cfg(unix)]
fn symlink_at(target: &str, link: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Windows tells a directory link from a file link when it is made, so the target is looked
/// at — read from the link's own directory, as the link will read it. A missing target makes a
/// file link: nothing says what it will be, and a file is the common case.
#[cfg(windows)]
fn symlink_at(target: &str, link: &std::path::Path) -> std::io::Result<()> {
    let target_path = std::path::Path::new(target);
    let seen_from_link = if target_path.is_absolute() {
        target_path.to_path_buf()
    } else {
        link.parent()
            .unwrap_or(std::path::Path::new("."))
            .join(target_path)
    };
    if seen_from_link.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

#[cfg(not(any(unix, windows)))]
fn symlink_at(_target: &str, _link: &std::path::Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "this system has no symlinks",
    ))
}

/// `link` replaced by a symlink to `target` in one rename: the new link made under a free
/// temp name beside it, then renamed over it, so the name never goes missing. A real
/// directory at `link` is refused — a rename would not replace it on Unix, and replacing a
/// tree with a link is not what "redirect this link" means.
fn symlink_replacing(target: &str, link: &std::path::Path) -> std::io::Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(link)
        && meta.is_dir()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::IsADirectory,
            "a directory is there; it is not replaced by a link",
        ));
    }
    let directory = link
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let mut last_error: std::io::Error = std::io::ErrorKind::AlreadyExists.into();
    for attempt in 0..64_u32 {
        let temp = directory.join(format!(
            ".brood-link-{}-{}",
            std::process::id(),
            stamp.wrapping_add(attempt)
        ));
        match symlink_at(target, &temp) {
            Ok(()) => {
                return std::fs::rename(&temp, link).inspect_err(|_| {
                    let _ = std::fs::remove_file(&temp);
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => last_error = error,
            Err(error) => return Err(error),
        }
    }
    Err(last_error)
}

/// Why a symlink was refused, in words a person can act on: Windows' missing privilege is
/// named for what grants it.
fn symlink_refusal(error: &std::io::Error) -> String {
    #[cfg(windows)]
    if error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_PRIVILEGE_NOT_HELD as i32)
    {
        return "this account may not make symlinks: turn on Developer Mode, or grant SeCreateSymbolicLinkPrivilege".to_string();
    }
    error.to_string()
}

/// `(file/chmod path mode)` — set `path`'s permission bits, or a failure saying why not
/// (missing, not the caller's file). Unix: `chmod(2)`, which follows a symlink. Windows: the
/// read-only attribute from the owner-write bit, the one permission a Windows file carries.
pub(super) fn change_mode(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/chmod", arg(args, 0))?;
    let mode = expect_int(heap, "file/chmod", arg(args, 1))?;
    if !(0..=0o7777).contains(&mode) {
        return Err(LispError::runtime(format!(
            "file/chmod: mode {mode:#o} is not permission bits (0 to 0o7777)"
        )));
    }
    Ok(match set_mode(std::path::Path::new(&path), mode as u32) {
        Ok(()) => Value::nil(),
        Err(error) => heap.alloc_failure(&format!("file/chmod: {}: {}", path, error)),
    })
}

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(path: &std::path::Path, mode: u32) -> std::io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(mode & 0o200 == 0);
    std::fs::set_permissions(path, permissions)
}

/// The files this program holds a `file/lock` on, by absolute path: each open `File` keeps
/// its lock, and dropping it (`file/unlock`) releases it. Program-wide, not per process —
/// the lock says the PROGRAM is alive, and the OS releases every one when it exits.
static HELD_LOCKS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, std::fs::File>>,
> = std::sync::LazyLock::new(Default::default);

/// `path` made absolute, so `file/lock` and `file/unlock` agree on a file however it was
/// spelled (the working directory is the program's).
fn lock_key(path: &str) -> std::path::PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| std::path::PathBuf::from(path))
}

/// `(file/lock path)` — an exclusive advisory lock on `path`, held for the program's life:
/// true, or a failure (another program holds it; the file cannot be opened).
pub(super) fn lock_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/lock", arg(args, 0))?;
    let key = lock_key(&path);
    let mut held = crate::core::sync::lock(&HELD_LOCKS);
    if held.contains_key(&key) {
        return Ok(Value::boolean(true));
    }
    let opened = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&key);
    let file = match opened {
        Ok(file) => file,
        Err(error) => return Ok(heap.alloc_failure(&format!("file/lock: {path}: {error}"))),
    };
    Ok(match file.try_lock() {
        Ok(()) => {
            held.insert(key, file);
            Value::boolean(true)
        }
        Err(std::fs::TryLockError::WouldBlock) => {
            heap.alloc_failure(&format!("file/lock: {path}: another program holds it"))
        }
        Err(std::fs::TryLockError::Error(error)) => {
            heap.alloc_failure(&format!("file/lock: {path}: {error}"))
        }
    })
}

/// `(file/unlock path)` — release this program's lock on `path`; nil either way.
pub(super) fn unlock_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/unlock", arg(args, 0))?;
    // Released explicitly, not by the drop. A `flock` lock belongs to the open file
    // description, and a child another process is spawning shares that description from its
    // fork until its exec closes it: closing OUR descriptor in that window leaves the lock
    // held, and `file/locked?` straight after answered true (in a full suite run, where
    // other tests spawn all the time; 315 of 3000 unlocks with spawners running). `LOCK_UN`
    // releases the description's lock whoever else still refers to it.
    if let Some(file) = crate::core::sync::lock(&HELD_LOCKS).remove(&lock_key(&path)) {
        let _ = file.unlock();
    }
    Ok(Value::nil())
}

/// `(file/locked? path)` — whether any program, this one included, holds a lock on `path`.
/// A shared lock taken and dropped at once: it is refused only while an exclusive one is held.
pub(super) fn file_locked(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/locked?", arg(args, 0))?;
    let key = lock_key(&path);
    if crate::core::sync::lock(&HELD_LOCKS).contains_key(&key) {
        return Ok(Value::boolean(true));
    }
    let Ok(file) = std::fs::File::open(&key) else {
        return Ok(Value::boolean(false));
    };
    Ok(Value::boolean(match file.try_lock_shared() {
        // Released explicitly, as `file/unlock` does: a child mid-spawn would otherwise keep
        // this probe's shared lock past the drop, and a `file/lock` straight after would be
        // refused as though another program held the file.
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(std::fs::TryLockError::Error(_)) => false,
    }))
}

/// The user name for `uid` via `getpwuid`, or `None` if it doesn't resolve. The libc
/// call returns a pointer into a shared static buffer, so a process-wide lock serialises
/// our calls (Brood schedules green processes across OS threads); the name is copied out
/// before the lock drops. `None` off unix.
#[cfg(unix)]
pub(super) fn uid_name(uid: u32) -> Option<String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = crate::core::sync::lock(&LOCK);
    // SAFETY: `getpwuid`'s result points into libc's static buffer, which only another
    // `getpwuid` call can overwrite — and every one in this process holds `LOCK`. The name
    // is copied out before the guard drops.
    unsafe {
        let pw = libc::getpwuid(uid as libc::uid_t);
        if pw.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*pw).pw_name)
            .to_str()
            .ok()
            .map(|s| s.to_string())
    }
}

/// The group name for `gid` via `getgrgid` (see `uid_name` for the locking note).
#[cfg(unix)]
pub(super) fn gid_name(gid: u32) -> Option<String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = crate::core::sync::lock(&LOCK);
    // SAFETY: as in `uid_name`, for `getgrgid`'s static buffer.
    unsafe {
        let gr = libc::getgrgid(gid as libc::gid_t);
        if gr.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*gr).gr_name)
            .to_str()
            .ok()
            .map(|s| s.to_string())
    }
}

#[cfg(not(unix))]
pub(super) fn uid_name(_uid: u32) -> Option<String> {
    None
}

#[cfg(not(unix))]
pub(super) fn gid_name(_gid: u32) -> Option<String> {
    None
}

/// `(%file-swap lock-path data-path expected new)` — replace the ENTIRE contents of
/// `data-path` with `new`, but only if they currently equal `expected`. Returns
/// true when the swap happened, false when the contents differed (the caller should
/// re-read, recompute, and try again).
///
/// This is the mechanism behind a safe read-modify-write of a file whose "modify"
/// step is Brood code — `nest add` editing `project.blsp`, say. Without it, two
/// concurrent editors both read the original, both append, and the second write
/// erases the first while both report success.
///
/// Two properties make it work, and both are load-bearing:
///
///   * **Serialised** by a blocking exclusive `flock` on `lock-path` — a separate
///     file, never the data file, because the data file is replaced by `rename`
///     below and a lock on a since-unlinked inode excludes nobody. The lock is held
///     only for the duration of this call, so it cannot leak, and the OS drops it if
///     the process dies.
///   * **Crash-atomic** in its write: the new contents go to a temp file in the same
///     directory and are `rename`d over `data-path`, so a crash mid-call leaves the
///     old file intact rather than a truncated one. (A half-written manifest is
///     exactly the "project no longer parses" failure this is meant to prevent.)
///
/// A missing `data-path` reads as `""`, so the same call creates it when `expected`
/// is `""`.
pub(super) fn file_swap(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let lock_path = expect_string(heap, "%file-swap", arg(args, 0))?;
    let data_path = expect_string(heap, "%file-swap", arg(args, 1))?;
    let expected = expect_string(heap, "%file-swap", arg(args, 2))?;
    let new = expect_string(heap, "%file-swap", arg(args, 3))?;

    let io_err = |what: &str, path: &str, e: &std::io::Error| {
        LispError::runtime(format!("%file-swap: {what} {path}: {e}"))
            .with_code(crate::error::error_codes::FILE_IO)
    };

    // The lock file's own directory must exist; the caller picks a durable
    // location (the project's cache dir), so a missing parent is a real error.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| io_err("cannot open lock file", &lock_path, &e))?;
    let _guard = FileLock::acquire(&lock).map_err(|e| io_err("cannot lock", &lock_path, &e))?;

    // Read under the lock: this is the re-validation that makes the caller's
    // earlier (unlocked) read safe to act on.
    let current = match std::fs::read_to_string(&data_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io_err("cannot read", &data_path, &e)),
    };
    if current != expected {
        return Ok(Value::boolean(false));
    }

    // The temp file is created with O_EXCL (`create_new`) at an *unguessable* path,
    // and both halves matter. `fs::write` to `{data}.swap.{pid}` is
    // `O_CREAT|O_WRONLY|O_TRUNC` with no `O_EXCL` and no symlink check, so in a
    // directory an attacker can write to, a symlink pre-planted at that entirely
    // predictable name makes this swap overwrite whatever it points at — `O_CREAT`
    // follows symlinks, `O_EXCL` refuses them. The random suffix removes the
    // pre-planting; `create_new` removes the follow. Retried because a random name can
    // (astronomically rarely) already exist.
    let (mut temp_file, temp_path) = {
        let mut made = None;
        let mut last: Option<std::io::Error> = None;
        for _ in 0..8 {
            let mut rnd = [0u8; 8];
            getrandom::fill(&mut rnd).map_err(|e| {
                LispError::runtime(format!("%file-swap: cannot get randomness: {e}"))
                    .with_code(crate::error::error_codes::FILE_IO)
            })?;
            let suffix = rnd.iter().fold(String::new(), |mut s, b| {
                use std::fmt::Write as _;
                let _ = write!(s, "{b:02x}");
                s
            });
            let path = format!("{data_path}.swap.{suffix}");
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(f) => {
                    made = Some((f, path));
                    break;
                }
                Err(e) => last = Some(e),
            }
        }
        match made {
            Some(pair) => pair,
            None => {
                let e =
                    last.unwrap_or_else(|| std::io::Error::other("no temp path could be created"));
                return Err(io_err("cannot create temp file beside", &data_path, &e));
            }
        }
    };
    {
        use std::io::Write as _;
        if let Err(e) = temp_file.write_all(new.as_bytes()) {
            drop(temp_file);
            let _ = std::fs::remove_file(&temp_path);
            return Err(io_err("cannot write", &temp_path, &e));
        }
    }
    drop(temp_file);
    if let Err(e) = std::fs::rename(&temp_path, &data_path) {
        // Don't leave the temp file behind on a failed rename.
        let _ = std::fs::remove_file(&temp_path);
        return Err(io_err("cannot replace", &data_path, &e));
    }
    Ok(Value::boolean(true))
}

/// An exclusive advisory lock held for a scope, released on drop (and by the OS if
/// the process dies, which is what keeps a crash from leaving a stale lock).
struct FileLock<'a> {
    file: &'a std::fs::File,
}

impl<'a> FileLock<'a> {
    #[cfg(unix)]
    fn acquire(file: &'a std::fs::File) -> std::io::Result<Self> {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        loop {
            // SAFETY: `fd` is a live descriptor owned by `file` for this scope.
            let rc = unsafe { libc::flock(fd, libc::LOCK_EX) };
            if rc == 0 {
                return Ok(FileLock { file });
            }
            let err = std::io::Error::last_os_error();
            // A signal can interrupt the blocking wait; that is not a failure.
            if err.kind() != std::io::ErrorKind::Interrupted {
                return Err(err);
            }
        }
    }

    // Off Unix (Windows: `LockFileEx`), std's own exclusive lock, which blocks as `flock`
    // does and is likewise released by the OS when the process dies.
    #[cfg(not(unix))]
    fn acquire(file: &'a std::fs::File) -> std::io::Result<Self> {
        match file.lock() {
            Ok(()) => {}
            // A target with no file locks at all (wasm32): the compare-and-swap still runs,
            // unserialised, which is all one process needs.
            Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {}
            Err(error) => return Err(error),
        }
        Ok(FileLock { file })
    }
}

#[cfg(unix)]
impl Drop for FileLock<'_> {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        // SAFETY: same live descriptor; failure to unlock is not actionable here,
        // and closing the fd would release it regardless.
        unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[cfg(not(unix))]
impl Drop for FileLock<'_> {
    fn drop(&mut self) {
        // Not actionable here, and closing the file would release it regardless.
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod windows_path_text_tests {
    use super::windows_path_text;

    #[test]
    fn a_drive_path_takes_forward_slashes() {
        assert_eq!(windows_path_text(r"C:\Users\me\src"), "C:/Users/me/src");
    }

    #[test]
    fn the_verbatim_prefix_is_dropped() {
        assert_eq!(windows_path_text(r"\\?\C:\Users\me"), "C:/Users/me");
    }

    #[test]
    fn a_verbatim_unc_path_is_a_unc_path_again() {
        assert_eq!(
            windows_path_text(r"\\?\UNC\server\share\x"),
            "//server/share/x"
        );
    }
}

#[cfg(all(test, unix))]
mod replace_tests {
    use super::{directory_refuses_entries, replace_file};
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    fn write_all(file: &mut std::fs::File, bytes: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(file, bytes)
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("brood-replace-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn names(directory: &std::path::Path) -> Vec<String> {
        let mut found: Vec<String> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        found.sort();
        found
    }

    #[test]
    fn a_file_this_user_may_not_write_stays_refused() {
        let directory = scratch("read-only");
        let path = directory.join("keep.txt");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let outcome = replace_file(&path, b"new", write_all);
        assert_eq!(
            outcome.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
    }

    #[test]
    fn a_directory_that_takes_no_entries_is_written_in_place() {
        let directory = scratch("closed-directory");
        let path = directory.join("keep.txt");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o555)).unwrap();
        let outcome = replace_file(&path, b"new", write_all);
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
        outcome.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    #[test]
    fn no_room_for_the_temp_file_is_no_reason_to_write_in_place() {
        let full = std::io::Error::from(std::io::ErrorKind::StorageFull);
        assert!(!directory_refuses_entries(&full));
        let quota = std::io::Error::from(std::io::ErrorKind::QuotaExceeded);
        assert!(!directory_refuses_entries(&quota));
    }

    #[test]
    fn a_full_disk_leaves_the_old_file_and_no_temp() {
        let directory = scratch("full");
        let path = directory.join("notes.txt");
        std::fs::write(&path, "old").unwrap();
        let outcome = replace_file(&path, b"new", |_, _| {
            Err(std::io::Error::from_raw_os_error(libc::ENOSPC))
        });
        assert_eq!(outcome.unwrap_err().raw_os_error(), Some(libc::ENOSPC));
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert_eq!(names(&directory), ["notes.txt"]);
    }

    #[test]
    fn the_mode_is_kept_and_the_file_is_a_new_inode() {
        let directory = scratch("mode");
        let path = directory.join("run.sh");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o751)).unwrap();
        let before = std::fs::metadata(&path).unwrap().ino();
        replace_file(&path, b"new", write_all).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o7777, 0o751);
        assert_ne!(meta.ino(), before, "a rename, not a write in place");
        assert_eq!(names(&directory), ["run.sh"]);
    }

    #[test]
    fn a_new_file_gets_the_mode_a_plain_write_gives() {
        let directory = scratch("new");
        let plain = directory.join("plain");
        std::fs::write(&plain, "x").unwrap();
        let path = directory.join("made");
        replace_file(&path, b"new", write_all).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode(),
            std::fs::metadata(&plain).unwrap().permissions().mode()
        );
    }

    #[test]
    fn a_symlink_stays_one_and_its_target_is_replaced() {
        let directory = scratch("link");
        let target = directory.join("target.txt");
        let link = directory.join("link.txt");
        std::fs::write(&target, "old").unwrap();
        symlink("target.txt", &link).unwrap();
        replace_file(&link, b"new", write_all).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
    }

    #[test]
    fn a_dangling_symlink_is_written_through() {
        let directory = scratch("dangling");
        let link = directory.join("link.txt");
        symlink("target.txt", &link).unwrap();
        replace_file(&link, b"new", write_all).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(directory.join("target.txt")).unwrap(), b"new");
    }

    #[test]
    fn a_hard_linked_file_is_written_in_place_so_the_link_sees_it() {
        let directory = scratch("hard");
        let path = directory.join("one");
        let other = directory.join("two");
        std::fs::write(&path, "old").unwrap();
        std::fs::hard_link(&path, &other).unwrap();
        replace_file(&path, b"new", write_all).unwrap();
        assert_eq!(std::fs::read(&other).unwrap(), b"new");
    }
}

// Run by CI's windows-smoke job (the `windows_replace_tests` filter on the brood lib tests):
// the ADR-671 replace over an existing file, a missing one, and one another program holds open.
#[cfg(all(test, windows))]
mod windows_replace_tests {
    use super::replace_file;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_HIDDEN, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, GetFileAttributesW, GetFileInformationByHandle, SetFileAttributesW,
    };

    fn write_all(file: &mut std::fs::File, bytes: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(file, bytes)
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("brood-replace-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn names(directory: &std::path::Path) -> Vec<String> {
        let mut found: Vec<String> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        found.sort();
        found
    }

    fn attributes(path: &std::path::Path) -> u32 {
        // SAFETY: a NUL-terminated wide path; the temporary lives to the end of the statement.
        unsafe { GetFileAttributesW(super::wide_path(path).as_ptr()) }
    }

    /// The file's index on its volume: what a write in place keeps and a replace changes.
    fn file_index(path: &std::path::Path) -> u64 {
        let file = std::fs::File::open(path).unwrap();
        // SAFETY: BY_HANDLE_FILE_INFORMATION is plain data, so all-zero is a valid value.
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        assert_ne!(
            // SAFETY: the handle is `file`'s, open for the call; `information` is writable.
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) },
            0
        );
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow)
    }

    /// `path` opened by "another program" that shares only what `share` says.
    fn hold(path: &std::path::Path, share: u32) -> std::fs::File {
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(share)
            .open(path)
            .unwrap()
    }

    #[test]
    fn an_existing_file_is_replaced_and_keeps_its_attributes() {
        let directory = scratch("existing");
        let path = directory.join("notes.txt");
        std::fs::write(&path, "old").unwrap();
        let wide = super::wide_path(&path);
        assert_ne!(
            // SAFETY: `wide` is a NUL-terminated wide path, live for the call.
            unsafe { SetFileAttributesW(wide.as_ptr(), attributes(&path) | FILE_ATTRIBUTE_HIDDEN) },
            0
        );
        let before = file_index(&path);
        replace_file(&path, b"new", write_all).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_ne!(
            attributes(&path) & FILE_ATTRIBUTE_HIDDEN,
            0,
            "ReplaceFileW keeps attributes"
        );
        assert_ne!(file_index(&path), before, "a replace, not a write in place");
        assert_eq!(names(&directory), ["notes.txt"]);
    }

    #[test]
    fn a_missing_file_is_made() {
        let directory = scratch("missing");
        let path = directory.join("made.txt");
        replace_file(&path, b"new", write_all).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(names(&directory), ["made.txt"]);
    }

    #[test]
    fn a_file_held_open_is_refused_with_a_reason_and_left_whole() {
        let directory = scratch("held");
        let path = directory.join("busy.txt");
        std::fs::write(&path, "old").unwrap();
        let holder = hold(&path, FILE_SHARE_READ);
        let started = std::time::Instant::now();
        let error = replace_file(&path, b"new", write_all).unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert!(
            error
                .to_string()
                .contains("another program has the file open"),
            "{error}"
        );
        drop(holder);
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert_eq!(names(&directory), ["busy.txt"]);
    }

    #[test]
    fn a_file_let_go_of_within_the_wait_is_replaced() {
        let directory = scratch("released");
        let path = directory.join("busy.txt");
        std::fs::write(&path, "old").unwrap();
        let holder = hold(&path, FILE_SHARE_READ);
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(60));
            drop(holder);
        });
        replace_file(&path, b"new", write_all).unwrap();
        releaser.join().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(names(&directory), ["busy.txt"]);
    }

    #[test]
    fn a_file_a_reader_shares_fully_is_replaced_under_it() {
        let directory = scratch("shared");
        let path = directory.join("log.txt");
        std::fs::write(&path, "old").unwrap();
        let reader = hold(
            &path,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        );
        replace_file(&path, b"new", write_all).unwrap();
        drop(reader);
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(names(&directory), ["log.txt"]);
    }

    #[test]
    fn a_read_only_file_stays_refused() {
        let directory = scratch("read-only");
        let path = directory.join("keep.txt");
        std::fs::write(&path, "old").unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions.clone()).unwrap();
        let outcome = replace_file(&path, b"new", write_all);
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).unwrap();
        assert_eq!(
            outcome.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert_eq!(names(&directory), ["keep.txt"]);
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_and_no_temp() {
        let directory = scratch("full");
        let path = directory.join("notes.txt");
        std::fs::write(&path, "old").unwrap();
        let outcome = replace_file(&path, b"new", |_, _| {
            Err(std::io::Error::from(std::io::ErrorKind::StorageFull))
        });
        assert_eq!(outcome.unwrap_err().kind(), std::io::ErrorKind::StorageFull);
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert_eq!(names(&directory), ["notes.txt"]);
    }

    #[test]
    fn a_hard_linked_file_is_written_in_place_so_the_link_sees_it() {
        let directory = scratch("hard");
        let path = directory.join("one");
        let other = directory.join("two");
        std::fs::write(&path, "old").unwrap();
        std::fs::hard_link(&path, &other).unwrap();
        replace_file(&path, b"new", write_all).unwrap();
        assert_eq!(std::fs::read(&other).unwrap(), b"new");
    }
}

#[cfg(all(test, unix))]
mod link_and_mode_tests {
    use super::{set_mode, symlink_at, symlink_replacing};

    fn scratch(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("brood-link-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn a_link_keeps_its_target_as_written() {
        let directory = scratch("as-written");
        let link = directory.join("link");
        symlink_at("../elsewhere/missing", &link).unwrap();
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            std::path::Path::new("../elsewhere/missing")
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_taken_name_is_refused_unless_replaced() {
        let directory = scratch("taken");
        let link = directory.join("link");
        std::fs::write(&link, "plain").unwrap();
        let refused = symlink_at("a", &link).unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::AlreadyExists);
        symlink_replacing("b", &link).unwrap();
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            std::path::Path::new("b")
        );
        // no temp link left beside it
        let names: Vec<_> = std::fs::read_dir(&directory).unwrap().collect();
        assert_eq!(names.len(), 1);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_directory_is_not_replaced_by_a_link() {
        let directory = scratch("directory");
        let inner = directory.join("inner");
        std::fs::create_dir(&inner).unwrap();
        assert!(symlink_replacing("x", &inner).is_err());
        assert!(inner.is_dir());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn mode_bits_round_trip() {
        use std::os::unix::fs::PermissionsExt;
        let directory = scratch("mode");
        let file = directory.join("tool");
        std::fs::write(&file, "#!/bin/sh\n").unwrap();
        set_mode(&file, 0o751).unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o751);
        let _ = std::fs::remove_dir_all(&directory);
    }
}
