//! Measured mixer cost, reported as numbers rather than adjectives.
//!
//! Deliberately harness-free and dependency-free, in the house style of
//! `cine-media/benches/decode.rs`: this is a measurement tool, not a
//! statistical benchmark suite. Every figure it prints is a wall-clock
//! measurement of work that actually happened.
//!
//! Run with:
//! ```text
//! cargo bench -p cine-audio --bench mix
//! ```
//!
//! The question it exists to answer: can `mix::render` deliver one block of a
//! realistic session inside that block's own duration? At 48 kHz a 512-frame
//! block lasts 10.667 ms, and anything at or above 100 % of that budget cannot
//! feed a real-time device, no matter how fast the machine.
//!
//! There is no synthetic-input caveat here as there is for media decode: a
//! mixing session *is* the input, and a session built in code is exactly as
//! real as one loaded from disk. What is synthetic is the audio in the clips,
//! and the mixer's cost does not depend on what the samples contain.

use cine_audio::mix::{pan_matrix, Renderer, BLOCK_SIZE};
use cine_audio::prelude::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SR: u32 = 48_000;
const RATE: f64 = 24.0;

/// Run `f` `n` times and report the best and mean wall-clock time.
///
/// Best-of is reported alongside the mean because the mean on a loaded
/// workstation measures the scheduler as much as the code.
fn time_it(n: u32, mut f: impl FnMut()) -> (Duration, Duration) {
    let mut best = Duration::MAX;
    let start = Instant::now();
    for _ in 0..n {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed());
    }
    (best, start.elapsed() / n)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// The wall-clock duration of one `BLOCK_SIZE` block: the real-time deadline.
fn block_deadline() -> f64 {
    BLOCK_SIZE as f64 / SR as f64 * 1000.0
}

// ---------------------------------------------------------------------------
// Session fixtures
// ---------------------------------------------------------------------------

/// How much of a real session to build. Every flag corresponds to one class of
/// per-block work, so switching them off one at a time attributes the cost.
#[derive(Clone, Copy)]
struct Spec {
    tracks: usize,
    /// Clips laid end to end on each track.
    clips_per_track: usize,
    /// Source channel count of the clips.
    src_channels: usize,
    inserts: bool,
    automation: bool,
    sends: bool,
    fades: bool,
    master_limiter: bool,
    stem_buses: bool,
}

impl Spec {
    fn full(tracks: usize) -> Spec {
        Spec {
            tracks,
            clips_per_track: 8,
            src_channels: 1,
            inserts: true,
            automation: true,
            sends: true,
            fades: true,
            master_limiter: true,
            stem_buses: true,
        }
    }
}

/// A source buffer long enough to cover the whole fixture timeline.
fn source(channels: usize, seconds: f64, freq: f64) -> Arc<AudioBuffer> {
    Arc::new(AudioBuffer::sine(SR, channels, freq, 0.25, seconds + 1.0))
}

/// Build a session to `spec`. Clips are laid end to end so that any render
/// window lands inside a clip — the realistic case, and the expensive one.
fn build(spec: Spec) -> Session {
    let mut s = if spec.stem_buses {
        Session::new(SR, RATE)
    } else {
        Session::empty(SR, RATE)
    };
    if !spec.master_limiter {
        s.master.inserts.clear();
    }
    let stems = [Stem::Dialogue, Stem::Music, Stem::Effects];
    // One reverb bus, so the sends have somewhere to land.
    let reverb = Bus::new("reverb");
    let reverb_id = reverb.id;
    s.buses.push(reverb);

    // The timeline span is fixed at SPAN seconds and subdivided into
    // `clips_per_track` clips, so varying the clip count varies *only* the clip
    // count — the render window stays covered either way. (Extending the span
    // instead would move the window off the end of the timeline and measure an
    // empty track, which is not the same question at all.)
    const SPAN: f64 = 32.0;
    let clip_len = SPAN / spec.clips_per_track as f64;
    let buf = source(spec.src_channels, SPAN, 220.0);

    for i in 0..spec.tracks {
        let stem = stems[i % stems.len()].clone();
        let mut t = Track::new(format!("t{i}"), s.master_id()).with_stem(stem);
        for c in 0..spec.clips_per_track {
            let mut clip =
                Clip::from_buffer(format!("c{c}"), Arc::clone(&buf), c as f64 * clip_len, RATE);
            clip.range = ucg_core::TimeRange::new(
                ucg_core::RationalTime::from_seconds(c as f64 * clip_len, RATE),
                ucg_core::RationalTime::from_seconds(clip_len, RATE),
            );
            clip.gain_db = -3.0;
            if spec.fades {
                let f = (clip_len * 0.02).min(0.05);
                clip.fade_in = Fade::new(f, FadeCurve::EqualPower);
                clip.fade_out = Fade::new(f, FadeCurve::EqualPower);
            }
            t = t.with_clip(clip);
        }
        // A pan position that is not dead centre, so the pan matrix is real work.
        t.pan.azimuth_deg = -40.0 + 80.0 * (i as f32 / spec.tracks.max(2) as f32);
        t.volume_db = -6.0;
        if spec.inserts {
            // The everyday channel strip: a high-pass, a bell, a compressor.
            t.inserts.push(Insert::Biquad {
                kind: BiquadKind::HighPass,
                freq_hz: 80.0,
                q: 0.707,
                gain_db: 0.0,
            });
            t.inserts.push(Insert::Biquad {
                kind: BiquadKind::Peaking,
                freq_hz: 2_500.0,
                q: 1.2,
                gain_db: 2.5,
            });
            t.inserts
                .push(Insert::Compressor(CompressorParams::default()));
        }
        if spec.sends {
            t.sends.push(Send {
                bus: reverb_id,
                level_db: -12.0,
                pre_fader: false,
            });
        }
        if spec.automation {
            t.automation.set_lane(
                Param::Volume,
                Lane::new(
                    vec![(0.0, -6.0), (5.0, -3.0), (10.0, -9.0), (30.0, -6.0)],
                    Interpolation::Linear,
                ),
            );
            t.automation.set_lane(
                Param::Pan,
                Lane::new(
                    vec![(0.0, -30.0), (7.0, 30.0), (20.0, -10.0)],
                    Interpolation::Linear,
                ),
            );
        }
        s.add_track(t);
    }
    s
}

// ---------------------------------------------------------------------------

fn main() {
    println!("cine-audio mixer cost");
    println!("sample rate {SR} Hz, block {BLOCK_SIZE} frames");
    println!(
        "real-time deadline for one block: {:.3} ms\n",
        block_deadline()
    );

    bench_streaming(); // the headline: can a prepared renderer hold the deadline?
    bench_block(); // the same work through the one-shot offline entry point
    bench_breakdown(); // where the time goes
    bench_fixed_cost(); // per-call setup versus per-sample work
    bench_offline(); // long-form render as a realtime factor
    bench_components(); // the public pieces, measured directly
    bench_cost_ratio(); // the ratio tests/mix_cost.rs guards
}

/// The ratio `tests/mix_cost.rs` asserts on: one block against a gain-and-sum
/// pass over the same samples. Printed here so the bound in the test is set
/// from a measurement rather than a guess.
fn bench_cost_ratio() {
    println!("== block cost as a ratio (what tests/mix_cost.rs guards) ==");
    let s = build(Spec::full(48));
    let mut r = Renderer::new(&s, ChannelLayout::Stereo).unwrap();
    r.prepare((0.0, 600.0)).unwrap();
    let mut block = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..64 {
        r.render_block(&mut block, BLOCK_SIZE).unwrap();
    }
    let inputs: Vec<Vec<f32>> = (0..48)
        .map(|i| {
            (0..BLOCK_SIZE * 2)
                .map(|k| ((k + i) as f32 * 0.001).sin())
                .collect()
        })
        .collect();
    let mut acc = vec![0.0f32; BLOCK_SIZE * 2];
    let sum = |acc: &mut Vec<f32>| {
        acc.fill(0.0);
        for (i, t) in inputs.iter().enumerate() {
            let g = 0.5 + i as f32 * 1e-4;
            for (a, x) in acc.iter_mut().zip(t) {
                *a += x * g;
            }
        }
    };
    let mut mix_best = Duration::MAX;
    let mut ref_best = Duration::MAX;
    for _ in 0..300 {
        let t = Instant::now();
        r.render_block(&mut block, BLOCK_SIZE).unwrap();
        mix_best = mix_best.min(t.elapsed());
        std::hint::black_box(&block);
        let t = Instant::now();
        sum(&mut acc);
        ref_best = ref_best.min(t.elapsed());
        std::hint::black_box(&acc);
    }
    println!(
        "48-track block {:8.4} ms   gain-and-sum reference {:8.4} ms   ratio {:6.1}x",
        ms(mix_best),
        ms(ref_best),
        mix_best.as_secs_f64() / ref_best.as_secs_f64()
    );
    println!();
}

/// Distribution of `n` samples of a duration. A real-time deadline is a
/// statement about the *worst* block, not the best one, so the best-of figure
/// that is right for throughput work is the wrong one here — it is reported
/// alongside as the floor the code could reach on an idle machine.
fn spread(mut v: Vec<Duration>) -> (Duration, Duration, Duration, Duration) {
    v.sort();
    let n = v.len();
    (v[0], v[n / 2], v[(n * 99) / 100], v[n - 1])
}

/// The number that decides whether playback is possible: one block from a
/// renderer that has already been prepared — which is how an audio callback
/// asks for one, and the only form of the question a real-time answer exists
/// for. `bench_block` below asks the offline entry point for the same audio,
/// which pays for compiling the session on every block.
fn bench_streaming() {
    println!("== one {BLOCK_SIZE}-frame block from a prepared renderer ==");
    let deadline = block_deadline();
    let cases = [
        (16usize, ChannelLayout::Stereo, "16 tracks  stereo"),
        (48, ChannelLayout::Stereo, "48 tracks  stereo"),
        (48, ChannelLayout::Surround51, "48 tracks  5.1   "),
        (128, ChannelLayout::Stereo, "128 tracks stereo"),
        (128, ChannelLayout::Atmos714, "128 tracks 7.1.4 "),
    ];
    for (tracks, layout, label) in cases {
        let s = build(Spec::full(tracks));
        for parallel in [false, true] {
            let mut r = Renderer::new(&s, layout).unwrap();
            r.set_parallel(parallel);
            r.prepare((0.0, 600.0)).unwrap();
            let mut block = vec![0.0f32; BLOCK_SIZE * layout.channel_count()];
            for _ in 0..64 {
                r.render_block(&mut block, BLOCK_SIZE).unwrap();
            }
            let mut times = Vec::with_capacity(400);
            for _ in 0..400 {
                let t = Instant::now();
                r.render_block(&mut block, BLOCK_SIZE).unwrap();
                times.push(t.elapsed());
                std::hint::black_box(&block);
            }
            let (best, med, p99, max) = spread(times);
            println!(
                "{label} {:9}  best {:7.3}  median {:7.3}  p99 {:7.3}  max {:7.3} ms   \
median {:6.1} % of budget",
                if parallel { "threaded" } else { "1 thread" },
                ms(best),
                ms(med),
                ms(p99),
                ms(max),
                ms(med) / deadline * 100.0
            );
        }
    }
    println!("-> a deadline is held when the *worst* block fits, not the best;");
    println!("   waking a thread pool per block puts the operating system on that path\n");
}

/// The same session through the one-shot offline entry point, which compiles
/// the session for every call. Kept because it is the number the engine was
/// judged on, and because the gap between it and the figures above is exactly
/// the cost of compiling a session.
fn bench_block() {
    println!("== one {BLOCK_SIZE}-frame block through mix::render (compiles each call) ==");
    let deadline = block_deadline();
    let window = (10.0, 10.0 + BLOCK_SIZE as f64 / SR as f64);
    let cases = [
        (16usize, ChannelLayout::Stereo, "16 tracks  stereo"),
        (48, ChannelLayout::Stereo, "48 tracks  stereo"),
        (48, ChannelLayout::Surround51, "48 tracks  5.1   "),
        (128, ChannelLayout::Stereo, "128 tracks stereo"),
        (128, ChannelLayout::Atmos714, "128 tracks 7.1.4 "),
    ];
    for (tracks, layout, label) in cases {
        let s = build(Spec::full(tracks));
        // Warm any lazily built state before measuring.
        render(&s, window, layout).unwrap();
        let (best, mean) = time_it(30, || {
            std::hint::black_box(render(&s, window, layout).unwrap());
        });
        println!(
            "{label}  best {:8.3} ms ({:6.1} % of budget)   mean {:8.3} ms ({:6.1} %)",
            ms(best),
            ms(best) / deadline * 100.0,
            ms(mean),
            ms(mean) / deadline * 100.0
        );
    }
    println!();
}

/// Where the time goes, by ablation.
///
/// Measured on **one track over one second** rather than on 48 tracks over one
/// block, deliberately. A single track uses one rayon task, so the figure is
/// not the thread pool's; a second of audio is 94 blocks, so per-call setup
/// cannot hide inside it. What comes out is the per-frame cost of one track's
/// signal path, which is the quantity that decides whether N tracks fit in a
/// block — and it survives a loaded machine, which a 13 ms difference-of-
/// differences does not.
///
/// Attribution by subtraction measures the cost of a feature *in situ*, which
/// is the number that matters, rather than the cost of a function in isolation.
fn bench_breakdown() {
    println!("== breakdown: one track, one second of audio, stereo out ==");
    let seconds = 1.0;
    let frames = seconds * SR as f64;
    let layout = ChannelLayout::Stereo;
    let full = Spec {
        tracks: 1,
        ..Spec::full(1)
    };
    let measure = |spec: Spec| -> f64 {
        let s = build(spec);
        render(&s, (10.0, 10.0 + seconds), layout).unwrap();
        let (best, _) = time_it(7, || {
            std::hint::black_box(render(&s, (10.0, 10.0 + seconds), layout).unwrap());
        });
        best.as_secs_f64() * 1e9 / frames
    };
    let base = measure(full);
    println!("full track signal path      {base:8.1} ns/frame");
    let variants: [(&str, Spec); 6] = [
        (
            "without track inserts   ",
            Spec {
                inserts: false,
                ..full
            },
        ),
        (
            "without automation      ",
            Spec {
                automation: false,
                ..full
            },
        ),
        (
            "without sends           ",
            Spec {
                sends: false,
                ..full
            },
        ),
        (
            "without clip fades      ",
            Spec {
                fades: false,
                ..full
            },
        ),
        (
            "without master limiter  ",
            Spec {
                master_limiter: false,
                ..full
            },
        ),
        (
            "1 clip on the track     ",
            Spec {
                clips_per_track: 1,
                ..full
            },
        ),
    ];
    for (label, spec) in variants {
        let t = measure(spec);
        println!(
            "{label}    {t:8.1} ns/frame  → that feature costs {:7.1} ns/frame ({:5.1} %)",
            base - t,
            (base - t) / base * 100.0
        );
    }
    let bare = measure(Spec {
        inserts: false,
        automation: false,
        sends: false,
        fades: false,
        master_limiter: false,
        ..full
    });
    println!("everything off              {bare:8.1} ns/frame  <- irreducible gather + pan + sum");
    println!(
        "-> at {base:.0} ns/frame/track a {BLOCK_SIZE}-frame block holds {:.0} track-slots \
in {:.3} ms of budget on one core",
        block_deadline() * 1e6 / (base * BLOCK_SIZE as f64),
        block_deadline()
    );
    println!();
}

/// Per-call setup cost versus per-sample work. If rendering 94 blocks costs
/// barely more than rendering one, the mixer is paying a fixed price per call
/// that has nothing to do with the audio — and that price is exactly what a
/// real-time engine cannot afford, because it pays it every block.
fn bench_fixed_cost() {
    println!("== fixed per-call cost versus per-sample work (48 tracks, stereo) ==");
    let s = build(Spec::full(48));
    let layout = ChannelLayout::Stereo;
    for blocks in [1usize, 4, 16, 94] {
        let frames = blocks * BLOCK_SIZE;
        let window = (10.0, 10.0 + frames as f64 / SR as f64);
        render(&s, window, layout).unwrap();
        let reps = if blocks > 16 { 10 } else { 30 };
        let (best, _) = time_it(reps, || {
            std::hint::black_box(render(&s, window, layout).unwrap());
        });
        println!(
            "{blocks:3} block(s) ({frames:6} frames)  best {:8.3} ms   {:8.4} ms/block",
            ms(best),
            ms(best) / blocks as f64
        );
    }
    println!("→ a flat ms/block curve means per-sample work dominates;");
    println!("  a falling one means fixed per-call setup dominates\n");
}

/// Long-form render, the offline delivery case, as a realtime factor.
fn bench_offline() {
    println!("== offline render (2 s of programme) ==");
    let seconds = 2.0;
    for (tracks, layout, label) in [
        (48usize, ChannelLayout::Stereo, "48 tracks  stereo"),
        (128, ChannelLayout::Atmos714, "128 tracks 7.1.4 "),
    ] {
        let s = build(Spec::full(tracks));
        let (best, mean) = time_it(3, || {
            std::hint::black_box(render(&s, (0.0, seconds), layout).unwrap());
        });
        println!(
            "{label} render()   best {:8.1} ms ({:5.2}x realtime)   mean {:8.1} ms ({:5.2}x)",
            ms(best),
            seconds / best.as_secs_f64(),
            ms(mean),
            seconds / mean.as_secs_f64()
        );
        // The same audio through the same block loop on one thread, to separate
        // what the thread pool costs from what block-major order costs.
        let blocks = (seconds * SR as f64 / BLOCK_SIZE as f64).ceil() as usize;
        let mut r = Renderer::new(&s, layout).unwrap();
        let mut block = vec![0.0f32; BLOCK_SIZE * layout.channel_count()];
        let (best1, _) = time_it(3, || {
            r.prepare((0.0, seconds)).unwrap();
            for _ in 0..blocks {
                r.render_block(&mut block, BLOCK_SIZE).unwrap();
                std::hint::black_box(&block);
            }
        });
        println!(
            "{label} 1 thread   best {:8.1} ms ({:5.2}x realtime)",
            ms(best1),
            seconds / best1.as_secs_f64()
        );
    }
    println!();
}

/// The public pieces the renderer leans on, measured on their own so the
/// ablation above can be checked against something independent.
fn bench_components() {
    println!("== components ==");

    for layout in [
        ChannelLayout::Stereo,
        ChannelLayout::Surround51,
        ChannelLayout::Atmos714,
    ] {
        let (best, _) = time_it(200, || {
            std::hint::black_box(VbapPanner::new(layout));
        });
        println!(
            "VbapPanner::new {:8}     best {:8.3} ms   ({} bases)",
            layout.label(),
            ms(best),
            VbapPanner::new(layout).base_count()
        );
    }

    for layout in [ChannelLayout::Stereo, ChannelLayout::Atmos714] {
        let p = VbapPanner::new(layout);
        let (best, _) = time_it(2_000, || {
            std::hint::black_box(pan_matrix(
                &p,
                PanLaw::ConstantPower3dB,
                None,
                -17.0,
                0.0,
                0.0,
                1,
            ));
        });
        let per = best.as_secs_f64() * 1e9;
        println!(
            "pan_matrix mono→{:8}    best {per:8.0} ns   ×48 tracks ×2 ends = {:6.3} ms/block",
            layout.label(),
            per * 96.0 / 1e6
        );
        let (best, _) = time_it(2_000, || {
            std::hint::black_box(pan_matrix(
                &p,
                PanLaw::ConstantPower3dB,
                None,
                -17.0,
                0.0,
                30.0,
                1,
            ));
        });
        println!(
            "  … with 30° spread          best {:8.0} ns",
            best.as_secs_f64() * 1e9
        );
    }

    // Allocation: what one block's worth of track buffers costs to create.
    for (n, ch) in [(48usize, 2usize), (128, 12)] {
        let (best, _) = time_it(500, || {
            let v: Vec<AudioBuffer> = (0..n)
                .map(|_| AudioBuffer::new(SR, ch, BLOCK_SIZE))
                .collect();
            std::hint::black_box(v);
        });
        println!(
            "{n:3} × AudioBuffer::new({ch:2} ch, {BLOCK_SIZE})  best {:8.3} ms",
            ms(best)
        );
    }

    // A limiter is rebuilt from scratch on every render call in the offline
    // design; its construction zeroes and fills three windows.
    let (best, _) = time_it(2_000, || {
        std::hint::black_box(Limiter::new(LimiterParams::default(), SR as f32, 12));
    });
    println!(
        "Limiter::new(12 ch)          best {:8.0} ns",
        best.as_secs_f64() * 1e9
    );

    let (best, _) = time_it(2_000, || {
        std::hint::black_box(Biquad::design(
            BiquadKind::Peaking,
            f64::from(SR),
            2_500.0,
            1.2,
            2.5,
        ));
    });
    println!(
        "Biquad::design               best {:8.0} ns",
        best.as_secs_f64() * 1e9
    );

    // Resampling a whole source buffer is done inside `render_track` whenever a
    // clip's rate differs from the session's; at 512 frames rendered that is a
    // spectacular amount of work per block.
    let src = AudioBuffer::sine(44_100, 1, 220.0, 0.25, 5.0);
    let (best, _) = time_it(3, || {
        std::hint::black_box(src.resample(SR));
    });
    println!(
        "resample 5 s 44.1→48 kHz     best {:8.3} ms   ← per clip, per render call",
        ms(best)
    );
    println!();
}
