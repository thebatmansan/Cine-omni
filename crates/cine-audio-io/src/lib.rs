//! # cine-audio-io — sound that reaches a speaker
//!
//! `cine_media::av` defines what a sink must be and paces the picture from the
//! frames a sink has *played*. This crate is the first sink that reaches
//! hardware: ALSA's PCM playback interface, through whatever the system's
//! `default` device is — on this machine, PipeWire.
//!
//! ## How ALSA is reached, and why that way
//!
//! `libasound` is loaded at runtime with `dlopen`. Nothing of ALSA is compiled
//! in or linked at build time, which keeps the build free of system headers and
//! a C toolchain, keeps the licence boundary plain (dynamic use of the system's
//! LGPL-2.1 library, nothing redistributed), and lets a machine without ALSA
//! start normally and report audio as unavailable.
//!
//! ## What "played" means here
//!
//! ALSA reports `snd_pcm_delay`: frames handed over that have not yet reached
//! the speaker, including the device's own buffering. Played is written minus
//! that delay, and never goes backwards. That is exactly the quantity
//! `AudioTimeSource` needs, and it is why picture paced from it lines up with
//! what is heard rather than with what was queued.
//!
//! ## What is not claimed
//!
//! That a person heard it. A test can observe that the device accepted and
//! consumed frames at a rate matching the system clock, and at what delay; it
//! cannot observe a room. The measurements in `AV-SYNC.md` say which is which.

use cine_media::av::{AudioSink, PlayedFrames};
use cine_media::{Error, Result};
use libloading::{Library, Symbol};
use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void, CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use ucg_core::Rate;

const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_NONBLOCK: c_int = 1;
const SND_PCM_FORMAT_FLOAT_LE: c_int = 14;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
const EAGAIN: c_int = 11;

type PcmOpen = unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int;
type PcmSetParams =
    unsafe extern "C" fn(*mut c_void, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int;
type PcmWritei = unsafe extern "C" fn(*mut c_void, *const c_void, c_ulong) -> c_long;
type PcmAvail = unsafe extern "C" fn(*mut c_void) -> c_long;
type PcmDelay = unsafe extern "C" fn(*mut c_void, *mut c_long) -> c_int;
type PcmRecover = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int;
type PcmSimple = unsafe extern "C" fn(*mut c_void) -> c_int;
type PcmGetParams = unsafe extern "C" fn(*mut c_void, *mut c_ulong, *mut c_ulong) -> c_int;
type StrError = unsafe extern "C" fn(c_int) -> *const c_char;

/// What a device reported about itself when it was opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub name: String,
    pub rate: u32,
    pub channels: usize,
    /// The device's ring buffer, in frames.
    pub buffer_frames: u64,
    /// The device's period, in frames.
    pub period_frames: u64,
}

/// Counters a playback run can report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceStats {
    pub written: u64,
    pub xruns: u64,
    pub would_block: u64,
}

/// ALSA playback, non-blocking, 32-bit float interleaved.
pub struct AlsaSink {
    lib: Library,
    pcm: *mut c_void,
    info: DeviceInfo,
    rate: Rate,
    written: u64,
    played: PlayedFrames,
    stats: DeviceStats,
}

// The PCM handle is used by one thread at a time; moving it between threads is
// what ALSA permits.
unsafe impl Send for AlsaSink {}

impl AlsaSink {
    /// Whether this system can load ALSA at all.
    pub fn available() -> std::result::Result<(), String> {
        unsafe { Library::new("libasound.so.2") }
            .map(|_| ())
            .map_err(|e| format!("libasound could not be loaded: {e}"))
    }

    /// Open `device` (usually `"default"`) for playback.
    ///
    /// `latency_us` is the buffering asked for; the device decides what it
    /// grants, and [`AlsaSink::info`] reports it.
    pub fn open(device: &str, rate: u32, channels: usize, latency_us: u32) -> Result<AlsaSink> {
        let exact_rate = Rate::new(rate, 1)
            .ok_or_else(|| Error::Unsupported("a sample rate must be non-zero".into()))?;
        let lib = unsafe { Library::new("libasound.so.2") }
            .map_err(|e| Error::Unsupported(format!("no ALSA on this system: {e}")))?;
        let name =
            CString::new(device).map_err(|_| Error::Unsupported("bad device name".into()))?;
        let mut pcm: *mut c_void = std::ptr::null_mut();
        let mut sink = AlsaSink {
            lib,
            pcm: std::ptr::null_mut(),
            info: DeviceInfo {
                name: device.to_string(),
                rate,
                channels,
                buffer_frames: 0,
                period_frames: 0,
            },
            rate: exact_rate,
            written: 0,
            played: Arc::new(AtomicU64::new(0)),
            stats: DeviceStats::default(),
        };
        unsafe {
            let open: Symbol<PcmOpen> = sink.sym(b"snd_pcm_open\0")?;
            let err = open(
                &mut pcm,
                name.as_ptr(),
                SND_PCM_STREAM_PLAYBACK,
                SND_PCM_NONBLOCK,
            );
            if err < 0 {
                return Err(sink.alsa_error("opening the device", err));
            }
            sink.pcm = pcm;
            let set: Symbol<PcmSetParams> = sink.sym(b"snd_pcm_set_params\0")?;
            let err = set(
                pcm,
                SND_PCM_FORMAT_FLOAT_LE,
                SND_PCM_ACCESS_RW_INTERLEAVED,
                channels as c_uint,
                rate,
                // Refuse to let ALSA resample behind our back: a rate the device
                // cannot run at is an error, not a silent conversion.
                0,
                latency_us,
            );
            if err < 0 {
                return Err(sink.alsa_error(
                    &format!("configuring {rate} Hz, {channels} channels, float"),
                    err,
                ));
            }
            let get: Symbol<PcmGetParams> = sink.sym(b"snd_pcm_get_params\0")?;
            let (mut buffer, mut period) = (0 as c_ulong, 0 as c_ulong);
            // `c_ulong` is 64-bit here and 32-bit on other targets.
            #[allow(clippy::useless_conversion)]
            if get(pcm, &mut buffer, &mut period) == 0 {
                sink.info.buffer_frames = u64::from(buffer);
                sink.info.period_frames = u64::from(period);
            }
        }
        Ok(sink)
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn stats(&self) -> DeviceStats {
        self.stats
    }

    unsafe fn sym<T>(&self, name: &[u8]) -> Result<Symbol<'_, T>> {
        self.lib.get(name).map_err(|e| {
            Error::Unsupported(format!(
                "libasound lacks {}: {e}",
                String::from_utf8_lossy(&name[..name.len() - 1])
            ))
        })
    }

    fn alsa_error(&self, doing: &str, err: c_int) -> Error {
        let text = unsafe {
            self.sym::<StrError>(b"snd_strerror\0")
                .ok()
                .map(|f| CStr::from_ptr(f(err)).to_string_lossy().into_owned())
        };
        Error::Unsupported(format!(
            "ALSA {doing}: {}",
            text.unwrap_or_else(|| format!("error {err}"))
        ))
    }

    /// Recover from an underrun or a suspend, counting it.
    fn recover(&mut self, err: c_int) -> Result<()> {
        self.stats.xruns += 1;
        unsafe {
            let recover: Symbol<PcmRecover> = self.sym(b"snd_pcm_recover\0")?;
            if recover(self.pcm, err, 1) < 0 {
                return Err(self.alsa_error("recovering", err));
            }
        }
        Ok(())
    }

    /// Frames handed over and not yet played, straight from the device.
    fn delay(&self) -> Option<u64> {
        unsafe {
            let delay: Symbol<PcmDelay> = self.sym(b"snd_pcm_delay\0").ok()?;
            let mut d: c_long = 0;
            (delay(self.pcm, &mut d) == 0).then_some(d.max(0) as u64)
        }
    }

    /// Bring the played counter up to date. Never moves it backwards.
    fn update_played(&self) {
        if let Some(delay) = self.delay() {
            let played = self.written.saturating_sub(delay);
            self.played.fetch_max(played, Ordering::Relaxed);
        }
    }
}

impl AudioSink for AlsaSink {
    fn sample_rate(&self) -> Rate {
        self.rate
    }

    fn channels(&self) -> usize {
        self.info.channels
    }

    fn writable(&self) -> usize {
        self.update_played();
        unsafe {
            match self.sym::<PcmAvail>(b"snd_pcm_avail_update\0") {
                Ok(avail) => avail(self.pcm).max(0) as usize,
                Err(_) => 0,
            }
        }
    }

    fn write(&mut self, interleaved: &[f32]) -> Result<usize> {
        let channels = self.info.channels.max(1);
        let frames = interleaved.len() / channels;
        if frames == 0 {
            return Ok(0);
        }
        for attempt in 0..2 {
            let n = unsafe {
                let writei: Symbol<PcmWritei> = self.sym(b"snd_pcm_writei\0")?;
                writei(
                    self.pcm,
                    interleaved.as_ptr() as *const c_void,
                    frames as c_ulong,
                )
            };
            if n >= 0 {
                self.written += n as u64;
                self.stats.written += n as u64;
                self.update_played();
                return Ok(n as usize);
            }
            let err = n as c_int;
            if err == -EAGAIN {
                self.stats.would_block += 1;
                return Ok(0);
            }
            if attempt == 0 {
                self.recover(err)?;
            } else {
                return Err(self.alsa_error("writing", err));
            }
        }
        Ok(0)
    }

    fn queued(&self) -> u64 {
        self.update_played();
        self.delay().unwrap_or(0)
    }

    fn played_counter(&self) -> PlayedFrames {
        self.played.clone()
    }

    fn flush(&mut self) -> Result<()> {
        unsafe {
            let drop_: Symbol<PcmSimple> = self.sym(b"snd_pcm_drop\0")?;
            let prepare: Symbol<PcmSimple> = self.sym(b"snd_pcm_prepare\0")?;
            drop_(self.pcm);
            let err = prepare(self.pcm);
            if err < 0 {
                return Err(self.alsa_error("preparing after a flush", err));
            }
        }
        // What was queued was discarded, so it will never be played: what has
        // been played is where the count stands.
        self.written = self.played.load(Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for AlsaSink {
    fn drop(&mut self) {
        if self.pcm.is_null() {
            return;
        }
        unsafe {
            if let Ok(close) = self.sym::<PcmSimple>(b"snd_pcm_close\0") {
                close(self.pcm);
            }
        }
    }
}
