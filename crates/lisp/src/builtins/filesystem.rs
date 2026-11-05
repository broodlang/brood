//! The filesystem primitives: paths, directories, whole-file reads and writes (text and
//! bytes), metadata, and the atomic `%file-swap`. Policy — `file/*`, `path/*` — is Brood.

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value, kw};
use crate::error::{LispError, LispResult};

use super::args::{arg, expect_int, expect_string};
use super::bytes::{bytes_to_value, collect_bytes, flatten_iolist};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
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
        file_exists,
    );
    primitives.def(
        "%canonicalize",
        Arity::exact(1),
        Sig::new(vec![string], string.union(nil_ty)),
        &["path"],
        "The real absolute path of `path` with symlinks and ./.. resolved. Works for a not-yet-existing target (the longest existing ancestor is resolved, then the remaining components appended). Relative paths are taken against the cwd. nil only if the cwd itself can't be read. Use it to make path sandboxing symlink-escape-proof.",
        path_canonicalize);
    primitives.def(
        "file/dir?",
        Arity::exact(1),
        Sig::new(vec![string], bool_ty),
        &["path"],
        "Whether path is a directory.\n\n    (file/dir? \"/tmp\")   ;=> true",
        is_dir,
    );
    primitives.def(
        "file/ls",
        Arity::exact(1),
        Sig::new(vec![string], Ty::list_of(string)),
        &["path"],
        "The entry names directly under directory path, sorted.\n\n    (file/ls \"src\")   ;~> (\"main.blsp\" \"util.blsp\")",
        list_dir,
    );
    primitives.def(
        "file/mkdir",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        &["path"],
        "Create a directory and any missing parents (like mkdir -p).\n\n    (file/mkdir \"build/out\")   ;~> nil, with build/ and build/out/ created",
        make_dir,
    );
    primitives.def(
        "file/mkdir-private",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        &["path"],
        "Create a directory and any missing parents (like mkdir -p), the directory itself owner-only whether it was made now or was already there — where an app keeps what other users must not read (an editor's sidecars of files edited as root, ADR-588), as file/spit-private is for a file. On Unix that is mode 0700, and no one else can reach anything inside, whatever its own mode. On Windows it is NOT the same guard: the directory gets a protected one-entry DACL (the current user) that what is made inside afterwards inherits, but every user holds the bypass-traverse-checking right, so a directory's DACL does not stop a file inside being opened by its path — a file that keeps entries of its own (made there before, or moved in) stays as open as they say. Write each secret with file/spit-private, which makes the file itself owner-only.\n\n    (file/mkdir-private \"state/auto-save\")   ;~> nil, state/auto-save made owner-only (mode 0700 on Unix)",
        make_private_dir,
    );
    primitives.def(
        "file/spit",
        Arity::exact(2),
        Sig::new(vec![string, iolist], nil_ty),
        &["path", "s"],
        "Write s (any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139)) to the file at path, replacing any existing file.\n\n    (file/spit \"notes.txt\" \"hello\")   ;~> nil",
        spit);
    primitives.def(
        "file/spit-append",
        Arity::exact(2),
        Sig::new(vec![string, iolist], nil_ty),
        &["path", "s"],
        "Append s (any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139)) to the file at path, creating it if absent (unlike file/spit, which truncates). Returns nil. Opens in append mode so each write lands at end-of-file — the OS-atomic append that makes a log safe to write from several processes at once. The string sibling of append-bytes.\n\n    (file/spit-append \"app.log\" [\"started\" 10])   ;~> nil, the line appended",
        spit_append);
    // Atomic compare-and-swap of a file's whole contents, serialised across
    // processes. The mechanism a safe read-modify-write needs when the "modify" is
    // Brood code (`nest add` editing project.blsp) — see `io::file_swap`.
    primitives.def(
        "%file-swap",
        Arity::exact(4),
        Sig::new(vec![string, string, string, string], bool_ty),
        &["lock-path", "data-path", "expected", "new"],
        "Replace the entire contents of data-path with new, but ONLY if they currently equal expected; returns true when swapped, false when they differ (re-read, recompute, retry). Serialised across processes by a blocking exclusive lock on lock-path (a separate file — the data file is replaced by rename, so a lock on it would exclude nobody), and crash-atomic (temp file + rename, so a crash leaves the old contents intact). A missing data-path reads as \"\", so the same call creates it. The mechanism behind a safe read-modify-write whose modify step is Brood code.",
        file_swap);
    primitives.def(
        "file/slurp",
        Arity::exact(1),
        Sig::new(vec![string], string),
        &["path"],
        "Read the whole file at path into a string (does not evaluate it). UTF-8; throws on a non-text file — use file/slurp-bytes for binary.\n\n    (file/slurp \"notes.txt\")   ;~> \"hello\"",
        slurp);
    primitives.def(
        "file/slurp-bytes",
        Arity::exact(1),
        Sig::new(vec![string], bytes_ty),
        &["path"],
        "Read the whole file at path as a bytes value. The byte-faithful read file/slurp can't be (file/slurp is UTF-8 and throws on a non-text file). Pairs with hash/sha256-bytes / hash/sha256-raw and the encoding byte variants — e.g. hashing a binary asset.\n\n    (file/slurp-bytes \"logo.png\")   ;~> the file's bytes",
        slurp_bytes);
    primitives.def(
        "file/spit-bytes",
        Arity::exact(2),
        Sig::new(vec![string, any], nil_ty),
        &["path", "bytes"],
        "Write any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139) to path byte-faithfully, replacing any existing file. Returns nil. The binary write-side counterpart to file/slurp-bytes (file/spit is UTF-8 string-only) — materialises a received image / archive / any binary asset to disk.\n\n    (file/spit-bytes \"copy.png\" (file/slurp-bytes \"logo.png\"))   ;~> nil",
        spit_bytes);
    primitives.def(
        "file/spit-bytes-append",
        Arity::exact(2),
        Sig::new(vec![string, any], nil_ty),
        &["path", "bytes"],
        "Append any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139) — to the file at path, byte-faithfully, creating it if absent. Returns nil. The append counterpart of file/spit-bytes (which truncates), as file/spit-append is to file/spit: lets a large payload be streamed to disk chunk-by-chunk (e.g. spooling a file upload) without ever holding it whole in memory.\n\n    (file/spit-bytes-append \"upload.bin\" chunk)   ;~> nil, chunk appended",
        append_bytes);
    primitives.def(
        "file/mtime",
        Arity::exact(1),
        Sig::new(vec![string], int.union(nil_ty)),
        &["path"],
        "Last-modified time of path as epoch-milliseconds, or nil if the file is missing. Cheap (stat) — pair with `load` to drive a hot-reloader.\n\n    (file/mtime \"project.blsp\")   ;~> 1761213600000, or nil when missing",
        file_mtime);
    primitives.def(
        "file/set-mtime",
        Arity::exact(2),
        Sig::new(vec![string, int], nil_ty),
        &["path", "ms"],
        "Set the last-modified time of the file at path to ms epoch-milliseconds — what `file/mtime` then answers. Returns nil; throws if the file is missing or cannot be changed. The portable `touch -t`: a test that needs a file older than another (an auto-save sidecar, a stale cache) on every OS, with no `touch` program.\n\n    (file/set-mtime \"notes.txt\" 1577836800000)   ;~> nil, notes.txt now dated 2020-01-01",
        set_file_mtime);
    primitives.def(
        "file/size",
        Arity::exact(1),
        Sig::new(vec![string], int.union(nil_ty)),
        &["path"],
        "Size of the file at path in bytes, or nil if it is missing.\n\n    (file/size \"notes.txt\")   ;~> 5, or nil when missing",
        file_size,
    );
    primitives.def(
        "file/stat",
        Arity::exact(1),
        Sig::new(vec![string], map_ty.union(nil_ty)),
        &["path"],
        "Metadata for path in ONE stat as a map {:dir? :size :mtime :atime :symlink? :exec? :mode :nlink :uid :gid :owner :group}, or nil if missing. :symlink? reads the link itself (lstat); the rest follow it. :mtime/:atime are epoch-ms last-modified/last-access (nil if unreadable; :atime may be coarse under relatime/noatime mounts); :exec? is the owner-execute bit; :mode is the unix permission bits (0 off-unix); :nlink the hard-link count; :uid/:gid the numeric ids; :owner/:group their resolved names (the numeric id as a string if unresolved). Everything an `ls -l` row + a recency sort needs in one syscall.\n\n    (file/stat \"notes.txt\")   ;~> {:dir? false, :size 5, :mtime 1761213600000, …}",
        file_stat);
    primitives.def(
        "%image-thumb",
        Arity::exact(3),
        Sig::new(vec![any, int, int], map_ty.union(nil_ty)),
        &["bytes", "max-w", "max-h"],
        "Decode an encoded image (PNG/JPEG/GIF/WebP/BMP) from a byte sequence and downscale it to fit within max-w×max-h pixels (aspect ratio preserved), returning {:width :height :rgba} where :rgba is a width*height*4 bytes value (row-major RGBA8). nil when the bytes aren't a decodable image or the dims are non-positive. Per-call decode limits bound a decompression bomb. The one image primitive; rendering (half-block cells, a GUI texture) is Brood policy over the decoded buffer.",
        image_thumb);
    primitives.def(
        "file/rm",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        &["path"],
        "Remove the file at path. Idempotent (nil if already absent); errors on a real I/O failure.\n\n    (file/rm \"notes.txt\")   ;~> nil",
        delete_file);
    primitives.def(
        "file/rmdir",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        &["path"],
        "Remove a directory and everything under it (recursive). Idempotent (nil if already absent); errors on a real I/O failure.\n\n    (file/rmdir \"build\")   ;~> nil",
        delete_dir);
    primitives.def(
        "file/rename",
        Arity::exact(2),
        Sig::new(vec![string, string], nil_ty),
        &["from", "to"],
        "Rename/move file `from` to `to`. Returns nil; errors on failure.\n\n    (file/rename \"draft.txt\" \"final.txt\")   ;~> nil",
        rename_file,
    );
    primitives.def(
        "file/cp",
        Arity::exact(2),
        Sig::new(vec![string, string], nil_ty),
        &["from", "to"],
        "Copy file `from` to `to` (replacing `to`), preserving contents and permissions. Binary-safe (unlike slurp+spit). Returns nil; errors on failure.\n\n    (file/cp \"logo.png\" \"logo-copy.png\")   ;~> nil",
        copy_file);
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

/// The largest side, in pixels, of an image a primitive will decode or accept.
pub(super) const IMAGE_MAX_SIDE: u32 = 16384;
/// The largest buffer, in bytes, an image primitive will allocate for one image.
pub(super) const IMAGE_MAX_ALLOC: u64 = 512 * 1024 * 1024;

/// Decode an encoded image (any format the `image` build reads: PNG, and JPEG / GIF /
/// WebP / BMP with `image-formats`) from bytes nobody vouches for — a file, the clipboard.
/// Per-call `Limits` bound a decompression bomb (`IMAGE_MAX_SIDE` a side, `IMAGE_MAX_ALLOC`
/// in all), and anything that is not a decodable image within them is `None`, never an
/// error.
pub(super) fn decode_untrusted_image(bytes: &[u8]) -> Option<image::DynamicImage> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(IMAGE_MAX_SIDE);
    limits.max_image_height = Some(IMAGE_MAX_SIDE);
    limits.max_alloc = Some(IMAGE_MAX_ALLOC);
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.limits(limits);
    reader.decode().ok()
}

/// Downscale `image` to fit within `max_width`×`max_height`, aspect ratio preserved.
/// Downscale-only: a source already within the box keeps its native size (never
/// upscaled — `thumbnail`/`resize` would blow a small image up to fill the box). The
/// one resize path the image primitives share (`image-thumb`, `clipboard-image`).
pub(super) fn fit_within(
    image: image::DynamicImage,
    max_width: u32,
    max_height: u32,
) -> image::DynamicImage {
    if image.width() <= max_width && image.height() <= max_height {
        image
    } else {
        image.thumbnail(max_width, max_height)
    }
}

/// `(image-thumb bytes max-w max-h)` — decode an encoded image (PNG / JPEG / GIF /
/// WebP / BMP) from a byte sequence and downscale it to fit within `max-w`×`max-h`
/// pixels (aspect ratio preserved), returning `{:width :height :rgba}` where `:rgba`
/// is a `width*height*4` bytes value (row-major RGBA8). Returns nil when the bytes
/// aren't a decodable image or the dims are non-positive — untrusted input degrades
/// to "not an image" rather than throwing. Per-call decode `Limits` bound a
/// decompression bomb (≤ 16384² px, ≤ 512 MB alloc). The one image primitive;
/// rendering (half-block cells, a GUI texture, …) is Brood policy over this buffer.
pub(super) fn image_thumb(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let bytes = collect_bytes("gui/image-thumb", arg(args, 0), heap)?;
    let max_w = expect_int(heap, "gui/image-thumb", arg(args, 1))?;
    let max_h = expect_int(heap, "gui/image-thumb", arg(args, 2))?;
    if max_w <= 0 || max_h <= 0 {
        return Ok(Value::nil());
    }
    let Some(img) = decode_untrusted_image(&bytes) else {
        return Ok(Value::nil());
    };
    let thumb = fit_within(img, max_w as u32, max_h as u32).to_rgba8();
    let (w, h) = (thumb.width(), thumb.height());
    // GC-safe: no eval between this alloc and map_from_pairs (a builtin never fires
    // GC mid-execution), mirroring file_stat holding its string handles.
    let rgba = bytes_to_value(thumb.as_raw(), heap);
    let pairs = vec![
        (kw("width"), Value::int(w as i64)),
        (kw("height"), Value::int(h as i64)),
        (kw("rgba"), rgba),
    ];
    Ok(heap.map_from_pairs(pairs))
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
