//! Package-manager delivery *mechanism* (ADR-037): the `git` and `tar` shells the
//! fetcher drives, plus the `_deps/`-bounded cache eviction. Rust runs the
//! subprocesses and enforces the filesystem guardrail; the cache layout,
//! ref-pinning, sha256 verification, and when-to-reclone *policy* are Brood in
//! `std/tool/package.blsp`. `git`/`tar` are assumed on PATH — the same
//! external-tool tradeoff both these primitives make.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{error_codes, LispError, LispResult};

use super::numeric::{arg, expect_int, expect_string};

/// Run `git` with `args` (optionally in `cwd`), capturing stdout+stderr. The
/// shared mechanism behind the package manager's git primitives (ADR-037).
pub(super) fn run_git(args: &[&str], cwd: Option<&str>) -> Result<std::process::Output, LispError> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    cmd.output().map_err(|e| {
        LispError::runtime(format!("git {}: {}", args.join(" "), e))
            .with_code(error_codes::SUBPROCESS_FAILED)
            .with_hint("is `git` installed and on PATH?")
    })
}

/// Run a `git` subcommand that's expected to succeed; turn a non-zero exit into a
/// `LispError` carrying git's stderr.
pub(super) fn git_or_err(args: &[&str], cwd: Option<&str>) -> Result<(), LispError> {
    let out = run_git(args, cwd)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(LispError::runtime(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .with_code(error_codes::SUBPROCESS_FAILED))
    }
}

/// `(%git-resolve-ref url ref)` — resolve `ref` (a tag, branch, or commit) at the
/// remote `url` to a full commit hash via `git ls-remote`, or `nil` if no such
/// ref exists. For an annotated tag, prefers the peeled `^{}` line (the commit the
/// tag points to). When `ref` is already a commit SHA the remote doesn't advertise
/// (ls-remote returns nothing), it's returned as-is — a commit pins itself.
/// The package manager's ref-pinning mechanism (ADR-037); pinning policy is Brood.
pub(super) fn git_resolve_ref(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let url = expect_string(heap, "%git-resolve-ref", arg(args, 0))?;
    let r = expect_string(heap, "%git-resolve-ref", arg(args, 1))?;
    let out = run_git(&["ls-remote", &url, &r], None)?;
    if !out.status.success() {
        return Err(LispError::runtime(format!(
            "%git-resolve-ref: git ls-remote {} {} failed: {}",
            url,
            r,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .with_code(error_codes::SUBPROCESS_FAILED));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut first: Option<&str> = None;
    let mut peeled: Option<&str> = None;
    for line in stdout.lines() {
        let sha = line.split_whitespace().next();
        if first.is_none() {
            first = sha;
        }
        if line.trim_end().ends_with("^{}") {
            peeled = sha;
        }
    }
    if let Some(s) = peeled.or(first) {
        return Ok(heap.alloc_string(s));
    }
    // No advertised ref: if `ref` itself looks like a commit SHA, it pins itself.
    let looks_like_sha = r.len() >= 7 && r.len() <= 40 && r.chars().all(|c| c.is_ascii_hexdigit());
    if looks_like_sha {
        Ok(heap.alloc_string(&r))
    } else {
        Ok(Value::nil())
    }
}

/// `(%git-list-tags url)` — the tag names published by the remote at `url`, as a
/// list of strings (via `git ls-remote --tags --refs`, so annotated tags' peeled
/// `^{}` lines are dropped and each `refs/tags/<name>` yields just `<name>`). The
/// list is unordered — the caller sorts. An empty list (a remote with no tags) is
/// `nil`. The package manager's git-tag range-resolution mechanism (ADR-209);
/// which tag a range picks is Brood policy in `std/tool/package.blsp`.
pub(super) fn git_list_tags(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let url = expect_string(heap, "%git-list-tags", arg(args, 0))?;
    let out = run_git(&["ls-remote", "--tags", "--refs", &url], None)?;
    if !out.status.success() {
        return Err(LispError::runtime(format!(
            "%git-list-tags: git ls-remote --tags {} failed: {}",
            url,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .with_code(error_codes::SUBPROCESS_FAILED));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let tags: Vec<Value> = stdout
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter_map(|r| r.strip_prefix("refs/tags/"))
        .map(|name| heap.alloc_string(name))
        .collect();
    Ok(heap.list(tags))
}

/// `(%git-changed-files dir)` — absolute paths of files that are NOT
/// committed-clean under `dir`: modified, staged, or untracked (`git status
/// --porcelain`, which unions all three). Returns a **list of strings** (which
/// is `nil` when the tree is clean — an empty Brood list is nil), or the
/// keyword **`:not-a-repo`** when `dir` is not inside a git work tree (distinct
/// from a clean repo's empty list). The mechanism behind `nest format
/// --changed`; the `.blsp` filter + formatting are Brood policy
/// (std/format.blsp).
pub(super) fn git_changed_files(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let dir = expect_string(heap, "%git-changed-files", arg(args, 0))?;
    let not_a_repo = || Ok(Value::keyword(value::intern("not-a-repo")));
    // `--porcelain -z` gives stable, NUL-terminated `XY <path>` records (a rename
    // adds a second NUL-separated path; we take the destination). Run from the
    // repo TOP so paths are root-relative and match what the caller walks.
    // `-uall` lists each untracked FILE individually — without it git collapses a
    // wholly-untracked directory to `?? dir/`, so brand-new files in a new
    // directory would be reported as the directory and dropped by a `.blsp`
    // filter (a `nest format --changed` would silently skip them).
    // A missing/un-spawnable `git` is treated as "not a repo" too (so a box
    // without git falls back to whole-project formatting, not a hard error) —
    // only a non-zero *exit* on a real git means a genuine "not a work tree".
    let top = match run_git(&["-C", &dir, "rev-parse", "--show-toplevel"], None) {
        Ok(o) if o.status.success() => o,
        _ => return not_a_repo(),
    };
    let root = String::from_utf8_lossy(&top.stdout).trim().to_string();
    let out = match run_git(&["-C", &root, "status", "--porcelain", "-z", "-uall"], None) {
        Ok(o) if o.status.success() => o,
        _ => return not_a_repo(),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut paths: Vec<Value> = Vec::new();
    // Records are NUL-separated; each is `XY <path>` (3+ chars). A rename/copy
    // record (`R`/`C` in X) is followed by an extra NUL-separated origin path,
    // which we skip — the record's own path is the current (destination) name.
    let mut it = stdout.split('\0');
    while let Some(rec) = it.next() {
        if rec.len() < 3 {
            continue;
        }
        let (status, path) = rec.split_at(3);
        let x = status.chars().next().unwrap_or(' ');
        if x == 'R' || x == 'C' {
            it.next(); // consume the origin path of a rename/copy
        }
        let abs = std::path::Path::new(&root).join(path);
        paths.push(heap.alloc_string(&abs.to_string_lossy()));
    }
    Ok(heap.list(paths))
}

/// `(%git-clone url dest ref commit)` — populate `dest` with a shallow clone of
/// `url` checked out at the exact `commit` (detached HEAD). Tries to fetch the
/// commit directly (servers that allow SHA-in-want, e.g. GitHub); falls back to
/// fetching `ref` then checking out `commit`. Returns `:ok`, or throws with git's
/// stderr. The package manager's fetch mechanism (ADR-037); the cache layout and
/// when-to-reclone policy are Brood (std/tool/package.blsp).
pub(super) fn git_clone(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let url = expect_string(heap, "%git-clone", arg(args, 0))?;
    let dest = expect_string(heap, "%git-clone", arg(args, 1))?;
    let gref = expect_string(heap, "%git-clone", arg(args, 2))?;
    let commit = expect_string(heap, "%git-clone", arg(args, 3))?;

    if let Some(parent) = std::path::Path::new(&dest).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                LispError::runtime(format!(
                    "%git-clone: cannot create {}: {}",
                    parent.display(),
                    e
                ))
                .with_code(error_codes::FILE_IO)
            })?;
        }
    }

    git_or_err(&["init", "-q", &dest], None)?;
    git_or_err(&["-C", &dest, "remote", "add", "origin", &url], None)?;

    // Fast path: fetch the exact commit shallowly. Many servers (GitHub) allow it.
    let direct = run_git(
        &[
            "-C", &dest, "fetch", "-q", "--depth", "1", "origin", &commit,
        ],
        None,
    )?;
    if !direct.status.success() {
        // Fallback: fetch the named ref (shallow first, then full if the server
        // rejects a shallow ref fetch), which must contain the locked commit.
        if git_or_err(
            &["-C", &dest, "fetch", "-q", "--depth", "1", "origin", &gref],
            None,
        )
        .is_err()
        {
            git_or_err(&["-C", &dest, "fetch", "-q", "origin", &gref], None)?;
        }
    }

    if git_or_err(&["-C", &dest, "checkout", "-q", "--detach", &commit], None).is_err() {
        return Err(LispError::runtime(format!(
            "%git-clone: commit {} is not reachable from {} at {}",
            commit, gref, url
        ))
        .with_code(error_codes::SUBPROCESS_FAILED)
        .with_hint("the ref may have moved since it was locked — try `nest update`"));
    }
    Ok(crate::core::value::kw("ok"))
}

/// `(%untar-gz archive dest strip)` — extract a gzip'd tar archive `archive` into
/// directory `dest`, stripping `strip` leading path components (the package-manager
/// convention is `strip = 1`, dropping the tarball's single wrapper directory so the
/// package root lands directly in `dest`). Shells out to the system `tar` (the same
/// dependency tradeoff as `%git-clone`'s `git`); on the offload allow-list so a large
/// extract runs on the dirty-native pool. Returns `:ok`. The tarball source-delivery
/// mechanism (ADR-037 tarball deps); policy — download, sha256-verify, `_deps/`
/// bounding — lives in `std/tool/package.blsp`.
pub(super) fn untar_gz(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let archive = expect_string(heap, "%untar-gz", arg(args, 0))?;
    let dest = expect_string(heap, "%untar-gz", arg(args, 1))?;
    let strip = expect_int(heap, "%untar-gz", arg(args, 2))?;
    if strip < 0 {
        return Err(
            LispError::runtime("%untar-gz: strip-components must be >= 0".to_string())
                .with_code(error_codes::FILE_IO),
        );
    }
    std::fs::create_dir_all(&dest).map_err(|e| {
        LispError::runtime(format!("%untar-gz: cannot create {}: {}", dest, e))
            .with_code(error_codes::FILE_IO)
    })?;
    let strip_arg = format!("--strip-components={}", strip);
    let out = std::process::Command::new("tar")
        .args(["-xzf", &archive, "-C", &dest, &strip_arg])
        .output()
        .map_err(|e| {
            LispError::runtime(format!("%untar-gz: tar: {}", e))
                .with_code(error_codes::SUBPROCESS_FAILED)
                .with_hint("is `tar` installed and on PATH?")
        })?;
    if out.status.success() {
        Ok(crate::core::value::kw("ok"))
    } else {
        Err(LispError::runtime(format!(
            "%untar-gz: extracting {} failed: {}",
            archive,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .with_code(error_codes::SUBPROCESS_FAILED))
    }
}

/// `(%rm-rf path)` — recursively delete `path`. **Bounded to `_deps/`**: refuses
/// any path without a `_deps` component, so a mis-computed cache path can't delete
/// something outside the package cache. Idempotent (`:ok` if already absent). The
/// package manager's cache-eviction mechanism (ADR-037); `nest update` re-clones.
pub(super) fn rm_rf(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%rm-rf", arg(args, 0))?;
    let under_deps = std::path::Path::new(&path)
        .components()
        .any(|c| c.as_os_str() == "_deps");
    if !under_deps {
        return Err(LispError::runtime(format!(
            "%rm-rf: refusing to delete {} — only paths under _deps/ may be removed",
            path
        ))
        .with_code(error_codes::FILE_IO));
    }
    match std::fs::remove_dir_all(&path) {
        Ok(()) => Ok(crate::core::value::kw("ok")),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(crate::core::value::kw("ok")),
        Err(e) => {
            Err(LispError::runtime(format!("%rm-rf: {}: {}", path, e))
                .with_code(error_codes::FILE_IO))
        }
    }
}
