//! The window's sound: the sequence's mix on the audio device, and the clock
//! the picture follows while it plays.
//!
//! An audio thread pumps [`AudioScheduler`] every few milliseconds, so the
//! device is fed whether or not the event loop is awake. The picture's
//! [`PlaybackClock`](cine_media::PlaybackClock) runs on [`AppTime`], which reads
//! the frames the device has *played* while sound is leading, and the system
//! clock otherwise — at other speeds, in reverse, or with no device.
//!
//! ## Switching clocks without a jump
//!
//! A playback clock measures elapsed source time from an anchor it takes when it
//! starts, pauses or changes rate. Two sources measure from different origins,
//! so a switch is only safe while the clock is **not advancing**, where its
//! position is its anchor and the next start re-anchors on the new source. The
//! window keeps that rule: it pauses before it switches.

use cine_audio_io::AlsaSink;
use cine_media::av::{AudioScheduler, AudioSink, AudioTimeSource};
use cine_media::clock::{SystemTime, TimeSource};
use cine_media::{SequenceAudio, SoundEntry};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use ucg_core::{Rate, RationalTime};

/// Elapsed time from the audio device while sound leads, otherwise the system.
#[derive(Clone)]
pub struct AppTime {
    system: Arc<SystemTime>,
    audio: Option<AudioTimeSource>,
    leading: Arc<AtomicBool>,
}

impl AppTime {
    pub fn system() -> AppTime {
        AppTime {
            system: Arc::new(SystemTime::default()),
            audio: None,
            leading: Arc::new(AtomicBool::new(false)),
        }
    }

    fn with_audio(audio: AudioTimeSource, leading: Arc<AtomicBool>) -> AppTime {
        AppTime {
            system: Arc::new(SystemTime::default()),
            audio: Some(audio),
            leading,
        }
    }

    fn audio(&self) -> Option<&AudioTimeSource> {
        self.audio
            .as_ref()
            .filter(|_| self.leading.load(Ordering::Relaxed))
    }
}

impl TimeSource for AppTime {
    fn elapsed(&self) -> Duration {
        match self.audio() {
            Some(a) => a.elapsed(),
            None => self.system.elapsed(),
        }
    }
    fn elapsed_exact(&self) -> Option<RationalTime> {
        self.audio().and_then(TimeSource::elapsed_exact)
    }
}

type Audio = AudioScheduler<AlsaSink, SequenceAudio>;

/// The sequence's sound on the device.
pub struct SoundEngine {
    audio: Arc<Mutex<Audio>>,
    playing: Arc<AtomicBool>,
    leading: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    time: AppTime,
    /// Picture presentation time minus audible sound time, at each presented
    /// picture while sound led, in milliseconds.
    pub offsets_ms: Vec<f64>,
    pub device: String,
}

pub const RATE: u32 = 48_000;

impl SoundEngine {
    /// Open the default device and prepare the sequence's mix. `None`, with the
    /// reason, when there is no device or nothing to play.
    pub fn open(end: RationalTime, entries: Vec<SoundEntry>) -> Result<SoundEngine, String> {
        if entries.is_empty() {
            return Err("the sequence has no audible sound clips".into());
        }
        AlsaSink::available()?;
        let sink = AlsaSink::open("default", RATE, 2, 50_000).map_err(|e| e.to_string())?;
        let device = format!(
            "{} ({} frames buffer, {} period)",
            sink.info().name,
            sink.info().buffer_frames,
            sink.info().period_frames
        );
        let rate = Rate::new(RATE, 1).expect("non-zero");
        let source = SequenceAudio::new(rate, end, entries);
        let audio = AudioScheduler::new(sink, source, RationalTime::zero(rate))
            .map_err(|e| e.to_string())?;
        let leading = Arc::new(AtomicBool::new(false));
        let time = AppTime::with_audio(audio.time_source(), leading.clone());
        let audio = Arc::new(Mutex::new(audio));
        let playing = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (audio, playing, stop) = (audio.clone(), playing.clone(), stop.clone());
            std::thread::Builder::new()
                .name("cine-sound".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        if playing.load(Ordering::Relaxed) {
                            if let Ok(mut a) = audio.lock() {
                                if let Err(e) = a.pump() {
                                    eprintln!("sound stopped: {e}");
                                    playing.store(false, Ordering::Relaxed);
                                }
                            }
                        }
                        std::thread::sleep(Duration::from_millis(4));
                    }
                })
                .map_err(|e| e.to_string())?
        };
        Ok(SoundEngine {
            audio,
            playing,
            leading,
            stop,
            thread: Some(thread),
            time,
            offsets_ms: Vec::new(),
            device,
        })
    }

    /// The time source the picture's clock must be built on.
    pub fn time(&self) -> AppTime {
        self.time.clone()
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    /// Start sound at `position` and make it lead the picture. The picture's
    /// clock must not be advancing when this is called.
    pub fn play_from(&self, position: RationalTime) {
        if let Ok(mut a) = self.audio.lock() {
            let _ = a.seek(position);
        }
        self.leading.store(true, Ordering::Relaxed);
        self.playing.store(true, Ordering::Relaxed);
    }

    /// Stop sound and hand the clock back to the system. The picture's clock
    /// must already be paused.
    pub fn stop(&self, position: RationalTime) {
        self.playing.store(false, Ordering::Relaxed);
        if let Ok(mut a) = self.audio.lock() {
            let _ = a.seek(position);
        }
        self.leading.store(false, Ordering::Relaxed);
    }

    /// Record the picture/sound offset for a presented picture.
    pub fn observe(&mut self, picture: RationalTime) {
        if !self.is_playing() {
            return;
        }
        if let Ok(a) = self.audio.lock() {
            let heard = a.position();
            self.offsets_ms
                .push((picture.to_seconds() - heard.to_seconds()) * 1e3);
        }
    }

    /// One line of evidence.
    pub fn report(&self) -> String {
        let (stats, queued, refusals, clipped) = match self.audio.lock() {
            Ok(a) => (
                a.stats(),
                a.sink().queued(),
                a.source().refusals().clone(),
                a.source().clipped(),
            ),
            Err(_) => return "sound: engine lock poisoned".into(),
        };
        let mut o = self.offsets_ms.clone();
        o.sort_by(|a, b| a.total_cmp(b));
        let pick = |p: usize| o.get(o.len().saturating_sub(1) * p / 100).copied();
        format!(
            "sound: device {}; wrote {} frames, {} underruns, worst queue {} frames, now {}; \
             picture minus sound at presentation: median {} ms, 5–95% {}..{} ms over {} pictures; \
             {} clipped samples; refused: {:?}",
            self.device,
            stats.frames_written,
            stats.underruns,
            stats.worst_queued,
            queued,
            pick(50).map_or("—".into(), |v| format!("{v:.1}")),
            pick(5).map_or("—".into(), |v| format!("{v:.1}")),
            pick(95).map_or("—".into(), |v| format!("{v:.1}")),
            o.len(),
            clipped,
            refusals
        )
    }
}

impl Drop for SoundEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
