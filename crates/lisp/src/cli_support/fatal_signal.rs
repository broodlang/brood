//! The **fatal-signal record**: what a runtime that dies on SIGSEGV, SIGBUS, SIGILL, SIGFPE
//! or SIGABRT (a Rust panic that aborts, a failed allocation, a stack overflow) leaves for
//! the next start to find.
//!
//! The panic hook ([`super::install_crash_dump`]) cannot see these: a signal is not a panic,
//! and a panic that aborts dies after its hook ran, by SIGABRT. Before this, a native crash
//! left no trace at all unless a terminal kept stderr.
//!
//! A signal handler may only do what is async-signal-safe, so everything that allocates or
//! formats a string is done at startup ([`install`]): the record's path, the directories to
//! make and the fields that do not change (version, pid, argv) are prepared into a
//! record published by one pointer, and the handler only reads it, formats integers into a buffer on its own
//! stack, and calls `mkdir`, `open`, `write`, `close`, `sigaction` and `raise` — all on
//! POSIX's async-signal-safe list. Nothing is created on disk unless the runtime crashes.
//!
//! The record is one Brood map literal in `$XDG_STATE_HOME/brood/crashes/crash-<pid>.blsp`
//! (else `~/.local/state/brood/crashes`), so the next start reads it with the reader:
//! `crash-report/previous-crashes`. The handler then gives the signal back to whatever handled
//! it before — Rust's stack-overflow handler for a fault, the default action otherwise — so
//! the process still dies as it would have, with the same exit status and core dump.

/// The directory crash records go to: `$XDG_STATE_HOME/brood/crashes`, else
/// `$HOME/.local/state/brood/crashes` — the directory [`install`] chose at startup, so a
/// program that changed its environment since still finds it. `None` before [`install`]
/// ran, off Unix, and when neither variable gives an absolute path.
pub fn crash_record_dir() -> Option<String> {
    #[cfg(unix)]
    {
        unix::prepared().map(|prepared| prepared.dir.clone())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Keep the last panic's message, so a panic that goes on to abort the runtime is named in
/// the record SIGABRT writes. Called from the panic hook, never from a signal handler.
pub fn note_panic(message: &str) {
    #[cfg(unix)]
    unix::note_panic(message);
    #[cfg(not(unix))]
    let _ = message;
}

/// Native code this thread is running that is not the runtime's own — a tree-sitter grammar
/// loaded from a library (ADR-660) — named so a crash inside it says whose code it was.
///
/// Built once per library, before its code first runs, and leaked: the handler can only read
/// what already exists. `record_fragment` is spliced into the crash record's map (` :grammar
/// {…}`); the optional crash file is written beside the record, so the next start can act on
/// it without reading crash records (a grammar quarantined by the crash that it caused).
pub struct ForeignCode {
    record_fragment: Vec<u8>,
    crash_file: Option<CrashFile>,
}

/// A path as the crash handler hands it to the OS, made beforehand: a C string for `open` on
/// Unix, a NUL-terminated UTF-16 string for `CreateFileW` on Windows.
#[cfg(not(windows))]
type NativePath = std::ffi::CString;
#[cfg(windows)]
type NativePath = Vec<u16>;

#[cfg(not(windows))]
fn native_path(path: &std::path::Path) -> Option<NativePath> {
    std::ffi::CString::new(path.to_string_lossy().into_owned().into_bytes()).ok()
}

#[cfg(windows)]
fn native_path(path: &std::path::Path) -> Option<NativePath> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    (!wide.contains(&0)).then(|| wide.into_iter().chain([0]).collect())
}

/// What [`ForeignCode::with_crash_file`] asks the handler to write.
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
struct CrashFile {
    /// Each directory to make before opening the file, outermost first.
    mkdirs: Vec<NativePath>,
    file: NativePath,
    /// Written before the signal's own fields, and after them.
    head: Vec<u8>,
    tail: Vec<u8>,
}

/// The longest fragment a crash record carries: the record is a fixed buffer on the signal
/// stack, and a fragment that did not fit would cut the map before its closing brace.
const FOREIGN_FRAGMENT_MAX: usize = 768;

impl ForeignCode {
    /// Code named by `record_fragment`: text inside a Brood map, starting with a space (` :grammar
    /// {:lang :json}`). One longer than [`FOREIGN_FRAGMENT_MAX`] is dropped, not cut.
    pub fn new(record_fragment: String) -> ForeignCode {
        let record_fragment = if record_fragment.len() <= FOREIGN_FRAGMENT_MAX {
            record_fragment.into_bytes()
        } else {
            Vec::new()
        };
        ForeignCode {
            record_fragment,
            crash_file: None,
        }
    }

    /// Also write `path` when the runtime dies in this code: `head`, then ` :signal "SIGSEGV"
    /// :time <ms> :pid <pid>`, then `tail` — so `head` opens a map literal and `tail` closes it.
    /// The directories above `path` are made first. Ignored when `path` cannot be a C string.
    pub fn with_crash_file(mut self, path: &std::path::Path, head: String, tail: String) -> Self {
        if let Some(file) = native_path(path) {
            let mut mkdirs: Vec<NativePath> = path
                .parent()
                .map(|dir| dir.ancestors().filter_map(native_path).collect())
                .unwrap_or_default();
            mkdirs.reverse();
            self.crash_file = Some(CrashFile {
                mkdirs,
                file,
                head: head.into_bytes(),
                tail: tail.into_bytes(),
            });
        }
        self
    }

    /// The fragment a crash record gets while this code runs.
    pub fn record_fragment(&self) -> &[u8] {
        &self.record_fragment
    }
}

thread_local! {
    /// The foreign code this thread is in, or null. Const-initialised and without a destructor,
    /// so reading it is a plain thread-local load — what lets the signal handler read it.
    static FOREIGN: std::cell::Cell<*const ForeignCode> =
        const { std::cell::Cell::new(std::ptr::null()) };
}

/// Restores the foreign code the thread was in before [`enter_foreign_code`].
pub struct ForeignGuard {
    previous: *const ForeignCode,
}

impl Drop for ForeignGuard {
    fn drop(&mut self) {
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
        let _ = FOREIGN.try_with(|cell| cell.set(self.previous));
    }
}

/// Mark this thread as running `code` (or nothing, for `None`) until the guard drops: a crash
/// meanwhile names it in the record and writes its crash file. Two thread-local writes, cheap
/// enough to wrap every parse.
pub fn enter_foreign_code(code: Option<&'static ForeignCode>) -> ForeignGuard {
    let next = code.map_or(std::ptr::null(), |code| code as *const ForeignCode);
    let previous = FOREIGN
        .try_with(|cell| cell.replace(next))
        .unwrap_or(std::ptr::null());
    // the mark must be in place before the foreign call it brackets, not moved past it
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    ForeignGuard { previous }
}

/// The foreign code this thread is in now, if any.
pub fn current_foreign_code() -> Option<&'static ForeignCode> {
    let pointer = FOREIGN.try_with(|cell| cell.get()).ok()?;
    // SAFETY: only ever a `&'static ForeignCode` (see `enter_foreign_code`), or null.
    unsafe { pointer.as_ref() }
}

/// Prepare the record and install the handlers; a no-op off Unix or with no state directory.
/// Called early in `main`, after the panic hook, and again when a test run has pointed
/// `XDG_STATE_HOME` at its scratch home (`arm_test_run`), which moves the record there: a
/// test that crashes a runtime never writes the user's state directory.
///
/// On Windows there is no record (ADR-635 is Unix-only), but a crash in foreign code still
/// writes its crash file — a grammar that crashed the process is quarantined for the next start
/// there too ([`windows`], ADR-672).
pub fn install() {
    #[cfg(unix)]
    unix::install();
    #[cfg(windows)]
    windows::install();
}

/// Windows: an unhandled-exception filter (and the C runtime's `SIGABRT` handler) that writes
/// the crash file of the foreign code the faulting thread was in — the quarantine of the grammar
/// that crashed the process (ADR-672). No crash record: ADR-635's is Unix-only.
///
/// The filter runs on the faulting thread, so the thread-local mark ([`current_foreign_code`])
/// names whose code it was, exactly as the Unix handler reads it. It runs only for an exception
/// nothing else handled, never a first-chance one, so a library that raises and catches its
/// own exceptions is not mistaken for a crash; it allocates nothing, makes the directories and
/// writes the file with `CreateDirectoryW` / `CreateFileW` / `WriteFile` over paths made at
/// load, then hands the exception on to the filter before it, so the process dies as it would
/// have (Windows Error Reporting included). A `__fastfail` (Rust's own abort) bypasses every
/// filter, and is never a grammar's.
#[cfg(windows)]
mod windows {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use windows_sys::Win32::Foundation::{
        CloseHandle, FILETIME, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_ALWAYS, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL, WriteFile,
    };
    use windows_sys::Win32::System::Diagnostics::Debug::{
        EXCEPTION_POINTERS, SetUnhandledExceptionFilter,
    };
    use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    /// Set once the filter is in place: `install` runs again when a test run moves the state
    /// directory, and the crash file's path is read from the mark, not from here.
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    /// The filter before ours, as an address (0 for none), handed every exception after.
    static PREVIOUS: AtomicUsize = AtomicUsize::new(0);
    /// Set by the first crash written, so a second exception on the way down writes nothing.
    static WRITTEN: AtomicBool = AtomicBool::new(false);

    type Filter = unsafe extern "system" fn(*const EXCEPTION_POINTERS) -> i32;
    /// What a filter answers to let the search go on: the previous filter, else the default.
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

    pub(super) fn install() {
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }
        // SAFETY: installing a filter function and a C runtime signal handler, both of which
        // live for the life of the process.
        unsafe {
            let previous = SetUnhandledExceptionFilter(Some(filter));
            PREVIOUS.store(
                previous.map_or(0, |filter| filter as usize),
                Ordering::SeqCst,
            );
            libc::signal(
                libc::SIGABRT,
                abort_handler as extern "C" fn(libc::c_int) as libc::sighandler_t,
            );
        }
    }

    /// The name of an exception code, as the vetting parent names a child's exit status.
    fn exception_name(code: u32) -> &'static [u8] {
        match code {
            0xC000_0005 => b"EXCEPTION_ACCESS_VIOLATION",
            0xC000_0006 => b"EXCEPTION_IN_PAGE_ERROR",
            0xC000_001D => b"EXCEPTION_ILLEGAL_INSTRUCTION",
            0xC000_0094 => b"EXCEPTION_INT_DIVIDE_BY_ZERO",
            0xC000_008C => b"EXCEPTION_ARRAY_BOUNDS_EXCEEDED",
            0xC000_0096 => b"EXCEPTION_PRIV_INSTRUCTION",
            0xC000_00FD => b"EXCEPTION_STACK_OVERFLOW",
            0xC000_0409 => b"STATUS_STACK_BUFFER_OVERRUN",
            0xC000_0374 => b"STATUS_HEAP_CORRUPTION",
            0x8000_0003 => b"EXCEPTION_BREAKPOINT",
            _ => b"EXCEPTION",
        }
    }

    unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the system hands the filter valid exception pointers; checked for null anyway.
        let code = unsafe {
            if info.is_null() || (*info).ExceptionRecord.is_null() {
                0
            } else {
                (*(*info).ExceptionRecord).ExceptionCode as u32
            }
        };
        // SAFETY: reads the thread's mark and writes a file through paths made at load.
        unsafe { write_crash_file(exception_name(code)) };
        let previous = PREVIOUS.load(Ordering::SeqCst);
        if previous == 0 {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        // SAFETY: `previous` is the filter `SetUnhandledExceptionFilter` returned, as an address.
        let previous: Filter = unsafe { std::mem::transmute::<usize, Filter>(previous) };
        unsafe { previous(info) }
    }

    /// A C `abort()` in foreign code raises `SIGABRT` in this C runtime before it ends the
    /// process (a grammar built against another C runtime raises it in that one, unseen).
    /// Returning lets the runtime go on and end the process as it would have.
    extern "C" fn abort_handler(_signal: libc::c_int) {
        // SAFETY: as the filter.
        unsafe { write_crash_file(b"abort") };
    }

    unsafe fn write_all(handle: windows_sys::Win32::Foundation::HANDLE, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let mut wrote: u32 = 0;
            let asked = bytes.len().min(u32::MAX as usize) as u32;
            // SAFETY: a live handle, a buffer of `asked` bytes, no overlapped I/O.
            let ok = unsafe {
                WriteFile(
                    handle,
                    bytes.as_ptr(),
                    asked,
                    &mut wrote,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || wrote == 0 {
                return;
            }
            bytes = &bytes[wrote as usize..];
        }
    }

    /// Write the crash file of the foreign code this thread is in, if it has one: its head,
    /// ` :signal "<name>" :time <ms> :pid <pid>`, its tail. Allocates nothing.
    unsafe fn write_crash_file(name: &[u8]) {
        let Some(code) = super::current_foreign_code() else {
            return;
        };
        let Some(crash_file) = &code.crash_file else {
            return;
        };
        if WRITTEN.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut now = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        // SAFETY: plain Win32 calls over NUL-terminated wide paths made at load, and a
        // stack-allocated FILETIME.
        unsafe {
            GetSystemTimeAsFileTime(&mut now);
            // 100 ns ticks since 1601 to milliseconds since 1970
            let ticks = ((now.dwHighDateTime as u64) << 32) | now.dwLowDateTime as u64;
            let time = (ticks.saturating_sub(116_444_736_000_000_000) / 10_000) as i128;
            let pid = GetCurrentProcessId() as i128;
            for dir in &crash_file.mkdirs {
                CreateDirectoryW(dir.as_ptr(), std::ptr::null());
            }
            let handle = CreateFileW(
                crash_file.file.as_ptr(),
                GENERIC_WRITE,
                0,
                std::ptr::null(),
                CREATE_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            );
            if handle == INVALID_HANDLE_VALUE {
                return;
            }
            let mut digits = [0u8; 40];
            write_all(handle, &crash_file.head);
            write_all(handle, b" :signal \"");
            write_all(handle, name);
            write_all(handle, b"\" :time ");
            write_all(handle, super::format_int(time, &mut digits));
            write_all(handle, b" :pid ");
            write_all(handle, super::format_int(pid, &mut digits));
            write_all(handle, &crash_file.tail);
            CloseHandle(handle);
        }
    }
}

/// Where the record would go, from the environment: an absolute `XDG_STATE_HOME`, else
/// `HOME/.local/state`, then `brood/crashes` (an empty or relative `XDG_STATE_HOME` is
/// ignored, as the XDG spec says).
#[cfg(unix)]
fn record_dir_from_env() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let set = |name| std::env::var_os(name).filter(|value| !value.is_empty());
    set("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            set("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".local").join("state"))
        })
        .map(|base| base.join("brood").join("crashes"))
}

#[cfg(any(unix, windows))]
/// `value` in decimal, written into the end of `digits`; the slice that holds it.
pub(crate) fn format_int(value: i128, digits: &mut [u8; 40]) -> &[u8] {
    let mut at = digits.len();
    let negative = value < 0;
    let mut rest = value.unsigned_abs();
    loop {
        at -= 1;
        digits[at] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    if negative {
        at -= 1;
        digits[at] = b'-';
    }
    &digits[at..]
}

/// `text` safe inside a Brood string literal and on one line: a quote or backslash becomes
/// `'` or `/`, a control character a space, and it is cut to `max` bytes on a char boundary.
pub(crate) fn literal_safe(text: &str, max: usize) -> String {
    let mut out = String::new();
    for character in text.chars() {
        let character = match character {
            '"' => '\'',
            '\\' => '/',
            c if c.is_control() => ' ',
            c => c,
        };
        if out.len() + character.len_utf8() > max {
            break;
        }
        out.push(character);
    }
    out
}

#[cfg(unix)]
mod unix {
    use super::format_int;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicUsize, Ordering};

    /// The signals that end the runtime with a record.
    const FATAL: [libc::c_int; 5] = [
        libc::SIGSEGV,
        libc::SIGBUS,
        libc::SIGILL,
        libc::SIGFPE,
        libc::SIGABRT,
    ];

    /// Everything the handler needs, made at startup.
    pub(super) struct Prepared {
        /// The record directory as a string, for `crash_record_dir`.
        pub(super) dir: String,
        /// Each directory to `mkdir` before opening the record, outermost first — the
        /// state directory itself may not exist yet.
        mkdirs: Vec<CString>,
        /// The record's path.
        file: CString,
        /// The record's fixed fields, ` :pid N :version "…" …`, ready to copy.
        fixed: Vec<u8>,
        /// The handler each fatal signal had before ours, by the index in [`FATAL`].
        prior: [libc::sigaction; FATAL.len()],
    }

    // SAFETY: `libc::sigaction` is plain data (a handler address, a mask, flags); it is
    // written before the `Prepared` is published and only read after.
    unsafe impl Sync for Prepared {}
    unsafe impl Send for Prepared {}

    /// The prepared record, published once built. Replaced (the old one leaked, a few hundred
    /// bytes once) when a test run moves the state directory: a pointer, so the handler
    /// reads whichever is current with one atomic load.
    static PREPARED: AtomicPtr<Prepared> = AtomicPtr::new(std::ptr::null_mut());

    pub(super) fn prepared() -> Option<&'static Prepared> {
        // SAFETY: only ever a leaked `Box<Prepared>`, never freed.
        unsafe { PREPARED.load(Ordering::SeqCst).as_ref() }
    }

    /// Set by the first fatal signal: a second one (Rust's overflow handler aborting after
    /// the SIGSEGV that found it) gives its signal back without writing over the first.
    static WRITTEN: AtomicBool = AtomicBool::new(false);

    /// The last panic's message, for the record a following abort writes. Atomics, so the
    /// handler reads it without a lock; a torn read of two racing panics is still text.
    const PANIC_NOTE_MAX: usize = 240;
    static PANIC_NOTE: [AtomicU8; PANIC_NOTE_MAX] = [const { AtomicU8::new(0) }; PANIC_NOTE_MAX];
    static PANIC_NOTE_LEN: AtomicUsize = AtomicUsize::new(0);

    pub(super) fn note_panic(message: &str) {
        let safe = super::literal_safe(message, PANIC_NOTE_MAX);
        PANIC_NOTE_LEN.store(0, Ordering::SeqCst);
        for (slot, byte) in PANIC_NOTE.iter().zip(safe.bytes()) {
            slot.store(byte, Ordering::Relaxed);
        }
        PANIC_NOTE_LEN.store(safe.len(), Ordering::SeqCst);
    }

    pub(super) fn install() {
        let Some(dir) = super::record_dir_from_env() else {
            return;
        };
        let earlier = prepared();
        if earlier.is_some_and(|earlier| earlier.dir == dir.to_string_lossy()) {
            return;
        }
        let cstring = |path: &std::path::Path| CString::new(path.as_os_str().as_bytes()).ok();
        let mut mkdirs: Vec<CString> = dir.ancestors().filter_map(cstring).collect();
        mkdirs.reverse();
        let pid = std::process::id();
        let Some(file) = cstring(&dir.join(format!("crash-{pid}.blsp"))) else {
            return;
        };
        let argv: Vec<String> = std::env::args_os()
            .take(8)
            .map(|argument| super::literal_safe(&argument.to_string_lossy(), 200))
            .collect();
        let argv = argv
            .iter()
            .map(|argument| format!("\"{argument}\""))
            .collect::<Vec<_>>()
            .join(" ");
        let fixed = format!(
            " :pid {pid} :version \"{}\" :sha \"{}\" :argv [{argv}]",
            env!("CARGO_PKG_VERSION"),
            env!("BROOD_GIT_SHA"),
        )
        .into_bytes();
        // The handlers before ours: read now, or kept from the record this one replaces (the
        // current ones are ours by then).
        // SAFETY: zeroed `sigaction` structs are valid to pass as the out-parameter; a null
        // new action only reads the current one.
        let prior = match earlier {
            Some(earlier) => earlier.prior,
            None => {
                let mut prior: [libc::sigaction; FATAL.len()] = unsafe { std::mem::zeroed() };
                for (slot, signal) in prior.iter_mut().zip(FATAL) {
                    unsafe { libc::sigaction(signal, std::ptr::null(), slot) };
                }
                prior
            }
        };
        let prepared = Prepared {
            dir: dir.to_string_lossy().into_owned(),
            mkdirs,
            file,
            fixed,
            prior,
        };
        PREPARED.store(Box::into_raw(Box::new(prepared)), Ordering::SeqCst);
        if earlier.is_some() {
            return;
        }
        for signal in FATAL {
            // SAFETY: a zeroed action naming our handler, on the alternate stack (a stack
            // overflow has no stack left to run it on; std gives every thread one), with
            // the signal's info.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = handler as *const () as libc::sighandler_t;
                action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(signal, &action, std::ptr::null_mut());
            }
        }
    }

    /// A fixed buffer on the handler's stack, appended to without allocating.
    struct Record {
        bytes: [u8; 2048],
        len: usize,
    }

    impl Record {
        fn push(&mut self, more: &[u8]) {
            let room = self.bytes.len() - self.len;
            let take = more.len().min(room);
            self.bytes[self.len..self.len + take].copy_from_slice(&more[..take]);
            self.len += take;
        }

        fn push_int(&mut self, value: i128) {
            let mut digits = [0u8; 40];
            self.push(format_int(value, &mut digits));
        }
    }

    fn signal_name(signal: libc::c_int) -> &'static [u8] {
        match signal {
            libc::SIGSEGV => b"SIGSEGV",
            libc::SIGBUS => b"SIGBUS",
            libc::SIGILL => b"SIGILL",
            libc::SIGFPE => b"SIGFPE",
            libc::SIGABRT => b"SIGABRT",
            _ => b"SIG?",
        }
    }

    /// The record, as one Brood map literal.
    ///
    /// SAFETY (async-signal-safety): reads atomics and the prepared data, calls
    /// `clock_gettime` and (Linux) `prctl`, and formats into its own stack buffer.
    unsafe fn compose(
        prepared: &Prepared,
        signal: libc::c_int,
        info: *const libc::siginfo_t,
    ) -> Record {
        let mut record = Record {
            bytes: [0; 2048],
            len: 0,
        };
        record.push(b"{:kind :fatal-signal :signal \"");
        record.push(signal_name(signal));
        record.push(b"\" :signo ");
        record.push_int(signal as i128);
        let mut now: libc::timespec = unsafe { std::mem::zeroed() };
        unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut now) };
        record.push(b" :time ");
        record.push_int(now.tv_sec as i128 * 1000 + now.tv_nsec as i128 / 1_000_000);
        record.push(&prepared.fixed);
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            let mut name = [0u8; 16];
            if unsafe { libc::prctl(libc::PR_GET_NAME, name.as_mut_ptr()) } == 0 {
                record.push(b" :thread \"");
                for &byte in name.iter().take_while(|&&byte| byte != 0) {
                    let byte = match byte {
                        b'"' => b'\'',
                        b'\\' => b'/',
                        0..=31 | 127.. => b'?',
                        other => other,
                    };
                    record.push(&[byte]);
                }
                record.push(b"\"");
            }
            // the faulting address, which only a fault (si_code > 0) has
            if !info.is_null() && unsafe { (*info).si_code } > 0 {
                record.push(b" :address ");
                record.push_int(unsafe { (*info).si_addr() } as usize as i128);
            }
        }
        if !info.is_null() {
            record.push(b" :code ");
            record.push_int(unsafe { (*info).si_code } as i128);
        }
        let len = PANIC_NOTE_LEN.load(Ordering::SeqCst);
        // the foreign code this thread died in (a grammar, ADR-660), when it fits with the
        // panic note and the closing brace — a fragment cut short would leave no map at all
        if let Some(code) = super::current_foreign_code() {
            let fragment = code.record_fragment();
            if record.len + fragment.len() + len + 16 <= record.bytes.len() {
                record.push(fragment);
            }
        }
        if len > 0 {
            record.push(b" :panic \"");
            for slot in PANIC_NOTE.iter().take(len) {
                record.push(&[slot.load(Ordering::Relaxed)]);
            }
            record.push(b"\"");
        }
        record.push(b"}\n");
        record
    }

    /// Write `bytes` to `fd` whole, retrying a short write. Async-signal-safe.
    unsafe fn write_all(fd: libc::c_int, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let wrote = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
            if wrote <= 0 {
                return;
            }
            bytes = &bytes[wrote as usize..];
        }
    }

    /// Write the crash file of the foreign code the runtime died in: its head, the signal, the
    /// time and the pid, its tail. Async-signal-safe: `mkdir`, `open`, `write`, `close`,
    /// `clock_gettime`, `getpid`, and integers formatted on the stack.
    unsafe fn write_crash_file(crash_file: &super::CrashFile, signal: libc::c_int) {
        // written piece by piece, no second record buffer: the record's is still on this
        // stack, the alternate signal stack, which has only a few kilobytes
        let mut now: libc::timespec = unsafe { std::mem::zeroed() };
        unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut now) };
        let time = now.tv_sec as i128 * 1000 + now.tv_nsec as i128 / 1_000_000;
        let pid = unsafe { libc::getpid() } as i128;
        let mut digits = [0u8; 40];
        unsafe {
            for dir in &crash_file.mkdirs {
                libc::mkdir(dir.as_ptr(), 0o700);
            }
            let fd = libc::open(
                crash_file.file.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC,
                0o600 as libc::c_uint,
            );
            if fd >= 0 {
                write_all(fd, &crash_file.head);
                write_all(fd, b" :signal \"");
                write_all(fd, signal_name(signal));
                write_all(fd, b"\" :time ");
                write_all(fd, format_int(time, &mut digits));
                write_all(fd, b" :pid ");
                write_all(fd, format_int(pid, &mut digits));
                write_all(fd, &crash_file.tail);
                libc::close(fd);
            }
        }
    }

    extern "C" fn handler(
        signal: libc::c_int,
        info: *mut libc::siginfo_t,
        _context: *mut libc::c_void,
    ) {
        let Some(prepared) = prepared() else {
            return;
        };
        // SAFETY: everything below is on the async-signal-safe list, or reads memory that
        // was written before the handler was installed.
        unsafe {
            if !WRITTEN.swap(true, Ordering::SeqCst) {
                let record = compose(prepared, signal, info);
                for dir in &prepared.mkdirs {
                    libc::mkdir(dir.as_ptr(), 0o700);
                }
                let fd = libc::open(
                    prepared.file.as_ptr(),
                    libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC,
                    0o600 as libc::c_uint,
                );
                if fd >= 0 {
                    write_all(fd, &record.bytes[..record.len]);
                    libc::close(fd);
                    write_all(2, b"brood: fatal ");
                    write_all(2, signal_name(signal));
                    write_all(2, b"; crash record written to ");
                    write_all(2, prepared.file.as_bytes());
                    write_all(2, b"\n");
                }
                if let Some(code) = super::current_foreign_code()
                    && let Some(crash_file) = &code.crash_file
                {
                    write_crash_file(crash_file, signal);
                }
            }
            // Give the signal back. A fault (si_code > 0) re-raises itself when the handler
            // returns, so the prior handler (Rust's stack-overflow check) sees it as it would
            // have; a signal sent by kill or abort does not come back, so it is raised again
            // under the default action, and the process dies by it.
            let index = FATAL.iter().position(|&fatal| fatal == signal).unwrap_or(0);
            let sent = info.is_null() || (*info).si_code <= 0;
            if sent {
                let mut default: libc::sigaction = std::mem::zeroed();
                default.sa_sigaction = libc::SIG_DFL;
                libc::sigaction(signal, &default, std::ptr::null_mut());
                libc::raise(signal);
            } else {
                libc::sigaction(signal, &prepared.prior[index], std::ptr::null_mut());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::literal_safe;

    #[test]
    fn foreign_code_is_marked_per_thread_and_a_long_fragment_is_dropped() {
        use super::{ForeignCode, current_foreign_code, enter_foreign_code};
        let code: &'static ForeignCode = Box::leak(Box::new(
            ForeignCode::new(" :grammar {:lang :x}".to_string()).with_crash_file(
                std::path::Path::new("/state/brood/grammar-verdicts/ab.blsp"),
                "{:verdict :quarantined".to_string(),
                "}\n".to_string(),
            ),
        ));
        assert_eq!(code.record_fragment(), b" :grammar {:lang :x}");
        let crash_file = code.crash_file.as_ref().expect("a crash file");
        assert_eq!(
            crash_file.mkdirs.len(),
            4,
            "/, /state, /state/brood, …/grammar-verdicts"
        );
        {
            let _marked = enter_foreign_code(Some(code));
            assert!(current_foreign_code().is_some_and(|current| std::ptr::eq(current, code)));
            // another thread is not in it
            std::thread::spawn(|| assert!(current_foreign_code().is_none()))
                .join()
                .unwrap();
        }
        assert!(current_foreign_code().is_none());
        let long = ForeignCode::new(format!(" :grammar {{:library \"{}\"}}", "x".repeat(900)));
        assert!(long.record_fragment().is_empty());
    }

    #[test]
    fn a_literal_keeps_no_quote_backslash_or_newline_and_is_cut_on_a_char_boundary() {
        assert_eq!(literal_safe("a \"b\"\\c\nd", 100), "a 'b'/c d");
        assert_eq!(literal_safe("日本語", 7), "日本");
    }
}
