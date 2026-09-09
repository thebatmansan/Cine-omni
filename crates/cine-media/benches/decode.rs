//! Measured decode performance on **real media**, reported as numbers rather
//! than adjectives.
//!
//! Deliberately harness-free and dependency-free: this is a measurement tool,
//! not a statistical benchmark suite, and adding a benchmarking framework to
//! the dependency graph for it would not be justified. Every figure it prints
//! is a wall-clock measurement of work that actually happened.
//!
//! Run with:
//! ```text
//! cargo bench -p cine-media --bench decode
//! ```
//!
//! If the real asset is absent it says so and exits, because a performance
//! number produced from synthetic input would be worse than no number.

use cine_media::identity::ContentHash;
use cine_media::time::Timestamp;
use cine_media::{probe, Frame, FrameCache, FrameKey, MediaReader, VideoReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const ASSETS: &[&str] = &["/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4"];

fn asset() -> Option<PathBuf> {
    ASSETS.iter().map(PathBuf::from).find(|p| p.exists())
}

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

fn main() {
    let Some(path) = asset() else {
        eprintln!(
            "SKIP: no real media asset on this machine; refusing to benchmark synthetic input"
        );
        eprintln!("      expected one of: {ASSETS:?}");
        return;
    };
    println!("asset: {}", path.display());
    let size = std::fs::metadata(&path).unwrap().len();
    println!("size:  {:.2} MiB\n", size as f64 / (1024.0 * 1024.0));

    bench_identity(&path, size);
    bench_probe(&path);
    bench_decode(&path);
    bench_video(&path);
    bench_seek(&path);
    bench_video_seek(&path);
    bench_cache(&path);
}

/// Identity is on the ingest path for every file in a project, so the cost of
/// hashing decides whether cataloguing a card is seconds or minutes.
fn bench_identity(path: &Path, size: u64) {
    println!("== identity ==");
    let (best, mean) = time_it(20, || {
        ContentHash::fast(path).unwrap();
    });
    println!(
        "fast hash (head+tail+len)   best {:8.3} ms   mean {:8.3} ms",
        ms(best),
        ms(mean)
    );

    let (best, mean) = time_it(5, || {
        ContentHash::full(path).unwrap();
    });
    let mib_s = (size as f64 / (1024.0 * 1024.0)) / best.as_secs_f64();
    println!(
        "full hash (whole file)      best {:8.3} ms   mean {:8.3} ms   {:.0} MiB/s",
        ms(best),
        ms(mean),
        mib_s
    );
    println!(
        "→ fast hashing is {:.0}x cheaper; it is the ingest default, full is for conform\n",
        mean.as_secs_f64()
            / time_it(20, || {
                ContentHash::fast(path).unwrap();
            })
            .1
            .as_secs_f64()
    );
}

/// Probing must be cheap enough to catalogue thousands of files.
fn bench_probe(path: &Path) {
    println!("== probe ==");
    let (best, mean) = time_it(20, || {
        probe(path).unwrap();
    });
    println!(
        "probe (hash + sample tables) best {:8.3} ms   mean {:8.3} ms",
        ms(best),
        ms(mean)
    );
    let per_hour = 3600.0 / mean.as_secs_f64();
    println!("→ ~{per_hour:.0} files/hour-equivalent at this cost per file\n");
}

/// Decode throughput, expressed as a realtime factor. Anything above 1.0x can
/// sustain playback of this media; editorial wants a large margin for scrubbing.
fn bench_decode(path: &Path) {
    println!("== decode (real AAC, 48 kHz stereo) ==");
    let mut media_seconds = 0.0;
    let (best, mean) = time_it(5, || {
        let mut r = MediaReader::open_path(path).unwrap();
        let mut samples = 0usize;
        let mut rate = 48_000u32;
        while let Some(f) = r.next_audio_frame().unwrap() {
            samples += f.frame_count();
            rate = f.sample_rate;
        }
        media_seconds = samples as f64 / f64::from(rate);
    });
    println!(
        "full-file decode            best {:8.3} ms   mean {:8.3} ms",
        ms(best),
        ms(mean)
    );
    println!("decoded media duration      {media_seconds:.3} s");
    println!(
        "→ {:.0}x realtime (best), {:.0}x realtime (mean)\n",
        media_seconds / best.as_secs_f64(),
        media_seconds / mean.as_secs_f64()
    );
}

/// Video decode is the expensive path and the one that decides whether
/// playback is possible at all. Reported as a realtime factor against the
/// stream's own 24 fps.
fn bench_video(path: &Path) {
    println!("== decode (real H.264 High profile, 720x1280) ==");
    let mut frames = 0usize;
    let mut bytes = 0usize;
    let (best, mean) = time_it(3, || {
        let mut r = VideoReader::open_path(path).unwrap();
        frames = 0;
        bytes = 0;
        while let Some(f) = r.next_frame().unwrap() {
            frames += 1;
            bytes += f.byte_size();
        }
    });
    println!(
        "full-track decode           best {:8.3} ms   mean {:8.3} ms",
        ms(best),
        ms(mean)
    );
    let fps_best = frames as f64 / best.as_secs_f64();
    println!(
        "{frames} frames, {:.1} MiB decoded",
        bytes as f64 / (1024.0 * 1024.0)
    );
    println!(
        "→ {fps_best:.1} fps decode ({:.2}x realtime at 24 fps), {:.1} MiB/s of frame data",
        fps_best / 24.0,
        (bytes as f64 / (1024.0 * 1024.0)) / best.as_secs_f64()
    );
    println!("   single-threaded, software, no hardware decode claimed\n");
}

/// Scrubbing cost. A seek must re-enter at a keyframe and decode forward, so
/// the honest number is the worst case, not the best.
fn bench_video_seek(path: &Path) {
    println!("== video seek ==");
    let mut r = VideoReader::open_path(path).unwrap();
    println!(
        "{} frames, {} keyframes → mean GOP {:.0} frames",
        r.frame_count(),
        r.keyframe_count(),
        r.frame_count() as f64 / r.keyframe_count().max(1) as f64
    );

    // Forward-adjacent: the frame after the current play head, the scrub case.
    let near = r.pts_at(10).unwrap();
    let (best_near, mean_near) = time_it(5, || {
        r.rewind().unwrap();
        std::hint::black_box(r.frame_at(near).unwrap());
    });
    println!(
        "seek near the entry point   best {:8.3} ms   mean {:8.3} ms",
        ms(best_near),
        ms(mean_near)
    );

    // Deep into a long GOP: the worst case for decode-forward seeking.
    let deep = r.pts_at(r.frame_count() - 1).unwrap();
    let (best_deep, mean_deep) = time_it(3, || {
        r.rewind().unwrap();
        std::hint::black_box(r.frame_at(deep).unwrap());
    });
    println!(
        "seek deep into the GOP      best {:8.3} ms   mean {:8.3} ms",
        ms(best_deep),
        ms(mean_deep)
    );

    // Re-requesting the frame at the play head must not re-decode the GOP.
    let (best_repeat, _) = time_it(20, || {
        std::hint::black_box(r.frame_at(deep).unwrap());
    });
    println!(
        "re-request the same frame   best {:8.3} ms",
        ms(best_repeat)
    );
    println!(
        "→ worst-case scrub is {:.0} ms because entry is only possible at a keyframe;\n   \
         this is precisely the cost a proxy or an index would remove\n",
        ms(best_deep)
    );
}

/// Seek latency governs whether scrubbing feels attached to the cursor.
fn bench_seek(path: &Path) {
    println!("== seek + decode ==");
    let mut r = MediaReader::open_path(path).unwrap();
    let base = r.audio_time_base();

    // Sequential-ish: repeated seeks to the same point, the scrub-in-place case.
    let target = Timestamp::exact(5 * 48_000, base);
    let (best, mean) = time_it(20, || {
        r.decode_audio_at(target).unwrap();
    });
    println!(
        "seek to a fixed point       best {:8.3} ms   mean {:8.3} ms",
        ms(best),
        ms(mean)
    );

    // Scattered: the worst case, jumping across the file each time.
    let points: Vec<Timestamp> = (0..8)
        .map(|i| Timestamp::exact(i64::from(i) * 48_000 + 12_000, base))
        .collect();
    let mut i = 0usize;
    let (best, mean) = time_it(40, || {
        let t = points[i % points.len()];
        i += 1;
        r.decode_audio_at(t).unwrap();
    });
    println!(
        "scattered random-access     best {:8.3} ms   mean {:8.3} ms",
        ms(best),
        ms(mean)
    );
    println!("→ seek is decode-forward from the nearest packet, so cost tracks distance\n");
}

/// The cache exists to remove repeated decode work; this measures whether it
/// actually does, on frames that came out of a real decoder.
fn bench_cache(path: &Path) {
    println!("== frame cache ==");
    let mut r = MediaReader::open_path(path).unwrap();
    let hash = r.info().hash.clone();
    let frames = r.decode_audio_frames(200).unwrap();
    if frames.is_empty() {
        println!("no frames decoded; skipping\n");
        return;
    }
    let cache = FrameCache::with_default_capacity();
    let keys: Vec<FrameKey> = frames
        .iter()
        .map(|f| FrameKey::new(&hash, 1, f.pts))
        .collect();

    let (best_insert, mean_insert) = time_it(5, || {
        cache.clear();
        for (k, f) in keys.iter().zip(&frames) {
            cache.insert(k.clone(), Arc::new(Frame::Audio(f.clone())));
        }
    });
    println!(
        "insert {:3} frames           best {:8.3} ms   mean {:8.3} ms",
        frames.len(),
        ms(best_insert),
        ms(mean_insert)
    );

    let (best_hit, mean_hit) = time_it(200, || {
        for k in &keys {
            std::hint::black_box(cache.get(k));
        }
    });
    println!(
        "{:3} cache hits              best {:8.3} ms   mean {:8.3} ms",
        keys.len(),
        ms(best_hit),
        ms(mean_hit)
    );

    // The comparison that matters: a hit versus decoding the same span again.
    let (best_decode, _) = time_it(5, || {
        let mut r = MediaReader::open_path(path).unwrap();
        std::hint::black_box(r.decode_audio_frames(keys.len()).unwrap());
    });
    println!(
        "decoding the same {:3} frames best {:8.3} ms",
        keys.len(),
        ms(best_decode)
    );
    println!(
        "→ cache is {:.0}x cheaper than re-decoding the same span",
        best_decode.as_secs_f64() / best_hit.as_secs_f64()
    );
    let s = cache.stats();
    println!(
        "   {} entries, {:.2} MiB resident, {} evictions, hit rate {:.1}%\n",
        s.entries,
        s.bytes as f64 / (1024.0 * 1024.0),
        s.evictions,
        s.hit_rate() * 100.0
    );
}
