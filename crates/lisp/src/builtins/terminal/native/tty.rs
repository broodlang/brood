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

    /// Decode every byte the tty has waiting, without waiting, onto `out` — at most `room`
    /// events. True when the tty was drained (nothing more is waiting).
    pub(super) fn read_available(&mut self, out: &mut Vec<Event>, room: usize) -> io::Result<bool> {
        let fd = self.fd()?;
        let mut buffer = [0u8; 4096];
        while out.len() < room {
            if !readable(fd, Duration::ZERO)? {
                if self.decoder.waiting()
                    && self
                        .last_bytes
                        .is_some_and(|at| at.elapsed() >= SEQUENCE_PATIENCE)
                {
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

/// Whether `fd` has bytes to read, waiting up to `timeout` for some. An interrupted wait
/// (a signal) is "nothing yet".
pub(super) fn readable(fd: RawFd, timeout: Duration) -> io::Result<bool> {
    let mut poll_fd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
    // SAFETY: one valid pollfd, the count says so.
    let ready = unsafe { libc::poll(&mut poll_fd, 1, millis) };
    if ready < 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::Interrupted {
            Ok(false)
        } else {
            Err(error)
        };
    }
    Ok(ready > 0 && poll_fd.revents & (libc::POLLIN | libc::POLLHUP) != 0)
}
