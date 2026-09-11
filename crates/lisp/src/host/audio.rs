//! Audio output backend — the `audio-beep` builtin's engine (feature `audio`,
//! pulled in by `gui`). Like `gui.rs`, the symbol always exists; without the
//! feature it's a no-op, so the lean runtime links no audio stack.
//!
//! rodio's output stream is `!Send`, so (mirroring the gui thread) a dedicated
//! `brood-audio` thread owns the device and is fed `Beep` commands over a channel.
//! `beep` only sends — it never blocks the caller — so a game can fire a blip from
//! its frame loop with no latency. Muted (a graceful no-op) when there's no audio
//! device, when `BROOD_AUDIO=0`, or under `BROOD_GUI_HEADLESS` (so tests stay
//! silent). Beeps are synthesised sine tones mixed concurrently, so overlapping
//! sounds (a hit during a score jingle) just stack.

/// `(audio/beep freq-hz ms [vol])` — play a short tone at peak amplitude `vol`
/// (0..1). No-op without `--features audio`.
#[cfg(not(feature = "audio"))]
pub fn beep(_freq: f32, _ms: u64, _vol: f32) {}

#[cfg(feature = "audio")]
pub fn beep(freq: f32, ms: u64, vol: f32) {
    backend::beep(freq, ms, vol);
}

/// The longest a single `audio-beep` may run. `audio-beep` is fire-and-forget:
/// nothing ever stops a tone early, and the mixer holds each one until it ends.
/// So an unbounded duration is not "a long beep", it is a source that never
/// leaves the mixer — and since the primitive reaches this through
/// `ms.max(0.0) as u64`, a float `ms` of `1e300` saturates to `u64::MAX`
/// (~584 million years) instead of erroring. One stray expression in a frame loop
/// then piles up permanent tones for the process's life. 30 s is far past any
/// game blip or jingle and is what a *bounded* mistake costs.
#[cfg(feature = "audio")]
const MAX_BEEP_MS: u64 = 30_000;

/// The tone range a beep may ask for, in Hz — roughly human hearing. Outside it
/// there is nothing to hear, and the ends are actively bad: `SineWave::new` takes
/// the frequency straight into `sin(2*pi*f*t)`, so a NaN/inf `freq-hz` (which
/// `num_to_f64(…) as f32` will happily produce from `(/ 0.0 0.0)`) feeds NaN
/// samples into the shared mixer, where they contaminate every tone stacked
/// alongside them rather than just the bad one.
#[cfg(feature = "audio")]
const MIN_BEEP_HZ: f32 = 1.0;
#[cfg(feature = "audio")]
const MAX_BEEP_HZ: f32 = 20_000.0;

#[cfg(feature = "audio")]
mod backend {
    use rodio::source::SineWave;
    use rodio::{DeviceSinkBuilder, Source};
    use std::sync::mpsc::{self, Sender};
    use std::sync::OnceLock;
    use std::time::Duration;

    /// Default peak amplitude (0..1) — modest so stacked tones don't clip.
    const VOLUME: f32 = 0.18;

    struct Beep {
        freq: f32,
        ms: u64,
        vol: f32,
    }

    fn muted() -> bool {
        let on = |k: &str| {
            std::env::var(k)
                .map(|v| v != "0" && !v.is_empty())
                .unwrap_or(false)
        };
        on("BROOD_GUI_HEADLESS")
            || std::env::var("BROOD_AUDIO")
                .map(|v| v == "0")
                .unwrap_or(false)
    }

    /// The channel to the audio thread, started on first use. `None` when muted or
    /// the thread couldn't start; the audio thread itself exits quietly if there's
    /// no output device, after which sends are harmless no-ops.
    fn sender() -> Option<&'static Sender<Beep>> {
        static S: OnceLock<Option<Sender<Beep>>> = OnceLock::new();
        S.get_or_init(|| {
            if muted() {
                return None;
            }
            let (tx, rx) = mpsc::channel::<Beep>();
            let started = std::thread::Builder::new()
                .name("brood-audio".into())
                .spawn(move || {
                    // Own the device for the thread's life (the sink must stay alive).
                    let stream = match DeviceSinkBuilder::open_default_sink() {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    let mixer = stream.mixer();
                    while let Ok(b) = rx.recv() {
                        let tone = SineWave::new(b.freq)
                            .take_duration(Duration::from_millis(b.ms))
                            .amplify(b.vol);
                        // `add` mixes concurrently, so overlapping beeps stack.
                        mixer.add(tone);
                    }
                });
            match started {
                Ok(_) => Some(tx),
                Err(_) => None,
            }
        })
        .as_ref()
    }

    pub fn beep(freq: f32, ms: u64, vol: f32) {
        use super::{MAX_BEEP_HZ, MAX_BEEP_MS, MIN_BEEP_HZ};
        // Clamp to a sane amplitude; non-finite or <=0 falls back to the default.
        let vol = if vol.is_finite() && vol > 0.0 {
            vol.min(1.0)
        } else {
            VOLUME
        };
        // A non-finite frequency has no tone to play and would poison the shared
        // mixer with NaN samples, so it is dropped rather than clamped — unlike
        // `vol`, there is no sensible default pitch to substitute. A finite one is
        // clamped into the audible band. A zero-length beep is likewise nothing to
        // play, and the length is capped so a fire-and-forget tone always ends.
        if !freq.is_finite() || ms == 0 {
            return;
        }
        let freq = freq.clamp(MIN_BEEP_HZ, MAX_BEEP_HZ);
        let ms = ms.min(MAX_BEEP_MS);
        if let Some(tx) = sender() {
            let _ = tx.send(Beep { freq, ms, vol });
        }
    }
}
