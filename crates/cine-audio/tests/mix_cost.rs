//! Cost tripwires for the block loop.
//!
//! A wall-clock threshold is not a test. It passes on a quiet machine and fails
//! on a busy one, which means it reports the machine rather than the code, and
//! a test that cries wolf is removed rather than fixed. Everything here is
//! either a **count**, which does not depend on the machine at all, or a
//! **ratio** against a cheap reference operation measured on the same data at
//! the same moment, which divides the machine out.
//!
//! It is deliberately **one test**. Two of the three properties are read off a
//! process-wide allocation counter, and `cargo test` runs the tests in a binary
//! concurrently — a second test allocating on another thread would be counted
//! here and the result would depend on the scheduler. One test, run in
//! sequence, is the only way the counts mean anything.
//!
//! The three properties defended, each of which the offline renderer failed:
//!
//! 1. `render_block` allocates nothing once the renderer is prepared. A block
//!    loop that allocates cannot hold a deadline, because the one block that
//!    hits the allocator's slow path is the one that drops out.
//! 2. A block costs a small multiple of touching the same samples once. That
//!    is the sanity check on the whole signal path: the mixer does more than
//!    add, but not two orders of magnitude more.
//! 3. Compiling a session is paid once, not per block. This is the regression
//!    that made the renderer offline-only, and the one most easily
//!    reintroduced — a helper that rebuilds a `Renderer` inside the loop looks
//!    perfectly reasonable until it is measured. Also a count: compiling
//!    allocates per track, so where the allocations are says where the work is.

use cine_audio::mix::{Renderer, BLOCK_SIZE};
use cine_audio::prelude::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SR: u32 = 48_000;
const RATE: f64 = 24.0;
const TRACKS: usize = 48;

// ---------------------------------------------------------------------------
// A counting allocator, so "allocates nothing" can be asserted rather than hoped
// ---------------------------------------------------------------------------

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn allocs() -> usize {
    ALLOCS.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

/// A session of the size the deadline is argued about: 48 tracks, each a real
/// channel strip (filters, a compressor, a reverb send) over clips laid end to
/// end, with volume and pan automation running.
fn session(tracks: usize) -> Session {
    let mut s = Session::new(SR, RATE);
    let reverb = Bus::new("reverb");
    let reverb_id = reverb.id;
    s.buses.push(reverb);
    let stems = [Stem::Dialogue, Stem::Music, Stem::Effects];
    let buf = Arc::new(AudioBuffer::sine(SR, 1, 220.0, 0.25, 33.0));

    for i in 0..tracks {
        let mut t =
            Track::new(format!("t{i}"), s.master_id()).with_stem(stems[i % stems.len()].clone());
        for c in 0..8 {
            let mut clip =
                Clip::from_buffer(format!("c{c}"), Arc::clone(&buf), c as f64 * 4.0, RATE);
            clip.range = ucg_core::TimeRange::new(
                ucg_core::RationalTime::from_seconds(c as f64 * 4.0, RATE),
                ucg_core::RationalTime::from_seconds(4.0, RATE),
            );
            clip.gain_db = -3.0;
            clip.fade_in = Fade::new(0.05, FadeCurve::EqualPower);
            clip.fade_out = Fade::new(0.05, FadeCurve::EqualPower);
            t = t.with_clip(clip);
        }
        t.pan.azimuth_deg = -40.0 + 80.0 * (i as f32 / tracks.max(2) as f32);
        t.volume_db = -6.0;
        t.inserts
            .push(Insert::filter(BiquadKind::HighPass, 80.0, 0.707, 0.0));
        t.inserts
            .push(Insert::filter(BiquadKind::Peaking, 2_500.0, 1.2, 2.5));
        t.inserts
            .push(Insert::Compressor(CompressorParams::default()));
        t.sends.push(Send {
            bus: reverb_id,
            level_db: -12.0,
            pre_fader: false,
        });
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
        s.add_track(t);
    }
    s
}

// ---------------------------------------------------------------------------
// 1. Bounded allocation — a count, so no clock is involved at all
// ---------------------------------------------------------------------------

#[test]
fn the_block_loop_is_bounded_cheap_and_compiled_once() {
    bounded_allocation();
    cost_against_a_reference_pass();
    compilation_is_not_per_block();
}

/// 1. `render_block` allocates nothing once the renderer is prepared.
fn bounded_allocation() {
    let s = session(TRACKS);
    let mut r = Renderer::new(&s, ChannelLayout::Stereo).expect("compile");
    r.prepare((0.0, 60.0)).expect("prepare");
    let mut block = vec![0.0f32; BLOCK_SIZE * 2];

    // Warm-up: the first blocks wake rayon's workers, and a thread's first
    // allocation is not a per-block cost.
    for _ in 0..16 {
        r.render_block(&mut block, BLOCK_SIZE).expect("block");
    }

    let before = allocs();
    for _ in 0..8 {
        r.render_block(&mut block, BLOCK_SIZE).expect("block");
    }
    let mid = allocs();
    for _ in 0..64 {
        r.render_block(&mut block, BLOCK_SIZE).expect("block");
    }
    let after = allocs();

    let first = mid - before; // 8 blocks
    let second = after - mid; // 64 blocks
                              // Anything running per block allocates at least once per block, so 64
                              // blocks would cost at least 64. The measured figure is 1 — rayon growing
                              // a worker's queue once, not the block loop — and the bound sits an order
                              // of magnitude below one-per-block so that a per-block allocation cannot
                              // hide under it, while leaving room for the pool to settle.
    assert!(
        second <= 8,
        "the block loop allocates: 8 blocks took {first} allocations and the \
         next 64 took {second}. Every buffer the renderer needs is sized when \
         it is prepared; something in render_block is building one per call."
    );
}

// ---------------------------------------------------------------------------
// 2. A block costs a small multiple of touching the same samples
// ---------------------------------------------------------------------------

/// The reference: what a mixer cannot avoid doing. Scale each track's block by
/// a gain and sum it into an accumulator — one multiply-add per sample per
/// track, over exactly the sample count the real block loop moves.
fn reference_sum(tracks: &[Vec<f32>], acc: &mut [f32]) {
    acc.fill(0.0);
    for (i, t) in tracks.iter().enumerate() {
        let g = 0.5 + i as f32 * 1e-4;
        for (a, s) in acc.iter_mut().zip(t) {
            *a += s * g;
        }
    }
}

/// 2. A block costs a small multiple of touching the same samples once.
fn cost_against_a_reference_pass() {
    let s = session(TRACKS);
    let mut r = Renderer::new(&s, ChannelLayout::Stereo).expect("compile");
    r.prepare((0.0, 60.0)).expect("prepare");
    let mut block = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..16 {
        r.render_block(&mut block, BLOCK_SIZE).expect("block");
    }

    // The reference works on the same shape of data: 48 stereo blocks.
    let inputs: Vec<Vec<f32>> = (0..TRACKS)
        .map(|i| {
            (0..BLOCK_SIZE * 2)
                .map(|k| ((k + i) as f32 * 0.001).sin())
                .collect()
        })
        .collect();
    let mut acc = vec![0.0f32; BLOCK_SIZE * 2];
    reference_sum(&inputs, &mut acc);

    // Interleaved, so a burst of load from another process lands on both.
    let mut mix_best = Duration::MAX;
    let mut ref_best = Duration::MAX;
    for _ in 0..200 {
        let t = Instant::now();
        r.render_block(&mut block, BLOCK_SIZE).expect("block");
        mix_best = mix_best.min(t.elapsed());
        std::hint::black_box(&block);

        let t = Instant::now();
        reference_sum(&inputs, &mut acc);
        ref_best = ref_best.min(t.elapsed());
        std::hint::black_box(&acc);
    }

    let ratio = mix_best.as_secs_f64() / ref_best.as_secs_f64();
    // The mixer does far more per sample than the reference — gather, two
    // biquads in double precision, a compressor, a pan matrix, a fader, a send,
    // two bus sums and a lookahead limiter — but "far more" has a size. This
    // machine measures 81x optimised; the bound sits three times above that,
    // which is enough headroom for another machine's transcendental cost and
    // for a debug build, and not enough to hide a per-sample coefficient
    // recomputation or a per-block allocation — each of which moved this
    // figure by more than an order of magnitude when it was present.
    assert!(
        ratio < 250.0,
        "one {BLOCK_SIZE}-frame block of {TRACKS} tracks costs {ratio:.1}x a \
         gain-and-sum pass over the same samples ({:.3} ms vs {:.3} ms). \
         Something in the signal path is recomputing per sample what it could \
         compute per block.",
        mix_best.as_secs_f64() * 1e3,
        ref_best.as_secs_f64() * 1e3
    );
}

// ---------------------------------------------------------------------------
// 3. The session is compiled once, not once per block
// ---------------------------------------------------------------------------

/// 3. Compiling the session is paid once, not per block.
fn compilation_is_not_per_block() {
    // Stated as a count rather than a stopwatch. Compiling a session allocates
    // at least once per track — buffers, chains, resolved clips — so if
    // compilation happened per block, the allocation count would grow with the
    // number of blocks by at least that much. Streaming's must not grow at all.
    //
    // A timing comparison would say the same thing less reliably: the two
    // measurements would have to be taken at different moments, and on a
    // machine running someone else's build that difference is the machine's,
    // not the code's.
    let s = session(TRACKS);
    let mut r = Renderer::new(&s, ChannelLayout::Stereo).expect("compile");
    r.prepare((0.0, 60.0)).expect("prepare");
    let mut block = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..16 {
        r.render_block(&mut block, BLOCK_SIZE).expect("block");
    }

    const N: usize = 16;
    let before = allocs();
    for _ in 0..N {
        r.render_block(&mut block, BLOCK_SIZE).expect("block");
    }
    let streamed = allocs() - before;

    // The same N blocks of audio, asked for one block at a time from the
    // one-shot entry point — which is the only way a real-time caller could
    // use it, and what makes it an offline renderer.
    let before = allocs();
    for i in 0..N {
        let t = i as f64 * BLOCK_SIZE as f64 / SR as f64;
        std::hint::black_box(
            render(
                &s,
                (t, t + BLOCK_SIZE as f64 / SR as f64),
                ChannelLayout::Stereo,
            )
            .expect("render"),
        );
    }
    let one_shot = allocs() - before;

    assert!(
        one_shot >= N * TRACKS,
        "the one-shot renderer no longer compiles per call ({one_shot} \
         allocations for {N} blocks); this test is comparing against nothing"
    );
    assert!(
        streamed * 100 < one_shot,
        "streaming {N} blocks allocated {streamed} times against the one-shot \
         path's {one_shot}. Compilation has moved into the block loop."
    );
}
