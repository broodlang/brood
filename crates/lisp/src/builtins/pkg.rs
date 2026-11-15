//! Package-manager delivery *mechanism* (ADR-037): the `git` and `tar` shells the
//! fetcher drives, plus the `_deps/`-bounded cache eviction. Rust runs the
//! subprocesses and enforces the filesystem guardrail; the cache layout,
//! ref-pinning, sha256 verification, and when-to-reclone *policy* are Brood in
//! `std/tool/package.blsp`. `git`/`tar` are assumed on PATH — the same
//! external-tool tradeoff both these primitives make.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult, error_codes};

use super::args::{arg, expect_int, expect_string};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // The package manager's git mechanism (ADR-037): resolve a ref to a commit,
    // and clone+checkout a pinned commit. Thin shell-outs to `git`; the cache
    // layout / lock file / conflict policy are all Brood (std/tool/package.blsp).
    primitives.def(
        "%git-resolve-ref",
        Arity::exact(2),
        Sig::new(vec![string, string], string.union(nil_ty)),
        &["url", "ref"],
        "Resolve git `ref` (tag/branch/commit) at remote `url` to a commit hash (via `git ls-remote`), or nil if not found. The package manager's ref-pinning mechanism (ADR-037).",
        git_resolve_ref);
    primitives.def(
        "%git-clone",
        Arity::exact(4),
        Sig::new(vec![string, string, string, string], kw),
        &["url", "dest", "ref", "commit"],
        "Shallow-clone `url` into `dest` and check out the exact `commit` (detached); `ref` is the fetch fallback. Returns :ok or throws. The package manager's fetch mechanism (ADR-037).",
        git_clone);
    // The remote's published tag names, for resolving a git dep's `:version` range
    // to a concrete tag (ADR-209). List-of-strings, nil when the remote has no tags.
    primitives.def(
        "%git-list-tags",
        Arity::exact(1),
        Sig::new(vec![string], pair.union(nil_ty)),
        &["url"],
        "The tag names published by the remote at `url`, as a list of strings (via `git ls-remote --tags --refs`); nil when the remote has no tags. Backs resolving a git dep's `:version` range to the newest matching tag (ADR-209).",
        git_list_tags);
    // Files not committed-clean under a dir (modified/staged/untracked), for a
    // git-aware `nest format --changed` narrower scope. nil if not a git repo.
    primitives.def(
        "%git-changed-files",
        Arity::exact(1),
        Sig::new(vec![string], pair.union(nil_ty).union(kw)),
        &["dir"],
        "Absolute paths of files NOT committed-clean under `dir` (modified, staged, or untracked — the union `git status --porcelain` reports). Returns a list of strings (nil when the tree is clean — an empty list is nil), or the keyword :not-a-repo when `dir` is not inside a git work tree. Backs `nest format --changed`.",
        git_changed_files);
    // Extract a gzip'd tar archive into a dir, stripping N leading path components.
    // The tarball source-delivery mechanism (ADR-037 tarball deps); shells to `tar`.
    primitives.def(
        "%untar-gz",
        Arity::exact(3),
        Sig::new(vec![string, string, int], kw),
        &["archive", "dest", "strip"],
        "Extract a gzip'd tar `archive` into `dest`, stripping `strip` leading path components (package convention: 1). Shells to `tar`. Returns :ok or throws. The tarball-dep delivery mechanism (ADR-037).",
        untar_gz);
    // Delete a cached dependency tree. Bounded to paths under `_deps/` — refuses
    // anything else, so a mis-pathed `nest update` can't rm the wrong directory.
    primitives.def(
        "%rm-rf",
        Arity::exact(1),
        Sig::new(vec![string], kw),
        &["path"],
        "Recursively delete `path`. Bounded to paths strictly inside a `_deps/` directory with no `..` component (refuses anything else). Idempotent. The package manager's cache-eviction mechanism (ADR-037).",
        rm_rf);
}

/// Reject a git *operand* (a URL, ref, or commit) that git's own option parser
/// would read as an OPTION rather than a value.
///
/// These strings arrive from a manifest's `:git` dependency — including a
/// **transitive** dependency's manifest, which the user never reads — and land in
/// argv positions git parses itself. A dependency URL of
/// `--upload-pack=touch /tmp/pwned; git-upload-pack` makes `git ls-remote` run that
/// shell command, so `nest fetch` alone (no package code executed, no build script)
/// is arbitrary code execution. Verified: the file appeared.
///
/// Nothing legitimate is lost by refusing the class outright — git refuses to
/// *create* a ref whose name starts with `-`, and no transport URL does either. The
/// check lives here, in the mechanism that spawns the process, rather than in the
/// Brood policy layer: every caller of `git` goes through these primitives, so this
/// is the one place it cannot be forgotten.
fn reject_option_like(prim: &str, what: &str, value: &str) -> Result<(), LispError> {
    if value.starts_with('-') {
        return Err(LispError::runtime(format!(
            "{prim}: refusing the {what} {value:?} — it starts with '-', which git would \
             parse as an option, not a value"
        ))
        .with_code(error_codes::SUBPROCESS_FAILED)
        .with_hint("a git URL or ref never starts with '-'; check the manifest that declares it"));
    }
    Ok(())
}

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
    out.status.success().ok_or_else(|| {
        LispError::runtime(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .with_code(error_codes::SUBPROCESS_FAILED)
    })
}

/// The commit `git ls-remote` output names for EXACTLY `ref`. A ls-remote pattern
/// matches any ref ENDING in it at a `/` boundary, so asking for `v1` also lists a
/// branch `feature/v1` (or a tag `old/v1`); taking whatever line came first or last
/// pinned a dependency to the wrong commit. Precedence, first hit wins: the tag
/// `refs/tags/<ref>` — its peeled `^{}` line first, the commit an annotated tag points
/// to — then the branch `refs/heads/<ref>`, then a ref spelled in full (`HEAD`,
/// `refs/heads/main`).
fn ls_remote_exact_match<'a>(listing: &'a str, reference: &str) -> Option<&'a str> {
    let candidates = [
        format!("refs/tags/{reference}^{{}}"),
        format!("refs/tags/{reference}"),
        format!("refs/heads/{reference}"),
        format!("{reference}^{{}}"),
        reference.to_string(),
    ];
    let advertised: Vec<(&str, &str)> = listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?, fields.next()?))
        })
        .collect();
    candidates.iter().find_map(|wanted| {
        advertised
            .iter()
            .find(|(_, name)| *name == wanted.as_str())
            .map(|(sha, _)| *sha)
    })
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
    reject_option_like("%git-resolve-ref", "URL", &url)?;
    reject_option_like("%git-resolve-ref", "ref", &r)?;
    // The `^{}` pattern is needed for the peeled line: a pattern matches a ref's whole
    // name, and an annotated tag's peeled entry is NAMED `refs/tags/<ref>^{}`, so asking
    // for `<ref>` alone never lists it (and the tag OBJECT's sha was pinned instead).
    let peeled_pattern = format!("{r}^{{}}");
    let out = run_git(&["ls-remote", &url, &r, &peeled_pattern], None)?;
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
    if let Some(sha) = ls_remote_exact_match(&stdout, &r) {
        return Ok(heap.alloc_string(sha));
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
    reject_option_like("%git-list-tags", "URL", &url)?;
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
    // Every one of these lands in an argv slot git parses; see `reject_option_like`.
    reject_option_like("%git-clone", "URL", &url)?;
    reject_option_like("%git-clone", "destination", &dest)?;
    reject_option_like("%git-clone", "ref", &gref)?;
    reject_option_like("%git-clone", "commit", &commit)?;

    if let Some(parent) = std::path::Path::new(&dest).parent()
        && !parent.is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            LispError::runtime(format!(
                "%git-clone: cannot create {}: {}",
                parent.display(),
                e
            ))
            .with_code(error_codes::FILE_IO)
        })?;
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
/// extract runs on the dirty-native pool. An archive with an absolute, `..` or link
/// member is refused before anything is written (see [`reject_unsafe_members`]), and
/// ownership is never restored from the archive. Returns `:ok`. The tarball source-delivery
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
    // A relative archive name with a `:` before any `/` is a REMOTE archive to GNU tar
    // (`host:file`); anchoring it with `./` keeps it local without the GNU-only
    // `--force-local`.
    let archive_path = if archive.starts_with('/') {
        archive.clone()
    } else {
        format!("./{archive}")
    };
    reject_unsafe_members(&archive_path)?;
    std::fs::create_dir_all(&dest).map_err(|error| {
        LispError::runtime(format!("%untar-gz: cannot create {dest}: {error}"))
            .with_code(error_codes::FILE_IO)
    })?;
    let strip_argument = format!("--strip-components={strip}");
    let output = run_tar(&[
        "-xzf",
        &archive_path,
        "-C",
        &dest,
        &strip_argument,
        "--no-same-owner",
    ])?;
    if output.status.success() {
        Ok(crate::core::value::kw("ok"))
    } else {
        Err(LispError::runtime(format!(
            "%untar-gz: extracting {} failed: {}",
            archive,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .with_code(error_codes::SUBPROCESS_FAILED))
    }
}

fn run_tar(arguments: &[&str]) -> Result<std::process::Output, LispError> {
    std::process::Command::new("tar")
        .args(arguments)
        .output()
        .map_err(|error| {
            LispError::runtime(format!("%untar-gz: tar: {error}"))
                .with_code(error_codes::SUBPROCESS_FAILED)
                .with_hint("is `tar` installed and on PATH?")
        })
}

/// Refuse an archive any member of which could land outside the destination: an
/// absolute path, a `..` component, or a link (a symlink or hard link can point
/// anywhere, and a later member written through it goes there). A package tarball
/// needs none of them. Checked BEFORE anything is extracted.
fn reject_unsafe_members(archive_path: &str) -> Result<(), LispError> {
    let listing = |verbose: bool| -> Result<String, LispError> {
        let output = run_tar(&[if verbose { "-tvzf" } else { "-tzf" }, archive_path])?;
        if !output.status.success() {
            return Err(LispError::runtime(format!(
                "%untar-gz: listing {archive_path} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
            .with_code(error_codes::SUBPROCESS_FAILED));
        }
        Ok(String::from_utf8_lossy_owned(output.stdout))
    };
    let refuse = |member: &str, why: &str| {
        Err(LispError::runtime(format!(
            "%untar-gz: refusing {archive_path}: member {member:?} {why}"
        ))
        .with_code(error_codes::FILE_IO))
    };
    for member in listing(false)?.lines() {
        if member.starts_with('/') {
            return refuse(member, "is an absolute path");
        }
        if member.split('/').any(|component| component == "..") {
            return refuse(member, "escapes the destination with `..`");
        }
    }
    // The verbose listing's first column is an `ls`-style mode, whose first character is
    // the entry type — the same in GNU tar and bsdtar.
    for line in listing(true)?.lines() {
        match line.chars().next() {
            Some('l') => return refuse(line, "is a symbolic link"),
            Some('h') => return refuse(line, "is a hard link"),
            _ => {}
        }
    }
    Ok(())
}

/// Whether `path` names something STRICTLY inside a `_deps` directory, judged lexically:
/// a `..` component anywhere refuses outright (`root/_deps/..` is the project itself, and
/// `_deps/x/../../y` anything at all), `.` components are ignored, and some `_deps`
/// component must be followed by at least one more — `_deps` itself is not removable.
fn rm_rf_target_inside_deps(path: &str) -> bool {
    use std::path::Component;
    let mut seen_deps = false;
    let mut inside = false;
    for component in std::path::Path::new(path).components() {
        match component {
            Component::ParentDir => return false,
            Component::CurDir => {}
            Component::Normal(name) => {
                inside = seen_deps;
                if name == "_deps" {
                    seen_deps = true;
                }
            }
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    inside
}

/// `(%rm-rf path)` — recursively delete `path`. **Bounded to `_deps/`**: refuses any
/// path that is not strictly inside a `_deps` directory, or that has a `..` component
/// (see `rm_rf_target_inside_deps`), so a mis-computed or hostile cache path — a
/// dependency named `..` — can't delete something outside the package cache. Idempotent
/// (`:ok` if already absent). The package manager's cache-eviction mechanism (ADR-037);
/// `nest update` re-clones.
pub(super) fn rm_rf(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%rm-rf", arg(args, 0))?;
    if !rm_rf_target_inside_deps(&path) {
        return Err(LispError::runtime(format!(
            "%rm-rf: refusing to delete {} — only paths strictly inside a _deps/ directory, \
             with no `..` component, may be removed",
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

#[cfg(test)]
mod tests {
    use crate::Interp;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// A fresh scratch directory for one test, removed first if a previous run left it.
    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("brood-untar-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// `tar -czf archive -C directory [extra…] members…`, keeping member names verbatim.
    fn make_archive(archive: &Path, directory: &Path, members: &[&str]) {
        let status = Command::new("tar")
            .arg("-czPf")
            .arg(archive)
            .arg("-C")
            .arg(directory)
            .args(members)
            .status()
            .unwrap();
        assert!(status.success(), "building the test archive failed");
    }

    fn untar(archive: &Path, destination: &Path) -> Result<String, String> {
        let mut interp = Interp::new();
        let form = format!(
            "(%untar-gz \"{}\" \"{}\" 0)",
            archive.display(),
            destination.display()
        );
        interp
            .eval_str(&form)
            .map(|value| interp.print(value))
            .map_err(|error| error.to_string())
    }

    #[test]
    fn untar_gz_extracts_a_plain_archive() {
        let root = scratch("plain");
        std::fs::create_dir_all(root.join("source/pkg")).unwrap();
        std::fs::write(root.join("source/pkg/main.blsp"), "(def x 1)\n").unwrap();
        let archive = root.join("plain.tar.gz");
        make_archive(&archive, &root.join("source"), &["pkg"]);
        let destination = root.join("out");
        assert_eq!(untar(&archive, &destination).unwrap(), ":ok");
        assert!(destination.join("pkg/main.blsp").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn untar_gz_refuses_a_member_that_climbs_out_with_dot_dot() {
        let root = scratch("dotdot");
        std::fs::create_dir_all(root.join("source/inner")).unwrap();
        std::fs::write(root.join("source/escaped.txt"), "owned\n").unwrap();
        let archive = root.join("evil.tar.gz");
        make_archive(&archive, &root.join("source/inner"), &["../escaped.txt"]);
        let destination = root.join("deep/out");
        let error = untar(&archive, &destination).unwrap_err();
        assert!(error.contains("refusing"), "{error}");
        assert!(
            !root.join("deep/escaped.txt").exists(),
            "the member escaped"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn untar_gz_refuses_an_absolute_member() {
        let root = scratch("absolute");
        let target = root.join("absolute-target.txt");
        std::fs::write(&target, "owned\n").unwrap();
        let archive = root.join("evil.tar.gz");
        make_archive(&archive, &root, &[target.to_str().unwrap()]);
        std::fs::remove_file(&target).unwrap();
        let error = untar(&archive, &root.join("out")).unwrap_err();
        assert!(error.contains("absolute"), "{error}");
        assert!(!target.exists(), "the absolute member was written");
        let _ = std::fs::remove_dir_all(&root);
    }

    // A symlink needs a privilege on Windows that a test run does not have.
    #[cfg(unix)]
    #[test]
    fn untar_gz_refuses_a_symlink_member() {
        let root = scratch("symlink");
        std::fs::create_dir_all(root.join("source")).unwrap();
        std::fs::create_dir_all(root.join("outside")).unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("source/link")).unwrap();
        let archive = root.join("evil.tar.gz");
        make_archive(&archive, &root.join("source"), &["link"]);
        let destination = root.join("out");
        let error = untar(&archive, &destination).unwrap_err();
        assert!(error.contains("symbolic link"), "{error}");
        assert!(!destination.join("link").exists(), "the link was extracted");
        let _ = std::fs::remove_dir_all(&root);
    }
}
