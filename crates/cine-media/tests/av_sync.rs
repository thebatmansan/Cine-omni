//! Audio/video synchronisation, driven sample by sample.
//!
//! No device, no sleeping, no timing assertions. The sink is a model of one: the
//! test decides exactly how many frames it accepts and exactly how many the
//! "device" has played, which is what makes statements about synchronisation
//! provable instead of plausible.
//!
//! The central test is `a_video_scheduler_paced_by_the_audio_clock_is_exact`:
//! 23.976 fps picture against a 48 kHz audio clock, two rates with no common
//! frame boundary, checked at every picture across ten seconds.

use cine_media::av::{
    frames_to_duration, AudioScheduler, AudioSink, AudioSource, AudioTimeSource, AvStatus,
    PlayedFrames,
};
use cine_media::clock::{PlaybackClock, PlaybackRate, TimeSource, TransportState};
use cine_media::frame::{AlphaMode, ColorMetadata, PixelFormat, VideoFrame};
use cine_media::schedule::{FrameSource, Presentation, Scheduler};
use cine_media::time::{Rate, Timestamp};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// A sink the test drives by hand
// ---------------------------------------------------------------------------

/// Models a device with a fixed buffer: audio is written into a queue, and the
/// test decides when the "device" consumes it.
struct TestSink {
    rate: Rate,
    channels: usize,
    capacity: usize,
    queued: u64,
    played: PlayedFrames,
    /// Accept at most this many frames per write, to model a short write.
    accept_limit: Option<usize>,
    written: Vec<f32>,
    flushes: u64,
}

impl TestSink {
    fn new(rate: Rate, channels: usize, capacity: usize) -> TestSink {
        TestSink {
            rate,
            channels,
            capacity,
            queued: 0,
            played: Arc::new(AtomicU64::new(0)),
            accept_limit: None,
            written: Vec::new(),
            flushes: 0,
        }
    }
    /// The device plays `frames` of what it holds.
    fn play(&mut self, frames: u64) {
        let actually = frames.min(self.queued);
        self.queued -= actually;
        self.played.fetch_add(actually, Ordering::Relaxed);
    }
}

impl AudioSink for TestSink {
    fn sample_rate(&self) -> Rate {
        self.rate
    }
    fn channels(&self) -> usize {
        self.channels
    }
    fn writable(&self) -> usize {
        self.capacity - self.queued as usize
    }
    fn write(&mut self, interleaved: &[f32]) -> cine_media::Result<usize> {
        let offered = interleaved.len() / self.channels;
        let room = self.writable().min(self.accept_limit.unwrap_or(usize::MAX));
        let taken = offered.min(room);
        self.written
            .extend_from_slice(&interleaved[..taken * self.channels]);
        self.queued += taken as u64;
        Ok(taken)
    }
    fn queued(&self) -> u64 {
        self.queued
    }
    fn played_counter(&self) -> PlayedFrames {
        self.played.clone()
    }
    fn flush(&mut self) -> cine_media::Result<()> {
        self.queued = 0;
        self.flushes += 1;
        Ok(())
    }
}

/// Audio whose every sample encodes the absolute sample index it came from, so
/// a test can prove *which* audio was played, not merely how much.
struct CountingAudio {
    rate: Rate,
    channels: usize,
    length: i64,
}

impl AudioSource for CountingAudio {
    fn sample_rate(&self) -> Rate {
        self.rate
    }
    fn channels(&self) -> usize {
        self.channels
    }
    fn read(&mut self, from: i64, frames: usize, out: &mut Vec<f32>) -> cine_media::Result<usize> {
        if from >= self.length {
            return Ok(0);
        }
        let n = frames.min((self.length - from) as usize);
        for i in 0..n {
            for _ in 0..self.channels {
                out.push((from + i as i64) as f32);
            }
        }
        Ok(n)
    }
}

/// Pictures on a grid, for the scheduler to select from.
struct Pictures {
    pts: Vec<Timestamp>,
}

impl Pictures {
    fn grid(count: usize, rate: Rate) -> Pictures {
        Pictures {
            pts: (0..count)
                .map(|i| Timestamp::exact(i as i64, rate))
                .collect(),
        }
    }
}

impl FrameSource for Pictures {
    fn frame_count(&self) -> usize {
        self.pts.len()
    }
    fn pts_of(&self, index: usize) -> Option<Timestamp> {
        self.pts.get(index).copied()
    }
    fn index_at(&self, t: Timestamp) -> Option<usize> {
        self.pts.partition_point(|p| *p <= t).checked_sub(1)
    }
    fn extent(&self) -> Option<(Timestamp, Timestamp)> {
        let first = *self.pts.first()?;
        let last = *self.pts.last()?;
        Some((
            first,
            last.checked_add(&Timestamp::exact(1, last.rate_exact())),
        ))
    }
    fn decode_index(&mut self, index: usize) -> cine_media::Result<Option<VideoFrame>> {
        Ok(self.pts.get(index).map(|pts| VideoFrame {
            pts: *pts,
            frame_index: Some(index as i64),
            timecode: None,
            width: 1,
            height: 1,
            format: PixelFormat::Yuv420P8,
            color: ColorMetadata::default(),
            alpha: AlphaMode::None,
            planes: vec![(Arc::new(vec![0u8; 1]), 1)],
        }))
    }
}

fn rig(capacity: usize) -> AudioScheduler<TestSink, CountingAudio> {
    AudioScheduler::new(
        TestSink::new(Rate::AUDIO_48K, 2, capacity),
        CountingAudio {
            rate: Rate::AUDIO_48K,
            channels: 2,
            length: 48_000 * 60,
        },
        Timestamp::exact(0, Rate::AUDIO_48K),
    )
    .expect("matching rates")
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// Elapsed time is exactly the frames played, at 48 kHz and at 44.1 kHz.
#[test]
fn the_audio_clock_is_exactly_the_frames_that_have_been_played() {
    for (rate, per_second) in [(Rate::AUDIO_48K, 48_000u64), (Rate::AUDIO_44100, 44_100)] {
        let played: PlayedFrames = Arc::new(AtomicU64::new(0));
        let clock = AudioTimeSource::new(played.clone(), rate);
        assert_eq!(clock.elapsed(), std::time::Duration::ZERO);

        played.store(per_second, Ordering::Relaxed);
        assert_eq!(
            clock.elapsed(),
            std::time::Duration::from_secs(1),
            "a second of frames is a second, at {rate:?}"
        );
        played.store(per_second * 3600, Ordering::Relaxed);
        assert_eq!(clock.elapsed(), std::time::Duration::from_secs(3600));
    }
}

/// Rounding to whole nanoseconds must not accumulate: the clock is computed
/// from the absolute frame count, never advanced by increments.
#[test]
fn the_audio_clock_does_not_accumulate_rounding_over_hours() {
    let rate = Rate::AUDIO_44100; // 1/44100 s is not a whole number of nanoseconds
    let played: PlayedFrames = Arc::new(AtomicU64::new(0));
    let clock = AudioTimeSource::new(played.clone(), rate);
    // Four hours, one frame at a time in the model, read at the end.
    let frames = 44_100u64 * 3600 * 4 + 1;
    played.store(frames, Ordering::Relaxed);
    let exact_nanos = u128::from(frames) * 1_000_000_000 / 44_100;
    let got = clock.elapsed().as_nanos();
    assert!(
        exact_nanos - got < 1,
        "after four hours the clock is {} ns from exact",
        exact_nanos - got
    );
}

/// The playhead must follow the sound, not the writes. A device holding half a
/// second of audio would otherwise put every picture half a second early.
#[test]
fn the_position_is_what_is_audible_not_what_was_written() {
    let mut s = rig(24_000); // half a second of buffer
    s.pump().unwrap();
    assert_eq!(s.stats().frames_written, 24_000);
    assert_eq!(
        s.position(),
        Timestamp::exact(0, Rate::AUDIO_48K),
        "nothing has been heard yet, so the playhead is still at zero"
    );
    assert_eq!(s.device_offset(), std::time::Duration::from_millis(500));
}

// ---------------------------------------------------------------------------
// Picture paced by sound
// ---------------------------------------------------------------------------

/// The one that matters. A 23.976 fps picture paced by a 48 kHz audio clock:
/// two rates with no common frame boundary. For every picture in ten seconds,
/// the scheduler must select exactly that picture once the audio has played the
/// samples that correspond to it — and not one sample earlier.
#[test]
fn a_video_scheduler_paced_by_the_audio_clock_is_exact() {
    let video = Rate::F23_976;
    let audio = Rate::AUDIO_48K;
    let mut sink = TestSink::new(audio, 2, 48_000);
    let played = sink.played_counter();
    let pictures = Pictures::grid(240, video);
    let (start, end) = pictures.extent().unwrap();
    let clock = PlaybackClock::new(AudioTimeSource::new(played.clone(), audio), start, end);
    let mut s = Scheduler::new(clock, pictures);
    s.clock_mut().play().unwrap();
    sink.play(0);

    let mut shown = Vec::new();
    for picture in 0..240i64 {
        // Exact sample index at which this picture begins:
        //   picture * (1001/24000) s * 48000 samples/s = picture * 2002
        let at = picture * 2002;
        if at > 0 {
            // One sample before the boundary the previous picture is still due.
            played.store((at - 1) as u64, Ordering::Relaxed);
            match s.poll() {
                Presentation::Present(p) => panic!(
                    "picture {} was presented one sample early (at {})",
                    p.index,
                    at - 1
                ),
                Presentation::Hold => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        played.store(at as u64, Ordering::Relaxed);
        match s.poll() {
            Presentation::Present(p) => {
                assert_eq!(p.index as i64, picture, "wrong picture at sample {at}");
                assert_eq!(p.dropped, 0);
                shown.push(p.index);
            }
            other => panic!("picture {picture} was not presented at sample {at}: {other:?}"),
        }
    }
    assert_eq!(shown.len(), 240);
    assert_eq!(s.stats().dropped, 0);
    assert_eq!(s.state(), TransportState::Playing);
}

/// Audio stopping stops the picture. Nothing has to be told: no frames played
/// means no time passed.
#[test]
fn a_silent_device_freezes_the_playhead_rather_than_running_on() {
    let video = Rate::F24;
    let audio = Rate::AUDIO_48K;
    let sink = TestSink::new(audio, 2, 48_000);
    let played = sink.played_counter();
    let pictures = Pictures::grid(240, video);
    let (start, end) = pictures.extent().unwrap();
    let clock = PlaybackClock::new(AudioTimeSource::new(played.clone(), audio), start, end);
    let mut s = Scheduler::new(clock, pictures);
    s.clock_mut().play().unwrap();

    played.store(2000, Ordering::Relaxed);
    let before = s.position();
    s.poll();
    // The device stalls: no further frames are played.
    for _ in 0..10 {
        assert_eq!(
            s.position(),
            before,
            "the playhead moved while nothing played"
        );
        assert!(matches!(s.poll(), Presentation::Hold));
    }
    assert_eq!(s.stats().dropped, 0, "a stopped clock drops nothing");
}

// ---------------------------------------------------------------------------
// Feeding, seeking, starving
// ---------------------------------------------------------------------------

/// Audio is pulled by absolute sample index, so what is written is exactly the
/// audio for the position asked for.
#[test]
fn the_samples_written_are_the_samples_for_that_position() {
    let mut s = rig(1_000);
    s.seek(Timestamp::exact(5_000, Rate::AUDIO_48K)).unwrap();
    s.pump().unwrap();
    let written = &s.sink().written;
    assert_eq!(written.len(), 1_000 * 2);
    assert_eq!(written[0], 5_000.0, "first sample is the one at the seek");
    assert_eq!(written[2], 5_001.0, "and the next frame follows it");
    assert_eq!(written[written.len() - 2], 5_999.0);
}

/// A seek discards what the device still holds. Continuing to play the old
/// position would be worse than the gap.
#[test]
fn seeking_discards_what_the_device_still_holds() {
    let mut s = rig(10_000);
    s.pump().unwrap();
    assert_eq!(s.sink().queued(), 10_000);
    s.seek(Timestamp::exact(48_000, Rate::AUDIO_48K)).unwrap();
    assert_eq!(s.sink().queued(), 0, "the old audio must not still play");
    assert_eq!(s.sink().flushes, 1);
    assert_eq!(s.stats().seeks, 1);
    s.pump().unwrap();
    assert_eq!(s.sink().written[20_000], 48_000.0);
}

/// Refilling only what the device has consumed, repeatedly, keeps the queue
/// full and the cursor exact.
#[test]
fn repeated_pumping_keeps_the_device_fed_without_gaps() {
    let mut s = rig(4_800);
    for _ in 0..20 {
        s.pump().unwrap();
        assert_eq!(s.sink().queued(), 4_800);
        // The device plays a tenth of a second.
        s.sink_mut().play(4_800);
    }
    assert_eq!(s.stats().frames_written, 4_800 * 20);
    assert_eq!(*s.status(), AvStatus::Locked);
    assert_eq!(s.stats().underruns, 0);
}

/// A source that runs out reports the end once the device has drained, not when
/// the last sample is handed over — the sound is still playing until then.
#[test]
fn the_end_of_the_audio_is_reported_when_it_has_been_heard() {
    let mut s = AudioScheduler::new(
        TestSink::new(Rate::AUDIO_48K, 2, 4_800),
        CountingAudio {
            rate: Rate::AUDIO_48K,
            channels: 2,
            length: 2_400,
        },
        Timestamp::exact(0, Rate::AUDIO_48K),
    )
    .unwrap();
    s.pump().unwrap();
    assert_eq!(s.stats().frames_written, 2_400);
    s.pump().unwrap();
    assert_eq!(
        *s.status(),
        AvStatus::Locked,
        "still draining: the end is not audible yet"
    );
    s.sink_mut().play(2_400);
    s.pump().unwrap();
    assert_eq!(*s.status(), AvStatus::EndOfStream);
}

/// A sink that accepts less than it offered is a starved device, reported.
#[test]
fn a_short_write_is_reported_as_starvation_and_the_cursor_follows_it() {
    let mut s = rig(10_000);
    s.sink_mut().accept_limit = Some(100);
    s.pump().unwrap();
    assert_eq!(s.stats().frames_written, 100);
    assert_eq!(*s.status(), AvStatus::Starved);
    // The cursor must follow what was accepted, never what was offered.
    s.sink_mut().accept_limit = None;
    s.pump().unwrap();
    assert_eq!(s.sink().written[200], 100.0, "audio must not skip a gap");
    assert_eq!(*s.status(), AvStatus::Locked, "it recovers when fed again");
}

// ---------------------------------------------------------------------------
// What is refused
// ---------------------------------------------------------------------------

/// Mismatched rates are refused, not resampled silently.
#[test]
fn a_source_and_sink_at_different_rates_are_refused() {
    let err = AudioScheduler::new(
        TestSink::new(Rate::AUDIO_48K, 2, 1_000),
        CountingAudio {
            rate: Rate::AUDIO_44100,
            channels: 2,
            length: 1_000,
        },
        Timestamp::exact(0, Rate::AUDIO_44100),
    )
    .err()
    .expect("mismatched rates must be refused");
    let text = err.to_string();
    assert!(text.contains("resampler"), "{text}");
}

#[test]
fn a_channel_count_mismatch_is_refused() {
    let err = AudioScheduler::new(
        TestSink::new(Rate::AUDIO_48K, 2, 1_000),
        CountingAudio {
            rate: Rate::AUDIO_48K,
            channels: 6,
            length: 1_000,
        },
        Timestamp::exact(0, Rate::AUDIO_48K),
    )
    .err()
    .expect("mismatched channels must be refused");
    assert!(err.to_string().contains("channel mapper"), "{err}");
}

/// Any speed other than 1 mutes, and says why. Silence that announces itself is
/// honest; pitch-shifted audio pretending to be playback is not.
#[test]
fn speeds_other_than_one_mute_and_explain_themselves() {
    let mut s = rig(4_800);
    s.set_rate(PlaybackRate::new(1, 2).unwrap());
    match s.status() {
        AvStatus::Muted(why) => assert!(why.contains("time-stretch"), "{why}"),
        other => panic!("expected muted, got {other:?}"),
    }
    assert_eq!(s.pump().unwrap(), 0, "muted means nothing is written");

    s.set_rate(PlaybackRate::new(-1, 1).unwrap());
    match s.status() {
        AvStatus::Muted(why) => assert!(why.contains("reverse"), "{why}"),
        other => panic!("expected muted, got {other:?}"),
    }

    s.set_rate(PlaybackRate::FORWARD);
    assert_eq!(*s.status(), AvStatus::Locked);
    assert!(s.pump().unwrap() > 0);
}

/// Exactness of the frame-to-time conversion itself, at the rates that matter.
#[test]
fn frame_counts_convert_to_time_exactly() {
    assert_eq!(
        frames_to_duration(48_000, Rate::AUDIO_48K),
        std::time::Duration::from_secs(1)
    );
    assert_eq!(
        frames_to_duration(24_000, Rate::AUDIO_48K),
        std::time::Duration::from_millis(500)
    );
    assert_eq!(
        frames_to_duration(1, Rate::AUDIO_48K),
        std::time::Duration::from_nanos(20_833),
        "1/48000 s is 20833.33 ns; it floors rather than rounding up"
    );
    assert_eq!(
        frames_to_duration(96_000, Rate::AUDIO_96K),
        std::time::Duration::from_secs(1)
    );
}
