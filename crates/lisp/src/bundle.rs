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
//! Appended trailing bytes don't disturb the ELF/PE/Mach-O loader (the classic
//! self-extracting-archive trick). The footer is read last-bytes-first; its
//! magic disambiguates a release binary from a plain `brood`. Everything is
//! code-only — the manifest plus each module's source — no runtime asset files
//! (decision recorded with ADR-038's implementation).

use std::path::Path;
use std::sync::OnceLock;

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
    let mut modules = Vec::with_capacity(count);
    for _ in 0..count {
        let name = String::from_utf8(c.take_lp()?.to_vec()).ok()?;
        let src = String::from_utf8(c.take_lp()?.to_vec()).ok()?;
        modules.push((name, src));
    }
    Some(Bundle { manifest, modules })
}

/// If `bytes` ends with a valid footer, return `(archive_start, archive_len)`.
/// `None` means this is a plain (non-release) binary.
fn footer(bytes: &[u8]) -> Option<(usize, usize)> {
    if bytes.len() < FOOTER_LEN {
        return None;
    }
    let foot = &bytes[bytes.len() - FOOTER_LEN..];
    if &foot[0..8] != MAGIC {
        return None;
    }
    if u32::from_le_bytes(foot[8..12].try_into().ok()?) != FORMAT_VERSION {
        return None;
    }
    let alen = u64::from_le_bytes(foot[12..20].try_into().ok()?) as usize;
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
        if &foot[0..8] != MAGIC || u32::from_le_bytes(foot[8..12].try_into().ok()?) != FORMAT_VERSION
        {
            return None;
        }
        let alen = u64::from_le_bytes(foot[12..20].try_into().ok()?);
        if len < FOOTER_LEN as u64 + alen {
            return None;
        }
        // Real bundle — read just the archive bytes (not the base binary).
        f.seek(SeekFrom::Start(len - FOOTER_LEN as u64 - alen)).ok()?;
        let mut archive = vec![0u8; alen as usize];
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

    #[test]
    fn plain_binary_is_not_a_bundle() {
        assert!(footer(b"not a bundle, just ordinary bytes here").is_none());
        assert!(footer(b"tiny").is_none());
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
