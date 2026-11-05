//! Finding the program a child runs — the ONE resolver behind `os/which` and every spawn
//! (`os/spawn`, `os/spawn-pty`, `os/cmd`, `os/run-process`, `os/detach`), ADR-607.
//!
//! The spawns used to leave the search to the platform while `os/which` did its own, and on
//! Windows the two disagreed. `std::process::Command` with a bare name finds only
//! `name.exe`, so `which` said yes to `mix` (`mix.bat`) or an npm shim (`tsc.cmd`) and the
//! spawn then failed; and the ConPTY spawn handed `CreateProcessW` the bare name, whose
//! search tries the CURRENT DIRECTORY first — a `bash.exe` planted in a cloned repository
//! ran in place of the real one — and never looks at an `:env` `PATH`. Now a spawn asks this
//! resolver, with the child's `:cwd` and `:env`, and runs the absolute path it answers; and
//! `os/which` is this resolver too, so it answers where a spawn finds a program because it
//! is the same code.
//!
//! The rule is `execvp`'s, the same everywhere: a program naming a directory (a `/`, or on
//! Windows also a `\`) is not searched for — it is that file, taken against the child's
//! working directory — and otherwise each `PATH` entry is tried in order, an empty entry
//! meaning the working directory. The `PATH` is the child's: the `:env` one when the spawn
//! sets it, ours otherwise; with none at all nothing is found. A directory, or (Unix) a file
//! nobody may execute, is not a match. Windows adds its program extensions: a name is a
//! program only with one of `PATHEXT`'s (`.COM .EXE .BAT .CMD` by default) — `git` finds
//! `git.exe`, `tsc` finds `tsc.cmd`, and an extensionless file (an npm package's `sh` shim
//! beside its `.cmd`) is never the answer. Windows's own search adds the current directory,
//! the system directories and the executable's directory; this one does not, so a program
//! the user did not put on `PATH` is not run.
//!
//! A batch file (`.bat` / `.cmd`) is not a program Windows can start; `cmd.exe` runs it,
//! and `cmd.exe` re-parses the command line — `&`, `|`, `%VAR%` in an argument are commands
//! and expansions to it (the BatBadBut class, CVE-2024-24576). `std::process::Command`
//! handles a batch file it is given the path of (since Rust 1.77.2): it runs it through the
//! system's `cmd.exe` with each argument escaped for it, and refuses an argument it cannot
//! escape. The ConPTY spawn builds its own command line, so [`batch_command_line`] is that
//! escaping, written out.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use super::EnvEntry;

/// The extensions a Windows program may have when `PATHEXT` is unset.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// What a search needs to know about the platform and the child — passed in, not read, so
/// the Windows rule is tested on every platform.
pub(crate) struct Search<'a> {
    /// The Windows rule: `;` between `PATH` entries, `\` a separator too, `PATHEXT`.
    pub(crate) windows: bool,
    /// The child's `PATH`, or `None` when it has none.
    pub(crate) path: Option<OsString>,
    /// The program extensions (Windows), `;`-separated.
    pub(crate) extensions: String,
    /// The child's working directory, which relative candidates are taken against.
    pub(crate) base: PathBuf,
    /// Whether a path names a file this child could run.
    pub(crate) runnable: &'a dyn Fn(&Path) -> bool,
}

impl Search<'_> {
    fn names_a_path(&self, program: &str) -> bool {
        program.contains('/') || (self.windows && program.contains('\\'))
    }

    fn extensions(&self) -> Vec<String> {
        self.extensions
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(|extension| extension.to_ascii_lowercase())
            .collect()
    }

    /// What a file named `base` may be called to run: itself on Unix; on Windows itself
    /// only when it already ends in a program extension, else itself with each one.
    fn names_for(&self, base: &Path) -> Vec<PathBuf> {
        if !self.windows {
            return vec![base.to_path_buf()];
        }
        let lowered = base.to_string_lossy().to_ascii_lowercase();
        let extensions = self.extensions();
        if extensions
            .iter()
            .any(|extension| lowered.ends_with(extension.as_str()))
        {
            return vec![base.to_path_buf()];
        }
        extensions
            .iter()
            .map(|extension| {
                let mut name = base.as_os_str().to_os_string();
                name.push(extension);
                PathBuf::from(name)
            })
            .collect()
    }

    /// The first name `base` may go by that is runnable — tested against the child's
    /// working directory, answered as written.
    fn first_runnable(&self, base: &Path) -> Option<PathBuf> {
        self.names_for(base)
            .into_iter()
            .find(|candidate| (self.runnable)(&self.base.join(candidate)))
    }

    fn entries(&self) -> Vec<PathBuf> {
        let Some(path) = &self.path else {
            return Vec::new();
        };
        if path.is_empty() {
            return Vec::new();
        }
        let separator = if self.windows { ';' } else { ':' };
        path.to_string_lossy()
            .split(separator)
            .map(|entry| PathBuf::from(if entry.is_empty() { "." } else { entry }))
            .collect()
    }

    /// Where `program` is, as written (a relative answer is relative to [`Search::base`]),
    /// or `None`.
    pub(crate) fn find(&self, program: &str) -> Option<PathBuf> {
        if program.is_empty() {
            return None;
        }
        if self.names_a_path(program) {
            return self.first_runnable(Path::new(program));
        }
        self.entries()
            .iter()
            .find_map(|directory| self.first_runnable(&directory.join(program)))
    }
}

/// Whether `path` names a file someone may execute: it exists, it is not a directory, and
/// on Unix an execute bit is set. Windows has no execute bits, so existence is the test.
fn runnable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if metadata.is_dir() {
        return false;
    }
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = true;
    executable
}

/// The value `name` has in the child's environment: the `:env` entry when it sets or unsets
/// it (the last such entry, as they are applied in order), ours otherwise. Windows matches
/// the name without case.
fn child_variable(name: &str, env: &[EnvEntry]) -> Option<OsString> {
    let same = |other: &str| {
        if cfg!(windows) {
            other.eq_ignore_ascii_case(name)
        } else {
            other == name
        }
    };
    match env.iter().rev().find(|(variable, _)| same(variable)) {
        Some((_, value)) => value.as_ref().map(OsString::from),
        None => std::env::var_os(name),
    }
}

/// The child's working directory, absolute: `cwd` taken against ours, or ours.
fn child_directory(cwd: Option<&str>) -> PathBuf {
    let ours = std::env::current_dir().unwrap_or_default();
    match cwd {
        Some(directory) => ours.join(directory),
        None => ours,
    }
}

fn search_for<'a>(
    cwd: Option<&str>,
    env: &[EnvEntry],
    runnable: &'a dyn Fn(&Path) -> bool,
) -> Search<'a> {
    Search {
        windows: cfg!(windows),
        path: child_variable("PATH", env),
        extensions: child_variable("PATHEXT", env)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| DEFAULT_PATHEXT.to_string()),
        base: child_directory(cwd),
        runnable,
    }
}

/// Where a child started with `cwd` and `env` would find `program` — what `os/which`
/// answers: as written when no `cwd` is given (`./build.sh` stays `./build.sh`), else taken
/// against `cwd`, which the caller's relative path would not be. `None` when nothing by
/// that name can be run.
pub fn which(program: &str, cwd: Option<&str>, env: &[EnvEntry]) -> Option<PathBuf> {
    let search = search_for(cwd, env, &runnable);
    let found = search.find(program)?;
    Some(if cwd.is_some() {
        search.base.join(found)
    } else {
        found
    })
}

/// The absolute path of the file a child started with `cwd` and `env` runs for `program`,
/// or a `NotFound` error saying where it looked.
pub fn resolve(program: &str, cwd: Option<&str>, env: &[EnvEntry]) -> std::io::Result<PathBuf> {
    let search = search_for(cwd, env, &runnable);
    match search.find(program) {
        Some(found) => Ok(search.base.join(found)),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            if search.names_a_path(program) {
                "no such program (not found, or not executable)".to_string()
            } else {
                "not found on PATH".to_string()
            },
        )),
    }
}

/// A `Command` for `program` resolved as [`resolve`] does. On Unix the child still sees
/// `program` as its `argv[0]`, as it would have from `execvp`.
pub fn command(
    program: &str,
    cwd: Option<&str>,
    env: &[EnvEntry],
) -> std::io::Result<std::process::Command> {
    let path = resolve(program, cwd, env)?;
    #[allow(unused_mut)]
    let mut command = std::process::Command::new(&path);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.arg0(program);
    }
    Ok(command)
}

/// Whether `path` is a batch file, which `cmd.exe` runs rather than Windows.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn is_batch_file(path: &Path) -> bool {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|extension| extension == "bat" || extension == "cmd")
}

#[cfg_attr(not(windows), allow(dead_code))]
fn invalid(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.to_string())
}

/// The command line that has `cmd.exe` run the batch file `script` with `arguments`, NOT
/// NUL-terminated — `std::process::Command`'s escaping for a batch file, for the ConPTY
/// spawn, which writes its own command line. `cmd.exe /e:ON /v:OFF /d /c ""SCRIPT" ARGS"`:
/// extensions on (the `%` neutraliser below needs them), delayed expansion off (so `!` is
/// literal), no AutoRun. An argument holding anything but letters, digits and
/// `#$*+-./:?@\_` is quoted, a `"` in it doubled, and each `%` written `%%cd:~,%` — an
/// empty substring of `%cd%` — so `%NAME%` cannot expand. A line break would end the
/// command, so an argument holding one is refused, as is a script name with a `"` in it.
/// A `raw` argument is written as given: the caller has quoted it for `cmd.exe` itself.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn batch_command_line(
    script: &OsStr,
    arguments: &[String],
    raw: bool,
) -> std::io::Result<String> {
    let script = script.to_string_lossy();
    if script.contains('"') || script.ends_with('\\') {
        return Err(invalid(
            "a batch file's name cannot contain a double quote or end with a backslash",
        ));
    }
    let mut line = format!("cmd.exe /e:ON /v:OFF /d /c \"\"{script}\"");
    for argument in arguments {
        line.push(' ');
        if raw {
            line.push_str(argument);
        } else {
            append_batch_argument(&mut line, argument)?;
        }
    }
    line.push('"');
    Ok(line)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn append_batch_argument(line: &mut String, argument: &str) -> std::io::Result<()> {
    if argument.contains(['\r', '\n', '\0']) {
        return Err(invalid(
            "a batch file's argument cannot hold a line break or a NUL: cmd.exe would end the \
             command there",
        ));
    }
    const UNQUOTED: &str = r"#$*+-./:?@\_";
    let quote = argument.is_empty()
        || argument.ends_with('\\')
        || argument.chars().any(|character| {
            (character.is_ascii()
                && !(character.is_ascii_alphanumeric() || UNQUOTED.contains(character)))
                || character.is_control()
        });
    if quote {
        line.push('"');
    }
    let mut backslashes = 0usize;
    for character in argument.chars() {
        if character == '\\' {
            backslashes += 1;
        } else {
            if character == '"' {
                // the run of backslashes doubled, and the quote doubled
                line.extend(std::iter::repeat_n('\\', backslashes));
                line.push('"');
            } else if character == '%' {
                line.push_str("%%cd:~,");
            }
            backslashes = 0;
        }
        line.push(character);
    }
    if quote {
        line.extend(std::iter::repeat_n('\\', backslashes));
        line.push('"');
    }
    Ok(())
}

/// The system's `cmd.exe`, by absolute path — never `COMSPEC` (anyone can set it) nor a
/// search (the current directory could hold one).
#[cfg(windows)]
pub(crate) fn system_command_interpreter() -> std::io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let mut buffer = vec![0u16; 260];
    // SAFETY: a live buffer of the length passed; the call writes at most that many units.
    let length = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(std::io::Error::last_os_error());
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])).join("cmd.exe"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn search_in<'a>(windows: bool, path: &str, runnable: &'a dyn Fn(&Path) -> bool) -> Search<'a> {
        Search {
            windows,
            path: Some(OsString::from(path)),
            extensions: DEFAULT_PATHEXT.to_string(),
            base: PathBuf::from("/work"),
            runnable,
        }
    }

    fn files(names: &[&str]) -> HashSet<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn windows_finds_a_batch_file_or_a_shim_by_its_bare_name() {
        let present = files(&["/w/bin/mix.bat", "/w/npm/tsc", "/w/npm/tsc.cmd"]);
        let runnable = |path: &Path| present.contains(path);
        let search = search_in(true, "/w/bin;/w/npm", &runnable);
        assert_eq!(search.find("mix"), Some(PathBuf::from("/w/bin/mix.bat")));
        // the extensionless `sh` shim is never the answer
        assert_eq!(search.find("tsc"), Some(PathBuf::from("/w/npm/tsc.cmd")));
        assert_eq!(
            search.find("tsc.cmd"),
            Some(PathBuf::from("/w/npm/tsc.cmd"))
        );
        assert_eq!(search.find("nothing"), None);
    }

    #[test]
    fn windows_tries_the_extensions_in_pathext_order_per_directory() {
        let present = files(&["/a/tool.cmd", "/a/tool.exe", "/b/tool.com"]);
        let runnable = |path: &Path| present.contains(path);
        let search = search_in(true, "/a;/b", &runnable);
        // the first directory wins; within it, PATHEXT's order (.EXE before .CMD)
        assert_eq!(search.find("tool"), Some(PathBuf::from("/a/tool.exe")));
    }

    #[test]
    fn the_working_directory_is_searched_only_when_path_says_so() {
        let present = files(&["/work/bash.exe", "/sys/bash.exe"]);
        let runnable = |path: &Path| present.contains(path);
        // a planted bash.exe in the working directory is not run …
        assert_eq!(
            search_in(true, "/sys", &runnable).find("bash"),
            Some(PathBuf::from("/sys/bash.exe"))
        );
        // … unless PATH has an empty entry, which means the working directory
        assert_eq!(
            search_in(true, ";/sys", &runnable).find("bash"),
            Some(PathBuf::from("./bash.exe"))
        );
    }

    #[test]
    fn no_path_finds_nothing() {
        let runnable = |_: &Path| true;
        let mut search = search_in(false, "", &runnable);
        assert_eq!(search.find("sh"), None);
        search.path = None;
        assert_eq!(search.find("sh"), None);
    }

    #[test]
    fn a_name_with_a_separator_is_taken_against_the_working_directory() {
        let present = files(&["/work/./build.sh", "/work/tools\\run.bat"]);
        let runnable = |path: &Path| present.contains(path);
        let unix = search_in(false, "/bin", &runnable);
        assert_eq!(unix.find("./build.sh"), Some(PathBuf::from("./build.sh")));
        assert_eq!(unix.find("./missing"), None);
        // on Windows a backslash names a path too, and the extension is completed
        let windows = search_in(true, "/bin", &runnable);
        assert_eq!(
            windows.find("tools\\run"),
            Some(PathBuf::from("tools\\run.bat"))
        );
    }

    #[test]
    fn resolve_answers_an_absolute_path_and_which_agrees() {
        let shell = resolve("sh", None, &[]).expect("sh is on PATH");
        assert!(shell.is_absolute());
        assert_eq!(
            which("sh", None, &[]).map(|found| found.is_absolute()),
            Some(true)
        );
        // the child's PATH, not ours
        let empty: Vec<EnvEntry> = vec![("PATH".into(), Some(String::new()))];
        assert!(resolve("sh", None, &empty).is_err());
        assert_eq!(which("sh", None, &empty), None);
        let error = resolve("brood-surely-no-such-program", None, &[]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(error.to_string().contains("not found on PATH"));
    }

    #[test]
    fn batch_arguments_cannot_reach_the_command_interpreter() {
        let line = |arguments: &[&str]| {
            let arguments: Vec<String> = arguments.iter().map(|a| a.to_string()).collect();
            batch_command_line(OsStr::new(r"C:\bin\mix.bat"), &arguments, false)
        };
        assert_eq!(
            line(&["test", "a b"]).unwrap(),
            r#"cmd.exe /e:ON /v:OFF /d /c ""C:\bin\mix.bat" test "a b"""#
        );
        // `&` stays inside quotes, a `"` is doubled so it cannot close them
        assert_eq!(
            line(&["\"&calc"]).unwrap(),
            r#"cmd.exe /e:ON /v:OFF /d /c ""C:\bin\mix.bat" """&calc""#.to_string() + "\""
        );
        // `%PATH%` cannot expand
        assert_eq!(
            line(&["%PATH%"]).unwrap(),
            r#"cmd.exe /e:ON /v:OFF /d /c ""C:\bin\mix.bat" "%%cd:~,%PATH%%cd:~,%"""#
        );
        // a trailing backslash is quoted, and doubled before the closing quote
        assert_eq!(
            line(&[r"C:\dir\"]).unwrap(),
            r#"cmd.exe /e:ON /v:OFF /d /c ""C:\bin\mix.bat" "C:\dir\\"""#
        );
        assert_eq!(
            line(&[""]).unwrap(),
            r#"cmd.exe /e:ON /v:OFF /d /c ""C:\bin\mix.bat" """"#
        );
        assert!(line(&["a\nb"]).is_err());
        assert!(batch_command_line(OsStr::new("x\"y.bat"), &[], false).is_err());
    }

    #[test]
    fn a_batch_file_is_told_by_its_extension() {
        assert!(is_batch_file(Path::new(r"C:\bin\mix.BAT")));
        assert!(is_batch_file(Path::new("tsc.cmd")));
        assert!(!is_batch_file(Path::new("git.exe")));
        assert!(!is_batch_file(Path::new("mix")));
    }
}
