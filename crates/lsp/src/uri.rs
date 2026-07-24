//! `file://` URI ↔ filesystem path conversion (extracted from main.rs).
use lsp_types::Uri;
use std::path::PathBuf;

/// Extract the filesystem path from a `file://` URI. Percent-decodes the path
/// so an editor URI for `/home/Wilhelm Kirschbaum/proj/` (`%20`-escaped) maps
/// back to the real on-disk path — without this, `find_project_root` silently
/// failed for any path containing whitespace or non-ASCII bytes. A non-`file:`
/// URI returns `None` so callers skip project work.
///
/// Handles both `file:///abs/path` (empty authority — the common form) and
/// `file://host/abs/path` (some WSL / remote clients): the authority component
/// is dropped and the path taken from its leading `/`. Without this, a
/// host-bearing URI decoded to a *relative* path (`host/abs/path`) and project
/// bootstrap silently never fired.
pub(crate) fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    // Empty authority → `rest` already starts at the path's `/`. A non-empty
    // authority (a host) precedes the first `/`; the path begins there.
    let path = if rest.starts_with('/') {
        rest
    } else {
        &rest[rest.find('/')?..]
    };
    Some(PathBuf::from(percent_decode(path)))
}

/// Build a `file://` URI from an absolute filesystem path — the inverse of
/// [`uri_to_path`], for the cross-file `Location`s goto-definition returns.
/// Percent-encodes every byte outside the URI "unreserved" set (plus `/`), so
/// spaces and non-ASCII path components round-trip. `None` if the result somehow
/// doesn't parse as a URI (it always should for an absolute path).
pub(crate) fn path_to_uri(path: &str) -> Option<Uri> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::from("file://");
    for &b in path.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                s.push(b as char)
            }
            _ => {
                s.push('%');
                s.push(HEX[(b >> 4) as usize] as char);
                s.push(HEX[(b & 0xf) as usize] as char);
            }
        }
    }
    s.parse().ok()
}

/// Tiny `%`-decoder for the path portion of a `file://` URI — no allocation
/// unless the path actually contains a `%`. Invalid escapes (`%XY` with
/// non-hex digits, or a trailing `%`) pass through literally rather than
/// returning an error: the caller's failure mode (`exists()` returns false)
/// is already the right one for a path we can't make sense of.
fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    // `from_utf8_lossy` for the path-with-replacement-char fallback; the OS
    // won't accept a malformed-utf8 path anyway, and `String` is the public
    // shape `PathBuf::from` takes.
    String::from_utf8_lossy(&out).into_owned()
}
