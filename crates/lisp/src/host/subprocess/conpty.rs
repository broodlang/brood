//! Windows's pseudo-terminal: ConPTY (KI-369, ADR-597).
//!
//! A Unix pty is a device pair the child opens as its stdio. ConPTY is a pseudoconsole
//! the system hosts (`CreatePseudoConsole`) over two pipes of ours: it reads the child's
//! input from one and writes the child's screen, as VT sequences, into the other. The
//! child is attached to it at creation, through a `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`
//! entry in an attribute list handed to `CreateProcessW` — which `std::process::Command`
//! cannot pass on stable Rust, so the spawn is written out here.
//!
//! Two things differ from the Unix side, and the subprocess registry handles both:
//!
//! * the output pipe does not end when the child does — the pseudoconsole holds it, so
//!   the waiter closes the pseudoconsole once the child has exited
//!   ([`PseudoConsole::close`]), and the reader then sees end of file;
//! * there is no termios, so nothing says whether the child is reading a password:
//!   `os/pty-mode` answers nil (unknown) for a ConPTY child.
//!
//! The ConPTY functions are looked up in `kernel32.dll` at run time rather than linked:
//! they exist from Windows 10 1809, and an import of a missing function stops the whole
//! executable loading — on an older Windows `os/spawn-pty` raises instead.

use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Console::{COORD, HPCON};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use super::{EnvEntry, ProcessTree, program};

type CreatePseudoConsoleFunction =
    unsafe extern "system" fn(COORD, HANDLE, HANDLE, u32, *mut HPCON) -> i32;
type ResizePseudoConsoleFunction = unsafe extern "system" fn(HPCON, COORD) -> i32;
type ClosePseudoConsoleFunction = unsafe extern "system" fn(HPCON);

/// The three ConPTY entry points, found in `kernel32.dll`.
struct ConsoleFunctions {
    create: CreatePseudoConsoleFunction,
    resize: ResizePseudoConsoleFunction,
    close: ClosePseudoConsoleFunction,
}

/// The ConPTY functions, or `None` on a Windows older than 10 1809. Looked up once.
fn console_functions() -> Option<&'static ConsoleFunctions> {
    static FUNCTIONS: OnceLock<Option<ConsoleFunctions>> = OnceLock::new();
    FUNCTIONS
        .get_or_init(|| {
            let module_name: Vec<u16> = "kernel32.dll\0".encode_utf16().collect();
            // SAFETY: a NUL-terminated wide name; kernel32 is loaded in every process, and
            // GetModuleHandleW takes no reference on it, so nothing is released after.
            let module = unsafe { GetModuleHandleW(module_name.as_ptr()) };
            if module.is_null() {
                return None;
            }
            // SAFETY: NUL-terminated names; each found address is the documented function
            // of that name, whose signature the transmute target spells out.
            unsafe {
                let create = GetProcAddress(module, c"CreatePseudoConsole".as_ptr().cast())?;
                let resize = GetProcAddress(module, c"ResizePseudoConsole".as_ptr().cast())?;
                let close = GetProcAddress(module, c"ClosePseudoConsole".as_ptr().cast())?;
                Some(ConsoleFunctions {
                    create: std::mem::transmute::<
                        unsafe extern "system" fn() -> isize,
                        CreatePseudoConsoleFunction,
                    >(create),
                    resize: std::mem::transmute::<
                        unsafe extern "system" fn() -> isize,
                        ResizePseudoConsoleFunction,
                    >(resize),
                    close: std::mem::transmute::<
                        unsafe extern "system" fn() -> isize,
                        ClosePseudoConsoleFunction,
                    >(close),
                })
            }
        })
        .as_ref()
}

fn size_of_terminal(cols: u16, rows: u16) -> COORD {
    // `os/spawn-pty` clamps both to 1..=10000, inside an i16.
    COORD {
        X: cols.min(i16::MAX as u16) as i16,
        Y: rows.min(i16::MAX as u16) as i16,
    }
}

fn hresult_error(what: &str, result: i32) -> std::io::Error {
    std::io::Error::other(format!("{what} failed (HRESULT {result:#010x})"))
}

/// A pseudoconsole: resized while the child runs, closed once — by the waiter after the
/// child exits (which is what ends the output pipe), or on drop.
pub(super) struct PseudoConsole {
    handle: Mutex<Option<HPCON>>,
    functions: &'static ConsoleFunctions,
}

impl PseudoConsole {
    /// `ResizePseudoConsole`: the child sees its console buffer change size, as a Unix
    /// child sees `SIGWINCH`.
    pub(super) fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
        let handle = crate::core::sync::lock(&self.handle);
        let Some(console) = *handle else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "pty-resize: the terminal is already closed",
            ));
        };
        // SAFETY: an open pseudoconsole (closing takes it out under this lock).
        let result = unsafe { (self.functions.resize)(console, size_of_terminal(cols, rows)) };
        if result < 0 {
            return Err(hresult_error("ResizePseudoConsole", result));
        }
        Ok(())
    }

    /// `ClosePseudoConsole`, once. It ends the console host, which closes the output
    /// pipe after writing what was left — the reader must still be reading (it is, on
    /// its own thread), since the host may wait for that pipe to drain. Not under the
    /// lock: resize only needs to see it gone.
    pub(super) fn close(&self) {
        let taken = crate::core::sync::lock(&self.handle).take();
        if let Some(console) = taken {
            // SAFETY: taken out under the lock, so closed exactly once.
            unsafe { (self.functions.close)(console) };
        }
    }
}

impl Drop for PseudoConsole {
    fn drop(&mut self) {
        self.close();
    }
}

// SAFETY: an HPCON is a handle the console API takes from any thread; the mutex orders
// resize against close.
unsafe impl Send for PseudoConsole {}
unsafe impl Sync for PseudoConsole {}

/// A child attached to a pseudoconsole: its process handle (not a `std::process::Child`,
/// which only `Command` makes) and the pseudoconsole the waiter closes after it.
pub(super) struct ConsoleChild {
    process: HANDLE,
    console: Arc<PseudoConsole>,
}

// SAFETY: a process handle is a kernel handle, usable from any thread; it is closed once,
// on drop.
unsafe impl Send for ConsoleChild {}
unsafe impl Sync for ConsoleChild {}

impl ConsoleChild {
    pub(super) fn console(&self) -> &Arc<PseudoConsole> {
        &self.console
    }

    /// `TerminateProcess`, exit code 1 — what `std::process::Child::kill` does on Windows.
    pub(super) fn kill(&mut self) -> std::io::Result<()> {
        // SAFETY: the process handle is live until drop.
        if unsafe { TerminateProcess(self.process, 1) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// The exit code if the child has exited, without waiting.
    pub(super) fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
        // SAFETY: the process handle is live until drop.
        match unsafe { WaitForSingleObject(self.process, 0) } {
            WAIT_OBJECT_0 => self.exit_code().map(Some),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(std::io::Error::last_os_error()),
        }
    }

    /// Block until the child has exited.
    pub(super) fn wait(&mut self) -> std::io::Result<i32> {
        // SAFETY: the process handle is live until drop.
        if unsafe { WaitForSingleObject(self.process, INFINITE) } != WAIT_OBJECT_0 {
            return Err(std::io::Error::last_os_error());
        }
        self.exit_code()
    }

    fn exit_code(&self) -> std::io::Result<i32> {
        let mut code: u32 = 0;
        // SAFETY: a live process handle and a live out-parameter.
        if unsafe { GetExitCodeProcess(self.process, &mut code) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(code as i32)
    }
}

impl Drop for ConsoleChild {
    fn drop(&mut self) {
        // SAFETY: owned, closed once.
        unsafe { CloseHandle(self.process) };
    }
}

/// What [`spawn`] hands the registry: the child, the Job Object it runs in, and our two
/// pipe ends — `input` (what is written reaches the child as typed) and `output` (the
/// child's screen).
pub(super) struct Spawned {
    pub(super) child: ConsoleChild,
    pub(super) tree: Option<ProcessTree>,
    pub(super) input: File,
    pub(super) output: File,
}

/// An anonymous pipe: `(read end, write end)`, neither inheritable.
fn pipe() -> std::io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read_end: HANDLE = std::ptr::null_mut();
    let mut write_end: HANDLE = std::ptr::null_mut();
    // SAFETY: live out-parameters; null security attributes make the ends uninheritable.
    if unsafe { CreatePipe(&mut read_end, &mut write_end, std::ptr::null(), 0) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both freshly created, owned by nothing else.
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(read_end),
            OwnedHandle::from_raw_handle(write_end),
        )
    })
}

/// A process-thread attribute list holding one entry, deleted on drop. The buffer is
/// `usize`s so it is aligned as the list needs.
struct AttributeList {
    buffer: Vec<usize>,
}

impl AttributeList {
    fn with_pseudoconsole(console: HPCON) -> std::io::Result<AttributeList> {
        let mut size: usize = 0;
        // SAFETY: the documented size query — a null list fails, setting `size`.
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size) };
        if size == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut list = AttributeList {
            buffer: vec![0usize; size.div_ceil(std::mem::size_of::<usize>())],
        };
        // SAFETY: a buffer of at least `size` bytes, as the query asked for.
        if unsafe { InitializeProcThreadAttributeList(list.pointer(), 1, 0, &mut size) } == 0 {
            // Not initialised, so not to be deleted.
            std::mem::forget(list);
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: an initialised list with room for one entry. The pseudoconsole entry's
        // value is the HPCON itself, not a pointer to it (the documented use).
        let updated = unsafe {
            UpdateProcThreadAttribute(
                list.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                console as *const c_void,
                std::mem::size_of::<HPCON>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if updated == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(list)
    }

    fn pointer(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buffer.as_mut_ptr().cast()
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        // SAFETY: initialised by `with_pseudoconsole` (a failed one is forgotten).
        unsafe { DeleteProcThreadAttributeList(self.pointer()) };
    }
}

/// Append `argument` to a command line as the MSVC runtime reads one — the quoting
/// `std::process::Command` applies (`quote` forces the quotes, as it does for the program):
/// quoted when it holds a space or a tab or is empty, a `"` escaped, and the backslashes
/// before a `"` (or before the closing quote) doubled.
fn append_argument(line: &mut Vec<u16>, argument: &OsStr, quote: bool) {
    let quoted = quote || argument.is_empty() || {
        let units: Vec<u16> = argument.encode_wide().collect();
        units.contains(&(b' ' as u16)) || units.contains(&(b'\t' as u16))
    };
    if quoted {
        line.push(b'"' as u16);
    }
    let mut backslashes: usize = 0;
    for unit in argument.encode_wide() {
        if unit == b'\\' as u16 {
            backslashes += 1;
        } else {
            if unit == b'"' as u16 {
                // the run of backslashes doubled, then one more for the quote itself
                line.extend(std::iter::repeat_n(b'\\' as u16, backslashes + 1));
            }
            backslashes = 0;
        }
        line.push(unit);
    }
    if quoted {
        line.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
        line.push(b'"' as u16);
    }
}

/// The child's command line, NUL-terminated: the program quoted, then each argument —
/// quoted as [`append_argument`] does, or with `raw` written exactly as given (what
/// `cmd.exe /d /s /c "LINE"` needs, ADR-592).
pub(super) fn command_line(
    program: &str,
    arguments: &[String],
    raw: bool,
) -> std::io::Result<Vec<u16>> {
    if program.contains('"') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a program name cannot contain a double quote",
        ));
    }
    let mut line: Vec<u16> = Vec::new();
    append_argument(&mut line, OsStr::new(program), true);
    for argument in arguments {
        line.push(b' ' as u16);
        if raw {
            line.extend(OsStr::new(argument).encode_wide());
        } else {
            append_argument(&mut line, OsStr::new(argument), false);
        }
    }
    line.push(0);
    Ok(line)
}

/// The child's environment block: ours, with `env` applied (Windows names are matched
/// without case), `TERM` added as the Unix side adds it, sorted by name as
/// `CreateProcessW` expects, each `NAME=VALUE` NUL-terminated and the block ended by one
/// more NUL.
fn environment_block(env: &[EnvEntry]) -> Vec<u16> {
    let same_name = |left: &OsStr, right: &str| left.to_string_lossy().eq_ignore_ascii_case(right);
    let mut variables: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    for (name, value) in env {
        variables.retain(|(existing, _)| !same_name(existing, name));
        if let Some(value) = value {
            variables.push((OsString::from(name), OsString::from(value)));
        }
    }
    let term_named = env
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("TERM"))
        || variables.iter().any(|(name, _)| same_name(name, "TERM"));
    if !term_named {
        variables.push((OsString::from("TERM"), OsString::from("xterm-256color")));
    }
    variables.sort_by_key(|(name, _)| name.to_string_lossy().to_uppercase());
    let mut block: Vec<u16> = Vec::new();
    for (name, value) in &variables {
        block.extend(name.encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    if variables.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// The application `CreateProcessW` starts and the command line it is given, both
/// NUL-terminated: `program` resolved against the child's `cwd` and `env` (ADR-607) — an
/// absolute path, so no search of Windows's own is made — and, for a batch file, the
/// system's `cmd.exe` with the escaped line [`program::batch_command_line`] writes.
fn application_and_line(
    program: &str,
    arguments: &[String],
    raw: bool,
    cwd: Option<&str>,
    env: &[EnvEntry],
) -> std::io::Result<(Vec<u16>, Vec<u16>)> {
    let path = program::resolve(program, cwd, env)?;
    if program::is_batch_file(&path) {
        let interpreter = program::system_command_interpreter()?;
        let line = program::batch_command_line(path.as_os_str(), arguments, raw)?;
        return Ok((wide(interpreter.as_os_str()), wide(OsStr::new(&line))));
    }
    let resolved = path.to_string_lossy();
    Ok((
        wide(path.as_os_str()),
        command_line(&resolved, arguments, raw)?,
    ))
}

/// Start `program` attached to a fresh `cols`×`rows` pseudoconsole, in a Job Object (so
/// closing it ends what it started, ADR-592), and hand back the child and our pipe ends.
///
/// The child is created suspended and resumed only once it is in its job, so nothing it
/// starts can slip out of the tree first. Its std handles are set invalid with
/// `STARTF_USESTDHANDLES`: without that, a child of a runtime whose own stdio is
/// redirected (a CI runner, a daemon) inherits those handles and writes there instead of
/// to its console.
pub(super) fn spawn(
    program: &str,
    arguments: &[String],
    raw: bool,
    cwd: Option<&str>,
    env: &[EnvEntry],
    cols: u16,
    rows: u16,
) -> std::io::Result<Spawned> {
    let Some(functions) = console_functions() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "pty-spawn: this Windows has no ConPTY, the pseudo-terminal os/spawn-pty needs \
             (Windows 10 version 1809 or later)",
        ));
    };
    // The program by absolute path, found as every spawn finds one (ADR-607): handed to
    // CreateProcessW as the application, so its own search — the current directory first,
    // and never an `:env` PATH — is not made. A batch file runs under the system's
    // cmd.exe, its arguments escaped for it.
    let (application, mut line) = application_and_line(program, arguments, raw, cwd, env)?;
    let environment = environment_block(env);
    let directory: Option<Vec<u16>> = cwd.map(|directory| {
        OsStr::new(directory)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    });

    // The child's input is read from `input_read`; its screen is written to `output_write`.
    let (input_read, input_write) = pipe()?;
    let (output_read, output_write) = pipe()?;
    let mut console_handle: HPCON = 0;
    // SAFETY: two live pipe ends and a live out-parameter.
    let result = unsafe {
        (functions.create)(
            size_of_terminal(cols, rows),
            std::os::windows::io::AsRawHandle::as_raw_handle(&input_read),
            std::os::windows::io::AsRawHandle::as_raw_handle(&output_write),
            0,
            &mut console_handle,
        )
    };
    if result < 0 {
        return Err(hresult_error("CreatePseudoConsole", result));
    }
    let console = Arc::new(PseudoConsole {
        handle: Mutex::new(Some(console_handle)),
        functions,
    });
    // The console host has its own copies of these two; ours would keep the pipes open
    // after it closes.
    drop(input_read);
    drop(output_write);

    let mut attributes = AttributeList::with_pseudoconsole(console_handle)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
    startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
    startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
    startup.lpAttributeList = attributes.pointer();
    let mut information = PROCESS_INFORMATION::default();
    // SAFETY: every pointer is live for the call — the mutable command line (CreateProcessW
    // may write to it), the environment block, the directory, the startup information and
    // its attribute list; no handles are inherited.
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
            environment.as_ptr().cast(),
            directory
                .as_ref()
                .map_or(std::ptr::null(), |directory| directory.as_ptr()),
            &startup.StartupInfo,
            &mut information,
        )
    };
    drop(attributes);
    if created == 0 {
        let error = std::io::Error::last_os_error();
        return Err(std::io::Error::new(
            error.kind(),
            format!("cannot start {program}: {error}"),
        ));
    }
    let mut child = ConsoleChild {
        process: information.hProcess,
        console,
    };
    let tree = ProcessTree::of_handle(information.hProcess);
    // SAFETY: the primary thread's handle, live until closed just below.
    let resumed = unsafe { ResumeThread(information.hThread) };
    // SAFETY: owned, closed once; the child no longer needs it from us.
    unsafe { CloseHandle(information.hThread) };
    if resumed == u32::MAX {
        let error = std::io::Error::last_os_error();
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(Spawned {
        child,
        tree,
        input: File::from(input_write),
        output: File::from(output_read),
    })
}

#[cfg(test)]
mod command_line_tests {
    use super::*;

    fn line_of(program: &str, arguments: &[&str], raw: bool) -> String {
        let arguments: Vec<String> = arguments
            .iter()
            .map(|argument| argument.to_string())
            .collect();
        let units = command_line(program, &arguments, raw).expect("a command line");
        String::from_utf16(&units[..units.len() - 1]).expect("utf-16")
    }

    #[test]
    fn arguments_are_quoted_as_the_msvc_runtime_reads_them() {
        assert_eq!(
            line_of("cmd.exe", &["/c", "echo hi"], false),
            r#""cmd.exe" /c "echo hi""#
        );
        assert_eq!(line_of("p", &[""], false), r#""p" """#);
        assert_eq!(line_of("p", &[r#"a"b"#], false), r#""p" a\"b"#);
        assert_eq!(line_of("p", &[r"a\b c\"], false), r#""p" "a\b c\\""#);
        assert_eq!(line_of("p", &[r#"a\"b"#], false), r#""p" a\\\"b"#);
    }

    #[test]
    fn a_raw_argument_is_written_as_given() {
        assert_eq!(
            line_of("cmd.exe", &["/d", "/s", "/c", r#""echo "x" & ver""#], true),
            r#""cmd.exe" /d /s /c "echo "x" & ver""#
        );
    }
}
