//! Native numeric kernels through one narrow C ABI (ADR-807, feature `native-kernels`).
//!
//! A project's hot numeric code — b3d's skeleton pass, ~5 600 floats each way — can live in a
//! C-ABI library the PROJECT builds from its own tree, and be called as
//!
//! ```c
//! int32_t name(const double *in, size_t in_count, double *out, size_t out_count);
//! ```
//!
//! No Brood value crosses: the kernel reads `in_count` doubles and writes `out_count`, so it
//! needs nothing of `Value`, the heap or the GC, and nothing it does can corrupt them except
//! by writing outside `out` — which is what any native code can do, and why the two defences
//! below exist. A nonzero return is the kernel's own error code, raised as a Brood error.
//!
//! What it is not (docs/interop.md keeps WASM as THE extension ABI): it never loads a library
//! from a fetched package — a path under a `_deps` directory is refused here, whatever the
//! policy above asked (ADR-037 stays shut) — and it has no callbacks, handles, strings or
//! structs: two arrays of doubles, which is all a kernel needs and all this can get wrong.
//!
//! The defences are the grammar libraries' (ADR-660), minus vetting, which has no corpus to run
//! for a kernel — only the package knows its valid inputs:
//!
//! - **Attribution.** Every call is bracketed by a [`crate::cli_support::ForeignCode`] mark
//!   naming the library, so the fatal-signal record names it (` :kernel {…}`), and the handler
//!   writes its quarantine in the same breath.
//! - **Quarantine.** Kept by the library's SHA-256 under
//!   `$XDG_STATE_HOME/brood/kernel-verdicts`. A quarantined library is refused at load and
//!   says why; new bytes (a rebuilt kernel) are a new library. `native/forget` clears one.
//!
//! A loaded library is an opaque int token owned by the process that opened it, closed with
//! it (`close_process_kernels`), as a WASM instance is. A call is offload-safe — token, name
//! and numbers in, numbers out — so a long one runs on the ADR-144 pool.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use crate::error::LispError;
use crate::host::owned::OwnedTable;

/// The one kernel signature.
type KernelFn = unsafe extern "C" fn(*const f64, usize, *mut f64, usize) -> i32;

/// The most doubles one call may ask for back: 1 GiB of output. A count past it is a
/// mistake in the caller, refused before the allocation rather than an OOM.
const MAX_OUT_COUNT: usize = 1 << 27;

struct Kernel {
    library: libloading::Library,
    /// The crash mark of this library: leaked, since the signal handler may read it any time.
    mark: &'static crate::cli_support::ForeignCode,
    /// Each entry point looked up so far, by name.
    entries: Mutex<HashMap<String, KernelFn>>,
    path: String,
}

static REGISTRY: LazyLock<Mutex<OwnedTable<Arc<Kernel>>>> =
    LazyLock::new(|| Mutex::new(OwnedTable::default()));

/// Every mark ever built, kept reachable for the process's life (LeakSanitizer reports a
/// leaked box nothing points at), as the grammar marks are.
static MARKS: LazyLock<Mutex<Vec<&'static crate::cli_support::ForeignCode>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

fn registry() -> std::sync::MutexGuard<'static, OwnedTable<Arc<Kernel>>> {
    crate::core::sync::lock(&REGISTRY)
}

fn kernel_err(who: &str, message: impl std::fmt::Display) -> LispError {
    LispError::runtime(format!("{who}: {message}"))
}

/// `$XDG_STATE_HOME/brood/kernel-verdicts`, else `~/.local/state/brood/kernel-verdicts`.
fn verdict_dir() -> Option<PathBuf> {
    let absolute = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    absolute("XDG_STATE_HOME")
        .or_else(|| absolute("HOME").map(|home| home.join(".local").join("state")))
        .map(|base| base.join("brood").join("kernel-verdicts"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The quarantine record of the library with `hash`, if one was written.
fn quarantine_of(hash: &str) -> Option<String> {
    std::fs::read_to_string(verdict_dir()?.join(format!("{hash}.blsp"))).ok()
}

/// A path the kernel host will not load: one under a `_deps` directory, where fetched
/// packages live. Policy resolves a kernel inside the project; this holds whatever it asked.
fn fetched_package_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "_deps")
}

/// Open the kernel library at `path` (absolute, or relative to the working directory) for
/// process `owner`; the token.
pub fn open(path: &str, owner: u64) -> Result<u64, LispError> {
    let who = "%native-open";
    let canonical =
        std::fs::canonicalize(path).map_err(|e| kernel_err(who, format!("{path}: {e}")))?;
    if fetched_package_path(&canonical) {
        return Err(kernel_err(
            who,
            format!(
                "{}: a kernel library is loaded only from the project's own tree, never from a \
                 fetched package (`_deps`) — a package ships native code as a WASM component \
                 (docs/interop.md)",
                canonical.display()
            ),
        ));
    }
    let bytes = std::fs::read(&canonical).map_err(|e| kernel_err(who, format!("{path}: {e}")))?;
    let hash = sha256_hex(&bytes);
    if let Some(record) = quarantine_of(&hash) {
        return Err(kernel_err(
            who,
            format!(
                "{}: quarantined — it crashed a runtime ({}). Rebuild it (new bytes are a new \
                 library), or clear the verdict with native/forget",
                canonical.display(),
                record.trim()
            ),
        ));
    }
    let display = canonical.display().to_string();
    let about = format!(
        ":library \"{}\" :sha256 \"{hash}\"",
        crate::cli_support::literal_safe(&display, 300)
    );
    let mut mark = crate::cli_support::ForeignCode::new(format!(" :kernel {{{about}}}"));
    if let Some(dir) = verdict_dir() {
        mark = mark.with_crash_file(
            &dir.join(format!("{hash}.blsp")),
            format!(
                "{{:verdict :quarantined :by :crash :reason \"it crashed a runtime\" {about} \
                 :brood \"{}\"",
                env!("CARGO_PKG_VERSION")
            ),
            "}\n".to_string(),
        );
    }
    let mark: &'static _ = Box::leak(Box::new(mark));
    crate::core::sync::lock(&MARKS).push(mark);
    let library = {
        let _marked = crate::cli_support::enter_foreign_code(Some(mark));
        // SAFETY: loading runs the library's initialisers, native code this process did not
        // build. That is the risk this module exists to take (ADR-807): the library comes from
        // the project's own tree, is refused once quarantined, and the load is marked so a
        // crash in an initialiser names and quarantines it like a crash in a call.
        unsafe { libloading::Library::new(&canonical) }
    }
    .map_err(|e| kernel_err(who, format!("{display}: {e}")))?;
    let kernel = Arc::new(Kernel {
        library,
        mark,
        entries: Mutex::new(HashMap::new()),
        path: display,
    });
    let mut table = registry();
    let token = loop {
        let mut raw = [0u8; 8];
        getrandom::fill(&mut raw).map_err(|e| kernel_err(who, e))?;
        let token = u64::from_le_bytes(raw) & ((1u64 << 62) - 1);
        if token != 0 && !table.contains_key(&token) {
            break token;
        }
    };
    table.insert(token, owner, kernel);
    Ok(token)
}

fn kernel(who: &str, token: u64) -> Result<Arc<Kernel>, LispError> {
    registry()
        .get(&token)
        .cloned()
        .ok_or_else(|| kernel_err(who, "no such kernel library (already closed?)"))
}

/// Call entry point `name` of kernel library `token` on `input`, asking `out_count` doubles
/// back.
pub fn call(
    token: u64,
    name: &str,
    input: &[f64],
    out_count: usize,
) -> Result<Vec<f64>, LispError> {
    let who = "%native-call";
    if out_count > MAX_OUT_COUNT {
        return Err(kernel_err(
            who,
            format!("asked for {out_count} doubles back, more than {MAX_OUT_COUNT}"),
        ));
    }
    let kernel = kernel(who, token)?;
    let entry = {
        let mut entries = crate::core::sync::lock(&kernel.entries);
        match entries.get(name) {
            Some(entry) => *entry,
            None => {
                let symbol = std::ffi::CString::new(name)
                    .map_err(|_| kernel_err(who, format!("`{name}` is not a symbol name")))?;
                // SAFETY: the symbol is read as the one kernel signature, which is the contract
                // a kernel library is built to (ADR-807); `kernel` (and so the library) outlives
                // every use of the pointer, held by this call's `Arc`.
                let entry: KernelFn = unsafe {
                    *kernel
                        .library
                        .get::<KernelFn>(symbol.as_bytes_with_nul())
                        .map_err(|e| kernel_err(who, format!("`{name}` in {}: {e}", kernel.path)))?
                };
                entries.insert(name.to_string(), entry);
                entry
            }
        }
    };
    let mut out = vec![0.0f64; out_count];
    let status = {
        let _marked = crate::cli_support::enter_foreign_code(Some(kernel.mark));
        // SAFETY: `input` is `input.len()` readable doubles and `out` `out_count` writable
        // ones, the counts passed beside them; the kernel may touch nothing else. One that does
        // is native code misbehaving, which the mark attributes and quarantines.
        unsafe { entry(input.as_ptr(), input.len(), out.as_mut_ptr(), out_count) }
    };
    if status != 0 {
        return Err(kernel_err(
            who,
            format!("`{name}` in {} returned {status}", kernel.path),
        ));
    }
    Ok(out)
}

/// Close kernel library `token`. The library is unloaded once no call still holds it.
pub fn close(token: u64) -> Result<(), LispError> {
    registry()
        .remove(&token)
        .map(drop)
        .ok_or_else(|| kernel_err("%native-close", "no such kernel library (already closed?)"))
}

/// Close every kernel library process `pid` still holds — the scheduler's retirement sweep,
/// beside the WASM instances'.
pub fn close_process_kernels(pid: u64) {
    let closed = registry().remove_owned_by(pid);
    drop(closed);
}

/// Clear the quarantine of the library at `path`; true when there was one.
pub fn forget(path: &str) -> Result<bool, LispError> {
    let bytes =
        std::fs::read(path).map_err(|e| kernel_err("%native-forget", format!("{path}: {e}")))?;
    let Some(dir) = verdict_dir() else {
        return Ok(false);
    };
    Ok(std::fs::remove_file(dir.join(format!("{}.blsp", sha256_hex(&bytes)))).is_ok())
}
