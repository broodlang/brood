//! Bounding unauthenticated peers before a handshake completes: the whole-handshake
//! [`Deadline`] a trickling peer cannot restart, the [`HandshakeSlot`] permit that caps
//! handshakes in flight, and the rate-limited warning when connections are shed at that
//! cap.

use super::*;

/// A `Read`/`Write` shim that fails once a wall-clock deadline passes — the
/// whole-handshake bound the socket's `SO_RCVTIMEO` cannot express.
///
/// The distinction is the entire point: a socket read timeout restarts on every
/// byte, so a peer sending one byte just inside it stays alive indefinitely
/// while holding a [`HandshakeSlot`]. This checks an *absolute* instant that no
/// amount of progress moves, so a trickling peer is cut off on schedule.
///
/// Writes are covered too, not just reads: a peer that completes its side and
/// then stops reading would otherwise park us in `write_all` against a full
/// socket buffer with no timeout at all — the same slot held, by the other
/// direction.
///
/// The deadline is checked *around* each call rather than by shortening the
/// socket timeout, so a single blocking read can still overshoot by at most one
/// [`HANDSHAKE_TIMEOUT`]; the bound is the sum, which is what matters against an
/// unbounded hold.
pub(super) struct Deadline<'a, S> {
    inner: &'a mut S,
    until: Instant,
}

impl<'a, S> Deadline<'a, S> {
    pub(super) fn new(inner: &'a mut S, budget: Duration) -> Self {
        Self {
            inner,
            until: Instant::now() + budget,
        }
    }

    fn check(&self) -> io::Result<()> {
        if Instant::now() >= self.until {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "handshake exceeded its deadline",
            ));
        }
        Ok(())
    }
}

impl<S: Read> Read for Deadline<'_, S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        let n = self.inner.read(buf)?;
        // Also *after* the call: a read that dribbled back a byte just under the
        // socket timeout must not be allowed to start another one past the
        // deadline. `read_exact` loops on short reads, and this is the loop's
        // brake.
        self.check()?;
        Ok(n)
    }
}

impl<S: Write> Write for Deadline<'_, S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.check()?;
        let n = self.inner.write(buf)?;
        self.check()?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.check()?;
        self.inner.flush()
    }
}

/// How many inbound connections have been shed at the [`MAX_IN_FLIGHT_HANDSHAKES`]
/// cap since start.
pub(super) static SHED_HANDSHAKES: AtomicU64 = AtomicU64::new(0);

/// Millisecond timestamp of the last shed warning, for rate limiting.
///
/// `u64::MAX` — not 0 — is the "never warned yet" sentinel: `now_millis()` counts
/// from process start, so 0 is a perfectly real timestamp during the first
/// millisecond, and a 0 sentinel would silently swallow the first warning of a
/// flood that began at startup.
pub(super) const SHED_WARN_NEVER: u64 = u64::MAX;
pub(super) static LAST_SHED_WARN_MS: AtomicU64 = AtomicU64::new(SHED_WARN_NEVER);

/// Minimum gap between shed warnings. Long enough that a sustained flood costs
/// one line a minute, short enough that an operator watching a log sees it.
pub(super) const SHED_WARN_INTERVAL_MS: u64 = 60_000;

/// Count a shed inbound connection and, at most once per
/// [`SHED_WARN_INTERVAL_MS`], say so.
///
/// The shed itself is correct and stays correct — past the cap, closing the
/// socket is the whole response. It must not be **silent**: hitting the cap means
/// *every further inbound link is refused*, i.e. inbound distribution is
/// effectively down (the KI-36 lesson about silent drops).
///
/// Rate-limited rather than per-shed, because the trigger is by definition a
/// flood and a line each would be its own amplification vector — the same shape
/// as the ADR-232 per-name dedup. The cumulative count rides along, so one line
/// still conveys the scale.
pub(super) fn note_shed_handshake() {
    let total = SHED_HANDSHAKES.fetch_add(1, Ordering::Relaxed) + 1;
    let now = now_millis();
    let last = LAST_SHED_WARN_MS.load(Ordering::Relaxed);
    // The first shed ever always warns; afterwards, once per interval.
    if last != SHED_WARN_NEVER && now.saturating_sub(last) < SHED_WARN_INTERVAL_MS {
        return;
    }
    // Only the thread that wins the stamp prints, so a burst yields one line.
    if LAST_SHED_WARN_MS
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    eprintln!(
        "dist: inbound connection shed — {MAX_IN_FLIGHT_HANDSHAKES} handshakes already in \
         flight ({total} shed so far). Inbound links are being refused; if this is not a \
         flood, a peer may be connecting without completing its handshake."
    );
}

/// RAII permit for one in-flight handshake slot (see [`MAX_IN_FLIGHT_HANDSHAKES`]).
/// Held by the per-connection thread for the whole pre-auth window; released on
/// drop (thread end), whether the handshake succeeded, failed, or timed out.
pub(super) struct HandshakeSlot {
    count: &'static AtomicUsize,
}
impl HandshakeSlot {
    /// Take a slot, or `None` if the cap is already reached (caller sheds the
    /// connection). The over-count from a losing `fetch_add` is immediately
    /// rolled back, so the gate can't drift above the cap under contention.
    pub(super) fn try_acquire() -> Option<Self> {
        Self::try_acquire_on(&IN_FLIGHT_HANDSHAKES)
    }

    /// [`try_acquire`](Self::try_acquire) against `count` — the process's counter in
    /// production, a test's own one in the test (sibling test threads hold real slots).
    pub(super) fn try_acquire_on(count: &'static AtomicUsize) -> Option<Self> {
        if count.fetch_add(1, Ordering::AcqRel) >= MAX_IN_FLIGHT_HANDSHAKES {
            count.fetch_sub(1, Ordering::AcqRel);
            None
        } else {
            Some(HandshakeSlot { count })
        }
    }
}
impl Drop for HandshakeSlot {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::AcqRel);
    }
}
