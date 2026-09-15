//! A real device, measured. Silence by default — the device consumes it exactly
//! as it consumes sound — and a tone only when asked for with CINE_AUDIO_TONE=1.

use cine_audio_io::AlsaSink;
use cine_media::av::AudioSink;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[test]
fn the_device_plays_at_the_rate_of_the_system_clock_and_reports_its_delay() {
    if let Err(why) = AlsaSink::available() {
        eprintln!("SKIP: {why}");
        return;
    }
    let mut sink = match AlsaSink::open("default", 48_000, 2, 50_000) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SKIP: no playable default device: {e}");
            return;
        }
    };
    let info = sink.info().clone();
    let tone = std::env::var("CINE_AUDIO_TONE").is_ok();
    let played = sink.played_counter();
    let seconds = 3.0;
    let total = (48_000.0 * seconds) as u64;
    let mut offered = 0u64;
    let mut samples: Vec<(Instant, u64)> = Vec::new();
    let mut max_delay = 0u64;
    let began = Instant::now();
    let mut phase = 0.0f32;
    while began.elapsed() < Duration::from_secs_f64(seconds + 1.5) {
        let room = sink.writable().min((total - offered) as usize);
        if room > 0 {
            let mut buf = vec![0.0f32; room * 2];
            if tone {
                for f in buf.chunks_mut(2) {
                    let v = (phase * std::f32::consts::TAU).sin() * 0.1;
                    f[0] = v;
                    f[1] = v;
                    phase = (phase + 440.0 / 48_000.0) % 1.0;
                }
            }
            offered += sink.write(&buf).unwrap() as u64;
        }
        max_delay = max_delay.max(sink.queued());
        samples.push((Instant::now(), played.load(Ordering::Relaxed)));
        if offered >= total && sink.queued() == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let stats = sink.stats();
    // Rate over the steady middle: from 0.5 s to 2.5 s of wall time.
    let at = |t: f64| {
        samples
            .iter()
            .find(|(i, _)| i.duration_since(began).as_secs_f64() >= t)
            .copied()
            .unwrap()
    };
    let (t0, p0) = at(0.5);
    let (t1, p1) = at(2.5);
    let wall = t1.duration_since(t0).as_secs_f64();
    let rate = (p1 - p0) as f64 / wall;
    let ppm = (rate / 48_000.0 - 1.0) * 1e6;
    println!(
        "MEASURED device {:?}: buffer {} frames, period {} frames; wrote {}, played {}; \
         consumption {:.1} frames/s ({:+.0} ppm vs the system clock over {:.2} s); \
         max delay {} frames ({:.1} ms); {} xruns, {} would-block; {}",
        info.name,
        info.buffer_frames,
        info.period_frames,
        stats.written,
        played.load(Ordering::Relaxed),
        rate,
        ppm,
        wall,
        max_delay,
        max_delay as f64 / 48.0,
        stats.xruns,
        stats.would_block,
        if tone { "tone" } else { "silence" }
    );
    assert_eq!(stats.written, total, "every frame was accepted");
    // Scheduling jitter of the 5 ms polling loop bounds how exactly this can be
    // read; a device running at a different rate would be far outside it.
    assert!(ppm.abs() < 5_000.0, "consumption rate {rate} is not 48 kHz");
    assert!(
        played.load(Ordering::Relaxed) >= total * 95 / 100,
        "the device played what it was given"
    );
}

/// Silence, as a source: a real stream of samples with nothing audible in it.
struct Silence {
    frames: i64,
}

impl cine_media::av::AudioSource for Silence {
    fn sample_rate(&self) -> ucg_core::Rate {
        ucg_core::Rate::new(48_000, 1).unwrap()
    }
    fn channels(&self) -> usize {
        2
    }
    fn read(&mut self, from: i64, frames: usize, out: &mut Vec<f32>) -> cine_media::Result<usize> {
        let n = frames.min((self.frames - from).max(0) as usize);
        out.extend(std::iter::repeat_n(0.0, n * 2));
        Ok(n)
    }
}

/// The picture clock, running on what the device has played: after two seconds
/// of wall time the playhead is two seconds minus what is still in the device,
/// and it moves at the device's rate.
#[test]
fn a_playback_clock_paced_by_the_device_follows_the_sound_not_the_queue() {
    use cine_media::av::AudioScheduler;
    use cine_media::clock::PlaybackClock;
    use ucg_core::{Rate, RationalTime};
    if AlsaSink::available().is_err() {
        eprintln!("SKIP: no ALSA");
        return;
    }
    let Ok(sink) = AlsaSink::open("default", 48_000, 2, 50_000) else {
        eprintln!("SKIP: no playable default device");
        return;
    };
    let start = RationalTime::zero(Rate::new(48_000, 1).unwrap());
    let mut audio = AudioScheduler::new(sink, Silence { frames: 48_000 * 4 }, start).unwrap();
    let end = RationalTime::exact(4 * 24, Rate::new(24, 1).unwrap());
    let mut clock = PlaybackClock::new(
        audio.time_source(),
        RationalTime::zero(Rate::new(24, 1).unwrap()),
        end,
    );
    clock.play().unwrap();
    let began = Instant::now();
    let mut lag_ms = Vec::new();
    while began.elapsed() < Duration::from_secs(3) {
        audio.pump().unwrap();
        let wall = began.elapsed().as_secs_f64();
        let position = clock.position().to_seconds();
        if wall > 0.5 {
            lag_ms.push((wall - position) * 1e3);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let queued_ms = audio.sink().queued() as f64 / 48.0;
    lag_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = lag_ms[lag_ms.len() / 2];
    let spread = lag_ms[lag_ms.len() * 95 / 100] - lag_ms[lag_ms.len() * 5 / 100];
    println!(
        "MEASURED picture clock on the device: playhead trails wall time by median {median:.1} ms \
         (5–95% spread {spread:.1} ms); device queue now {queued_ms:.1} ms; {:?}",
        audio.stats()
    );
    // The playhead trails the wall clock by the audio still in the device, and
    // it does not run away from it: the lag is bounded, not growing.
    assert!(median > 0.0, "the playhead must not lead the sound");
    assert!(
        median < 250.0,
        "the playhead is not paced by the device ({median} ms behind)"
    );
    assert!(
        spread < 60.0,
        "the lag drifts instead of holding ({spread} ms)"
    );
}
