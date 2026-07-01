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

/// `(audio-beep freq-hz ms [vol])` — play a short tone at peak amplitude `vol`
/// (0..1). No-op without `--features audio`.
#[cfg(not(feature = "audio"))]
pub fn beep(_freq: f32, _ms: u64, _vol: f32) {}

#[cfg(feature = "audio")]
pub fn beep(freq: f32, ms: u64, vol: f32) {
    backend::beep(freq, ms, vol);
}

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
        // Clamp to a sane amplitude; non-finite or <=0 falls back to the default.
        let vol = if vol.is_finite() && vol > 0.0 {
            vol.min(1.0)
        } else {
            VOLUME
        };
        if let Some(tx) = sender() {
            let _ = tx.send(Beep { freq, ms, vol });
        }
    }
}
