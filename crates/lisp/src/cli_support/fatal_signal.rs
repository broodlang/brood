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

/// Prepare the record and install the handlers; a no-op off Unix or with no state directory.
/// Called early in `main`, after the panic hook, and again when a test run has pointed
/// `XDG_STATE_HOME` at its scratch home (`arm_test_run`), which moves the record there: a
/// test that crashes a runtime never writes the user's state directory.
pub fn install() {
    #[cfg(unix)]
    unix::install();
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

/// `text` safe inside a Brood string literal and on one line: a quote or backslash becomes
/// `'` or `/`, a control character a space, and it is cut to `max` bytes on a char boundary.
fn literal_safe(text: &str, max: usize) -> String {
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
            self.push(&digits[at..]);
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
    fn a_literal_keeps_no_quote_backslash_or_newline_and_is_cut_on_a_char_boundary() {
        assert_eq!(literal_safe("a \"b\"\\c\nd", 100), "a 'b'/c d");
        assert_eq!(literal_safe("日本語", 7), "日本");
    }
}
