//! Single-binary app bundling — the "ship a Brood app as one executable" path
//! (ADR-038). `nest release` appends an archive of the project's source to a
//! copy of the prebuilt `brood` binary; at startup `brood` reads its own path,
//! detects the footer, and boots the app's `:main` instead of starting a REPL.
//!
//! Wire format: `[ base brood binary ][ archive ][ 20-byte footer ]`.
//!
//! ```text
//! footer (20 bytes): magic b"BRDBNDL1" (8) | format-version u32-LE (4) | archive-len u64-LE (8)
//! archive:           u32-LE manifest-len, manifest bytes,
//!                    u32-LE module-count, then per module: u32-LE name-len, name, u32-LE src-len, src
//! ```
//!
//! The footer's archive-len is `u64` while every internal length (manifest,
//! module count, name, src) is `u32`: a single archive could in principle exceed
//! 4 GiB even though no individual source field will, so the offset that locates
//! the whole archive needs the wider type — the `u32`s deliberately cap each
//! component well below that.
//!
//! Appended trailing bytes don't disturb the ELF/PE/Mach-O loader (the classic
//! self-extracting-archive trick). The footer is read last-bytes-first; its
//! magic disambiguates a release binary from a plain `brood`. Everything is
//! code-only — the manifest plus each module's source — no runtime asset files
//! (decision recorded with ADR-038's implementation).

use std::path::Path;
use std::sync::OnceLock;

/// The `--brood-` argument namespace is **reserved by the runtime**: a bundle honours
/// the two names below as its first argument and hands every other argument, including
/// any other `--brood-…`, straight to the app's `:main`.
///
/// Reserving a prefix rather than the bare `--build-info` / `--boot-check` is the whole
/// design: the bundle's contract is that argv belongs to the app, and an exception that
/// costs the app a name it might want is an exception the app has to know about. This one
/// costs it nothing.
///
/// Print the bundle's build identity — which brood, which features, which app — and exit
/// 0. Answering that meant grepping the binary over SSH, and the first attempt used
/// `strings`, absent from `debian:bookworm-slim`, which reported nothing and read exactly
/// like "no JIT".
pub const BUNDLE_BUILD_INFO_ARG: &str = "--brood-build-info";
/// Load the bundle's embedded modules, resolve `:main`, run **nothing**, exit 0/1 — the
/// boot check `nest release --smoke` runs against each binary it writes (KI-66).
pub const BUNDLE_BOOT_CHECK_ARG: &str = "--brood-boot-check";

/// Which reserved command, if any, `args` (a bundle's argv minus argv[0]) asks for.
///
/// The whole contract in one testable place: recognized **only as the first argument**,
/// and **only** these two spellings. Anywhere else — second position, or any other
/// `--brood-…` name — the argument belongs to the app and is passed through untouched,
/// so a bundle can neither swallow an app's argument nor grow a second meaning for one.
pub fn reserved_command(args: &[String]) -> Option<&'static str> {
    match args.first().map(String::as_str) {
        Some(BUNDLE_BUILD_INFO_ARG) => Some(BUNDLE_BUILD_INFO_ARG),
        Some(BUNDLE_BOOT_CHECK_ARG) => Some(BUNDLE_BOOT_CHECK_ARG),
        _ => None,
    }
}

/// Footer magic — 8 bytes, the trailing digit doubling as the format version so
/// a stale `nest` writing v1 against a `brood` expecting v2 is detectable.
const MAGIC: &[u8; 8] = b"BRDBNDL1";
const FORMAT_VERSION: u32 = 1;
/// magic(8) + version(4) + archive_len(8).
const FOOTER_LEN: usize = 8 + 4 + 8;

/// The sources embedded in a release binary: the project manifest plus every
/// module keyed by filename stem (e.g. `"main"`). Code-only.
#[derive(Debug, PartialEq, Eq)]
pub struct Bundle {
    pub manifest: String,
    pub modules: Vec<(String, String)>,
}

impl Bundle {
    /// The source of module `name` (filename stem), if present.
    pub fn module_src(&self, name: &str) -> Option<&str> {
        self.modules
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s.as_str())
    }

    /// Every module name (stem), in archive order.
    pub fn module_names(&self) -> impl Iterator<Item = &str> {
        self.modules.iter().map(|(n, _)| n.as_str())
    }
}

/// Serialize the archive body (no footer) — the inverse of [`parse_archive`].
pub fn serialize(manifest: &str, modules: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    put_lp(&mut out, manifest.as_bytes());
    out.extend_from_slice(&(modules.len() as u32).to_le_bytes());
    for (name, src) in modules {
        put_lp(&mut out, name.as_bytes());
        put_lp(&mut out, src.as_bytes());
    }
    out
}

/// Append a `u32-LE` length prefix followed by the bytes.
fn put_lp(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b);
}

/// A forward-only reader over the archive bytes. Every accessor returns `None`
/// on truncation, so a corrupt/foreign trailer degrades to "not a bundle"
/// rather than panicking.
struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take_u32(&mut self) -> Option<u32> {
        let end = self.pos.checked_add(4)?;
        let n = u32::from_le_bytes(self.b.get(self.pos..end)?.try_into().ok()?);
        self.pos = end;
        Some(n)
    }

    /// A length-prefixed byte run.
    fn take_lp(&mut self) -> Option<&'a [u8]> {
        let len = self.take_u32()? as usize;
        let end = self.pos.checked_add(len)?;
        let out = self.b.get(self.pos..end)?;
        self.pos = end;
        Some(out)
    }
}

/// Parse the archive body into a [`Bundle`], or `None` if malformed.
fn parse_archive(bytes: &[u8]) -> Option<Bundle> {
    let mut c = Cursor { b: bytes, pos: 0 };
    let manifest = String::from_utf8(c.take_lp()?.to_vec()).ok()?;
    let count = c.take_u32()? as usize;
    // `count` comes from untrusted file bytes — a corrupt trailer can claim a
    // huge module count. The smallest possible module is two empty length-prefixed
    // runs (name + src = 8 bytes), so cap the pre-allocation at what the remaining
    // bytes could actually hold; the loop still bails via `take_lp` on truncation.
    let cap = count.min(bytes.len() / 8);
    let mut modules = Vec::with_capacity(cap);
    for _ in 0..count {
        let name = String::from_utf8(c.take_lp()?.to_vec()).ok()?;
        let src = String::from_utf8(c.take_lp()?.to_vec()).ok()?;
        modules.push((name, src));
    }
    Some(Bundle { manifest, modules })
}

/// Decode the 20 footer bytes: check magic + format version, return the archive
/// length. `None` if the magic/version don't match (i.e. not one of our bundles).
/// Shared by the slice-based [`footer`] and the seek/read-based [`mounted`].
fn decode_footer(foot: &[u8; FOOTER_LEN]) -> Option<u64> {
    if &foot[0..8] != MAGIC {
        return None;
    }
    if u32::from_le_bytes(foot[8..12].try_into().ok()?) != FORMAT_VERSION {
        return None;
    }
    Some(u64::from_le_bytes(foot[12..20].try_into().ok()?))
}

/// If `bytes` ends with a valid footer, return `(archive_start, archive_len)`.
/// `None` means this is a plain (non-release) binary.
fn footer(bytes: &[u8]) -> Option<(usize, usize)> {
    if bytes.len() < FOOTER_LEN {
        return None;
    }
    let foot: &[u8; FOOTER_LEN] = bytes[bytes.len() - FOOTER_LEN..].try_into().ok()?;
    // `try_from`, not `as`: the footer's archive-len is a `u64` read out of untrusted
    // file bytes, and `as usize` TRUNCATES on a 32-bit target (wasm32 builds this
    // module). A crafted `alen` of `2^32 + 30` would narrow to 30, sail past the
    // `bytes.len() < total` guard, and report a bogus archive window — the guard has to
    // see the value the footer actually claimed. On 64-bit this conversion never fails.
    let alen = usize::try_from(decode_footer(foot)?).ok()?;
    let total = FOOTER_LEN.checked_add(alen)?;
    if bytes.len() < total {
        return None;
    }
    Some((bytes.len() - total, alen))
}

/// The bundle embedded in *this* executable, read once from `current_exe()`.
/// `&None` for a plain `brood`/`nest` (the common case) — no behaviour change.
///
/// The not-a-bundle case reads only the 20-byte footer (not the whole multi-MB
/// binary): a plain `nest run` reaches here on every non-std `require`, so the
/// common path must stay cheap. Only a real bundle reads its archive bytes.
pub fn mounted() -> &'static Option<Bundle> {
    static MOUNTED: OnceLock<Option<Bundle>> = OnceLock::new();
    MOUNTED.get_or_init(|| {
        use std::io::{Read, Seek, SeekFrom};
        let exe = std::env::current_exe().ok()?;
        let mut f = std::fs::File::open(exe).ok()?;
        let len = f.metadata().ok()?.len();
        if len < FOOTER_LEN as u64 {
            return None;
        }
        // Footer first — 20 bytes off the end decides bundle-or-not.
        f.seek(SeekFrom::Start(len - FOOTER_LEN as u64)).ok()?;
        let mut foot = [0u8; FOOTER_LEN];
        f.read_exact(&mut foot).ok()?;
        let alen = decode_footer(&foot)?;
        // `alen` is attacker-controlled (the last 8 footer bytes of a file handed to
        // us). Compute the total with `checked_add` and guard `len < total` BEFORE the
        // subtraction — exactly as `footer()` does — so a crafted `alen` near `u64::MAX`
        // degrades to "not a bundle" instead of overflowing the add / underflowing the
        // sub (a panic under debug-assertions, a wrapped giant seek + ~16 EiB `vec!`
        // capacity-overflow panic in release). The module contract is graceful, not panic.
        let total = (FOOTER_LEN as u64).checked_add(alen)?;
        if len < total {
            return None;
        }
        // Real bundle — read just the archive bytes (not the base binary).
        f.seek(SeekFrom::Start(len - total)).ok()?;
        // `try_from` for the same reason as in `footer()`: `as usize` would truncate a
        // >4 GiB claim on a 32-bit target and read the wrong window instead of bailing.
        let mut archive = vec![0u8; usize::try_from(alen).ok()?];
        f.read_exact(&mut archive).ok()?;
        parse_archive(&archive)
    })
}

/// Whether this executable is a release bundle (an app), not a plain runtime.
pub fn is_bundled() -> bool {
    mounted().is_some()
}

/// If `bytes` is itself a release binary, return just the base (everything
/// before the appended archive + footer); otherwise return `bytes` unchanged.
/// Makes `nest release` idempotent: releasing from an already-released `brood`
/// strips the old payload instead of nesting a second archive.
pub fn strip_existing(bytes: &[u8]) -> &[u8] {
    match footer(bytes) {
        Some((start, _)) => &bytes[..start],
        None => bytes,
    }
}

/// Write a release binary: strip any existing payload off `base`, append
/// `archive` + footer, write `out`, and make it executable (unix).
pub fn write_release(base: &[u8], archive: &[u8], out: &Path) -> std::io::Result<()> {
    let base = strip_existing(base);
    let mut buf = Vec::with_capacity(base.len() + archive.len() + FOOTER_LEN);
    buf.extend_from_slice(base);
    buf.extend_from_slice(archive);
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    buf.extend_from_slice(&(archive.len() as u64).to_le_bytes());
    std::fs::write(out, &buf)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(out)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(out, perms)?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    /// Build a fake `[base][archive][footer]` image in memory, the way
    /// `write_release` lays it out, without touching the filesystem.
    fn fake_release(base: &[u8], manifest: &str, modules: &[(String, String)]) -> Vec<u8> {
        let archive = serialize(manifest, modules);
        let mut file = base.to_vec();
        file.extend_from_slice(&archive);
        file.extend_from_slice(MAGIC);
        file.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        file.extend_from_slice(&(archive.len() as u64).to_le_bytes());
        file
    }

    #[test]
    fn serialize_parse_round_trips() {
        let manifest = "(project :name foo :main app)";
        let modules = vec![
            ("app".to_string(), "(defn main () 1)".to_string()),
            ("util".to_string(), "(defn helper () 2)".to_string()),
        ];
        let file = fake_release(b"FAKE-BROOD-BINARY", manifest, &modules);
        let (start, alen) = footer(&file).expect("footer present");
        let bundle = parse_archive(&file[start..start + alen]).expect("parse");
        assert_eq!(bundle.manifest, manifest);
        assert_eq!(bundle.modules, modules);
        assert_eq!(bundle.module_src("util"), Some("(defn helper () 2)"));
        assert_eq!(bundle.module_src("absent"), None);
    }

    /// The reserved-argument contract: first position only, exact spelling only.
    /// Everything else is the app's — that is the property the bundle's whole argv
    /// design rests on, so it is asserted rather than assumed.
    #[test]
    fn reserved_commands_are_first_position_and_exact() {
        let argv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        assert_eq!(
            reserved_command(&argv(&[BUNDLE_BUILD_INFO_ARG])),
            Some(BUNDLE_BUILD_INFO_ARG)
        );
        assert_eq!(
            reserved_command(&argv(&[BUNDLE_BOOT_CHECK_ARG, "ignored"])),
            Some(BUNDLE_BOOT_CHECK_ARG)
        );

        // Not first → the app's.
        assert_eq!(
            reserved_command(&argv(&["serve", BUNDLE_BUILD_INFO_ARG])),
            None
        );
        // No args at all → run the app.
        assert_eq!(reserved_command(&[]), None);
        // The app's own similarly-named flags are untouched: reserving the
        // `--brood-` prefix is exactly what buys this.
        assert_eq!(reserved_command(&argv(&["--build-info"])), None);
        assert_eq!(reserved_command(&argv(&["--boot-check"])), None);
        // An unrecognized `--brood-…` is passed through, not swallowed as a typo'd
        // reserved word — the app may legitimately define it, and guessing would be
        // the silent-wrong-behaviour failure mode.
        assert_eq!(reserved_command(&argv(&["--brood-nonesuch"])), None);
    }

    #[test]
    fn plain_binary_is_not_a_bundle() {
        assert!(footer(b"not a bundle, just ordinary bytes here").is_none());
        assert!(footer(b"tiny").is_none());
    }

    #[test]
    fn crafted_overflow_footer_degrades_gracefully() {
        // A valid magic+version but an archive-len near u64::MAX must not overflow the
        // footer math — it degrades to "not a bundle", never a panic / 16-EiB `vec!`
        // capacity-overflow. `mounted()` shares this `checked_add`-then-guard shape.
        let mut file = b"some base binary bytes".to_vec();
        file.extend_from_slice(MAGIC);
        file.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        // alen so large that `FOOTER_LEN + alen` overflows u64.
        file.extend_from_slice(&(u64::MAX - 3).to_le_bytes());
        assert!(footer(&file).is_none());
    }

    #[test]
    fn strip_existing_recovers_base_and_is_idempotent() {
        let base = b"BASE-BINARY-BYTES";
        let file = fake_release(base, "m1", &[("a".into(), "1".into())]);
        // Strip once -> base.
        assert_eq!(strip_existing(&file), base);
        // Re-releasing from the stripped base yields the same base again.
        let rereleased = fake_release(strip_existing(&file), "m2", &[("b".into(), "2".into())]);
        assert_eq!(strip_existing(&rereleased), base);
        // A plain binary is returned untouched.
        assert_eq!(strip_existing(base), base);
    }
}

/// Robustness/fuzz surface: treat `bytes` as an untrusted candidate bundle —
/// footer detection + archive parse must return cleanly (Some/None), never
/// panic or over-allocate, on ANY input. Exercised by the `bundle` fuzz
/// target (`crates/lisp/fuzz/fuzz_targets/bundle.rs`).
#[doc(hidden)]
pub fn fuzz_parse(bytes: &[u8]) {
    let _ = strip_existing(bytes);
    if let Some((start, len)) = footer(bytes) {
        if let Some(archive) = bytes.get(start..start + len) {
            let _ = parse_archive(archive);
        }
    }
}
