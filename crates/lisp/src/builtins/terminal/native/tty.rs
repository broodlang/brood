//! The terminal's input bytes, read here (Unix) and decoded by [`super::decoder`] (KI-360).
//!
//! crossterm reads the tty itself; Brood reads it instead, so the decoder sees every byte.
//! The terminal is stdin when stdin is one, else `/dev/tty` (as crossterm chooses). A resize
//! is not a byte — crossterm learned of it from SIGWINCH — so the keyboard compares the
//! terminal's size as it reads (`keyboard::absorb`).

use std::io;
use std::os::fd::RawFd;
use std::time::{Duration, Instant};

use crossterm::event::Event;

use super::decoder::Decoder;

/// How long a part-read sequence may wait for the rest of its bytes before it is given up:
/// a terminal writes a sequence in one go, so only a lost byte leaves one waiting this long.
const SEQUENCE_PATIENCE: Duration = Duration::from_millis(100);

/// The tty's input: its descriptor, once opened, and the decoder's state between reads.
pub(super) struct TtyInput {
    fd: Option<RawFd>,
    decoder: Decoder,
    /// When the decoder last took bytes, so a sequence that never completes is let go.
    last_bytes: Option<Instant>,
}

impl TtyInput {
    pub(super) const fn new() -> Self {
        TtyInput {
            fd: None,
            decoder: Decoder::new(),
            last_bytes: None,
        }
    }

    /// The terminal's descriptor: stdin when it is a terminal, else `/dev/tty`, opened once.
    pub(super) fn fd(&mut self) -> io::Result<RawFd> {
        if let Some(fd) = self.fd {
            return Ok(fd);
        }
        // SAFETY: isatty and open only read their arguments; the path is NUL-terminated.
        let fd = unsafe {
            if libc::isatty(libc::STDIN_FILENO) == 1 {
                libc::STDIN_FILENO
            } else {
                let opened = libc::open(c"/dev/tty".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
                if opened < 0 {
                    return Err(io::Error::last_os_error());
                }
                opened
            }
        };
        self.fd = Some(fd);
        Ok(fd)
    }

    /// How long what is part-read may wait for its rest: an ESC that may be Meta on a key
    /// still to come ([`super::decoder::ESCAPE_PATIENCE`]), any other sequence
    /// [`SEQUENCE_PATIENCE`].
    fn patience(&self) -> Duration {
        if self.decoder.key_undecided() {
            super::decoder::ESCAPE_PATIENCE
        } else {
            SEQUENCE_PATIENCE
        }
    }

    /// Whether a sequence or a paste is part-read: its rest, or the patience that lets it go,
    /// is due soon, so a wait for input must not outlast [`SEQUENCE_PATIENCE`].
    pub(super) fn part_read(&self) -> bool {
        self.decoder.waiting() || self.decoder.in_paste()
    }

    /// Say whether a question to the terminal is outstanding ([`Decoder::await_replies`]).
    pub(super) fn await_replies(&mut self, awaiting: bool) {
        self.decoder.await_replies(awaiting);
    }

    /// The terminal's answers the decoder has read since the last take (ADR-740).
    pub(super) fn take_replies(&mut self) -> Vec<super::probe::Reply> {
        self.decoder.take_replies()
    }

    /// Decode every byte the tty has waiting, without waiting, onto `out` — at most `room`
    /// events. True when the tty was drained (nothing more is waiting).
    pub(super) fn read_available(&mut self, out: &mut Vec<Event>, room: usize) -> io::Result<bool> {
        let fd = self.fd()?;
        let mut buffer = [0u8; 4096];
        while out.len() < room {
            if !readable(fd, Duration::ZERO)? {
                let silent_for = self.last_bytes.map_or(Duration::ZERO, |at| at.elapsed());
                if super::decoder::paste_stalled(self.decoder.in_paste(), silent_for) {
                    // a paste whose end marker was lost: hand it over, keys are keys again
                    self.decoder.give_up_paste(out);
                } else if self.decoder.waiting() && silent_for >= self.patience() {
                    self.decoder.flush(out);
                }
                return Ok(true);
            }
            // SAFETY: the buffer is valid for its length, the descriptor open.
            let read = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
            if read < 0 {
                let error = io::Error::last_os_error();
                return match error.kind() {
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => Ok(true),
                    _ => Err(error),
                };
            }
            if read == 0 {
                // the terminal hung up: waiting on it again would return at once, forever
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
            let more = readable(fd, Duration::ZERO)?;
            self.decoder.feed(&buffer[..read as usize], more, out);
            self.last_bytes = Some(Instant::now());
        }
        Ok(false)
    }
}

/// How many SIGWINCHes this process has had: a resize is reported when it moves, even when
/// the terminal is back at the size it was at the last look (A → B → A between two reads
/// still repaints — the terminal reflowed the screen in between).
static RESIZE_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

extern "C" fn count_resize(_: libc::c_int) {
    RESIZE_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    wake();
}

/// A pipe whose read end a wait for input polls beside the tty, so a resize (which writes no
/// byte to the tty) or a stop request ends the wait at once instead of the waiter waking
/// every few milliseconds to look. Made once, by [`watch_resizes`]; both ends non-blocking.
static WAKE_PIPE: std::sync::OnceLock<(RawFd, RawFd)> = std::sync::OnceLock::new();

/// End any wait for input now ([`readable_or_woken`]). Async-signal-safe: one `write` to a
/// non-blocking pipe, a full pipe being a wake already pending.
pub(super) fn wake() {
    if let Some(&(_, write_end)) = WAKE_PIPE.get() {
        // SAFETY: one byte from a valid buffer to a descriptor this process owns.
        unsafe {
            libc::write(write_end, [1u8].as_ptr().cast(), 1);
        }
    }
}

/// Count SIGWINCH from now on (once per process; the handler only bumps a counter and writes
/// to the wake pipe, both async-signal-safe). The keyboard reads [`resize_generation`].
pub(super) fn watch_resizes() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let mut ends = [0 as RawFd; 2];
        // SAFETY: `ends` holds the two descriptors pipe2 writes.
        if unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } == 0 {
            let _ = WAKE_PIPE.set((ends[0], ends[1]));
        }
        // SAFETY: a zeroed sigaction with a handler that touches only an atomic and the pipe.
        unsafe {
            let handler: extern "C" fn(libc::c_int) = count_resize;
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as usize as libc::sighandler_t;
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(libc::SIGWINCH, &action, std::ptr::null_mut());
        }
    });
}

pub(super) fn resize_generation() -> u64 {
    RESIZE_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether `fd` has bytes to read and whether [`wake`] was called, waiting up to `timeout`
/// for either — a wake ends the wait at once, so the caller looks at what woke it (a resize,
/// a stop). Without a wake pipe, [`readable`] and never woken.
pub(super) fn readable_or_woken(fd: RawFd, timeout: Duration) -> io::Result<(bool, bool)> {
    let Some(&(wake_end, _)) = WAKE_PIPE.get() else {
        return readable(fd, timeout).map(|input| (input, false));
    };
    let mut watched = [watch(fd), watch(wake_end)];
    wait_for_any(&mut watched, timeout)?;
    let woken = has_input(&watched[1]);
    if woken {
        let mut drain = [0u8; 64];
        // SAFETY: the buffer is valid for its length; the pipe is non-blocking.
        while unsafe { libc::read(wake_end, drain.as_mut_ptr().cast(), drain.len()) } > 0 {}
    }
    Ok((has_input(&watched[0]), woken))
}

/// Whether `fd` has bytes to read, waiting up to `timeout` for some. An interrupted wait
/// (a signal) is "nothing yet".
pub(super) fn readable(fd: RawFd, timeout: Duration) -> io::Result<bool> {
    let mut watched = [watch(fd)];
    wait_for_any(&mut watched, timeout)?;
    Ok(has_input(&watched[0]))
}

/// `fd` watched for input by [`wait_for_any`].
fn watch(fd: RawFd) -> libc::pollfd {
    libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }
}

/// Wait up to `timeout` for any of `watched` to have input (`poll`), each then saying so by
/// [`has_input`]. An interrupted wait (a signal) is nothing ready, not an error.
fn wait_for_any(watched: &mut [libc::pollfd], timeout: Duration) -> io::Result<()> {
    let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
    // SAFETY: valid pollfds, the count says how many.
    let ready = unsafe { libc::poll(watched.as_mut_ptr(), watched.len() as libc::nfds_t, millis) };
    if ready < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
        watched.iter_mut().for_each(|entry| entry.revents = 0);
    }
    Ok(())
}

/// Whether a [`wait_for_any`] found input on `watched` — or a hang-up, which a read then
/// reports.
fn has_input(watched: &libc::pollfd) -> bool {
    watched.revents & (libc::POLLIN | libc::POLLHUP) != 0
}
