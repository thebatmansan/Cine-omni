//! Renderer tests: levels, panning, automation, solo/mute, stems.

use cine_audio::prelude::*;
use std::sync::Arc;

const SR: u32 = 48_000;

fn sine_track(name: &str, session: &Session, amp: f32, seconds: f64) -> Track {
    let buf = Arc::new(AudioBuffer::sine(SR, 1, 1000.0, amp, seconds));
    Track::new(name, session.master_id()).with_clip(Clip::from_buffer(
        "tone",
        buf,
        0.0,
        session.rate,
    ))
}

fn max_diff(a: &AudioBuffer, b: &AudioBuffer) -> f32 {
    assert_eq!(a.frames(), b.frames());
    assert_eq!(a.channels, b.channels);
    a.data
        .iter()
        .zip(&b.data)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max)
}

#[test]
fn mono_sine_at_minus_6_db_centre_panned_to_stereo() {
    // Full-scale sine (RMS 0.7071) through a −6 dB fader: 0.354 RMS pre-pan.
    // Constant-power centre pan puts 0.7071 × that on each channel: 0.2506.
    let mut s = Session::new(SR, 24.0);
    let mut t = sine_track("dx", &s, 1.0, 1.0).with_stem(Stem::Dialogue);
    t.volume_db = -6.0;
    s.add_track(t);
    let out = render(&s, (0.0, 1.0), ChannelLayout::Stereo).unwrap();
    assert_eq!(out.channels, 2);
    assert_eq!(out.frames(), SR as usize);
    let expected =
        std::f32::consts::FRAC_1_SQRT_2 * db_to_lin(-6.0) * std::f32::consts::FRAC_1_SQRT_2;
    assert!((expected - 0.2506).abs() < 1e-3);
    for c in 0..2 {
        let rms = out.rms_channel(c);
        assert!(
            (rms - expected).abs() / expected < 0.03,
            "channel {c} rms {rms} vs {expected}"
        );
    }
    assert!((out.rms_channel(0) - out.rms_channel(1)).abs() < 1e-4);
    // Hard left: everything on L.
    let t = s.tracks.first_mut().unwrap();
    t.pan = PanState::stereo(-1.0);
    let out = render(&s, (0.0, 1.0), ChannelLayout::Stereo).unwrap();
    assert!(out.rms_channel(1) < 1e-6, "R leaked {}", out.rms_channel(1));
    assert!((out.rms_channel(0) - db_to_lin(-6.0) * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
}

#[test]
fn surround_and_mono_layouts() {
    let mut s = Session::new(SR, 24.0);
    s.add_track(sine_track("c", &s, 0.5, 0.5));
    let out = render(&s, (0.0, 0.5), ChannelLayout::Surround51).unwrap();
    assert_eq!(out.channels, 6);
    let c = out.rms_channel(2);
    assert!(
        (c - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01,
        "C rms {c}"
    );
    for ch in [0, 1, 3, 4, 5] {
        assert!(out.rms_channel(ch) < 1e-5, "leak into {ch}");
    }
    let mono = render(&s, (0.0, 0.5), ChannelLayout::Mono).unwrap();
    assert_eq!(mono.channels, 1);
    assert!((mono.rms() - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
}

#[test]
fn clip_gain_fades_offsets_and_placeholders() {
    let mut s = Session::new(SR, 24.0);
    let buf = Arc::new(AudioBuffer::sine(SR, 1, 1000.0, 0.5, 1.0));
    let mut clip = Clip::from_buffer("tone", buf, 0.5, 24.0);
    clip.gain_db = -20.0;
    clip.fade_in = Fade::new(0.1, FadeCurve::Linear);
    clip.fade_out = Fade::new(0.1, FadeCurve::EqualPower);
    let mut t = Track::new("fx", s.master_id());
    t.pan = PanState::stereo(-1.0);
    t.clips.push(clip);
    t.clips.push(Clip::placeholder("later", 2.0, 1.0, 24.0));
    s.add_track(t);
    let out = render(&s, (0.0, 3.0), ChannelLayout::Stereo).unwrap();
    // Silence before the clip and where the placeholder sits.
    assert!(out.slice(0, 24_000).peak() < 1e-6);
    assert!(out.slice(2 * SR as usize, SR as usize).peak() < 1e-6);
    // Fades: first frame ~0, body at −20 dB.
    assert!(out.sample(24_000, 0).abs() < 1e-4);
    let body = out.slice(24_000 + 4800, 38_400);
    assert!(
        (body.rms_channel(0) - 0.5 * std::f32::consts::FRAC_1_SQRT_2 * db_to_lin(-20.0)).abs()
            < 1e-3
    );
    let tail = out.slice(72_000 - 100, 100);
    assert!(tail.peak() < 0.05);
    // A sub-range render lines up with the full render.
    let part = render(&s, (0.75, 1.25), ChannelLayout::Stereo).unwrap();
    let full_part = out.slice(36_000, 24_000);
    assert!(max_diff(&part, &full_part) < 1e-5);
}

#[test]
fn volume_automation_ramp_changes_level_across_the_render() {
    let mut s = Session::new(SR, 24.0);
    let mut t = sine_track("dx", &s, 0.5, 2.0);
    t.automation.set_lane(
        Param::Volume,
        Lane::new(vec![(0.0, 0.0), (2.0, -40.0)], Interpolation::Linear),
    );
    s.add_track(t);
    let out = render(&s, (0.0, 2.0), ChannelLayout::Stereo).unwrap();
    let first = out.slice(0, 4800).rms();
    let last = out.slice(2 * SR as usize - 4800, 4800).rms();
    assert!(first > 0.2, "start rms {first}");
    assert!(last < first * 0.05, "end rms {last} vs start {first}");
    // The ramp is monotonic block to block (no zipper steps upward).
    let mut prev = f32::INFINITY;
    for i in 0..20 {
        let r = out.slice(i * 4800, 4800).rms();
        assert!(r < prev, "block {i}: {r} >= {prev}");
        prev = r;
    }
    // Pan automation moves the image from L to R.
    let mut s = Session::new(SR, 24.0);
    let mut t = sine_track("dx", &s, 0.5, 2.0);
    t.automation.set_lane(
        Param::Pan,
        Lane::new(vec![(0.0, 90.0), (2.0, -90.0)], Interpolation::Linear),
    );
    s.add_track(t);
    let out = render(&s, (0.0, 2.0), ChannelLayout::Stereo).unwrap();
    let head = out.slice(0, 2400);
    let tail = out.slice(2 * SR as usize - 2400, 2400);
    assert!(
        head.rms_channel(0) > 0.3 && head.rms_channel(1) < 0.03,
        "{} {}",
        head.rms_channel(0),
        head.rms_channel(1)
    );
    assert!(tail.rms_channel(1) > 0.3 && tail.rms_channel(0) < 0.03);
}

#[test]
fn solo_and_mute() {
    let mut s = Session::new(SR, 24.0);
    let a = s.add_track(sine_track("a", &s, 0.2, 1.0));
    let b = s.add_track(sine_track("b", &s, 0.2, 1.0));
    let both = render(&s, (0.0, 1.0), ChannelLayout::Stereo).unwrap();
    let ref_level = 0.2 * std::f32::consts::FRAC_1_SQRT_2 * std::f32::consts::FRAC_1_SQRT_2;
    assert!((both.rms_channel(0) - 2.0 * ref_level).abs() < 0.01);
    s.track_mut(b).unwrap().mute = true;
    let only_a = render(&s, (0.0, 1.0), ChannelLayout::Stereo).unwrap();
    assert!((only_a.rms_channel(0) - ref_level).abs() < 0.01);
    s.track_mut(b).unwrap().mute = false;
    s.track_mut(b).unwrap().solo = true;
    let only_b = render(&s, (0.0, 1.0), ChannelLayout::Stereo).unwrap();
    assert!((only_b.rms_channel(0) - ref_level).abs() < 0.01);
    assert!(
        max_diff(&only_a, &only_b) < 1e-5,
        "identical tracks render identically"
    );
    // Mute automation.
    s.track_mut(b).unwrap().solo = false;
    s.track_mut(a).unwrap().automation.set_lane(
        Param::Mute,
        Lane::new(vec![(0.0, 1.0)], Interpolation::Step),
    );
    let out = render(&s, (0.0, 1.0), ChannelLayout::Stereo).unwrap();
    assert!((out.rms_channel(0) - ref_level).abs() < 0.01);
    // Master mute silences everything.
    s.master.mute = true;
    assert!(
        render(&s, (0.0, 1.0), ChannelLayout::Stereo)
            .unwrap()
            .peak()
            == 0.0
    );
}

#[test]
fn stems_sum_to_the_full_mix() {
    // Levels stay far below the master limiter ceiling (−1 dBTP), and every
    // bus insert is linear, so master_inserts(Σ stems) == Σ stems == mix.
    let mut s = Session::new(SR, 24.0);
    let mut dx = sine_track("dx", &s, 0.1, 1.0).with_stem(Stem::Dialogue);
    dx.inserts
        .push(Insert::filter(BiquadKind::HighPass, 100.0, 0.707, 0.0));
    let mut mx = Track::new("mx", s.master_id()).with_stem(Stem::Music);
    let stereo = Arc::new(AudioBuffer::sine(SR, 2, 220.0, 0.1, 1.0));
    mx.clips.push(Clip::from_buffer("bed", stereo, 0.0, 24.0));
    let mut fx = sine_track("fx", &s, 0.1, 1.0);
    fx.pan = PanState::azimuth(60.0);
    fx.output = s.stem_bus(&Stem::Effects).unwrap().id;
    // A reverb-style aux bus with an EQ, fed by a post-fader send from FX.
    let mut aux = Bus::new("verb");
    aux.inserts
        .push(Insert::filter(BiquadKind::LowPass, 4000.0, 0.707, 0.0));
    aux.volume_db = -3.0;
    let aux_id = aux.id;
    s.buses.push(aux);
    fx.sends.push(Send {
        bus: aux_id,
        level_db: -6.0,
        pre_fader: false,
    });
    let untagged = sine_track("beep", &s, 0.05, 1.0);
    s.add_track(dx);
    s.add_track(mx);
    s.add_track(fx);
    s.add_track(untagged);

    for layout in [ChannelLayout::Stereo, ChannelLayout::Surround51] {
        let mix = render(&s, (0.0, 1.0), layout).unwrap();
        let stems = render_stems(&s, (0.0, 1.0), layout).unwrap();
        assert!(
            stems.contains_key(&Stem::Dialogue)
                && stems.contains_key(&Stem::Music)
                && stems.contains_key(&Stem::Effects)
        );
        assert!(stems.contains_key(&cine_audio::mix::unassigned_stem()));
        // The aux bus carries no stem, so the send lands in "unassigned".
        assert!(stems[&Stem::Effects].rms() > 0.01);
        let mut sum = AudioBuffer::new(SR, layout.channel_count(), SR as usize);
        for b in stems.values() {
            for (d, x) in sum.data.iter_mut().zip(&b.data) {
                *d += x;
            }
        }
        let err = max_diff(&sum, &mix);
        assert!(
            err < 1e-4,
            "{}: stems differ from mix by {err}",
            layout.label()
        );
        assert!(mix.rms() > 0.05);
    }
}

#[test]
fn master_limiter_holds_the_ceiling() {
    let mut s = Session::new(SR, 24.0);
    s.add_track(sine_track("hot", &s, 1.0, 0.5));
    s.add_track(sine_track("hotter", &s, 1.0, 0.5));
    let out = render(&s, (0.0, 0.5), ChannelLayout::Stereo).unwrap();
    assert!(out.peak() <= db_to_lin(-1.0) * 1.001, "peak {}", out.peak());
    // Track-level limiter with lookahead is latency compensated: the render
    // is time-aligned with an un-limited quiet render.
    let mut s = Session::new(SR, 24.0);
    let mut t = sine_track("q", &s, 0.1, 0.5);
    t.inserts.push(Insert::Limiter(LimiterParams::default()));
    s.add_track(t);
    let limited = render(&s, (0.0, 0.5), ChannelLayout::Stereo).unwrap();
    s.tracks[0].inserts.clear();
    let plain = render(&s, (0.0, 0.5), ChannelLayout::Stereo).unwrap();
    assert!(max_diff(&limited, &plain) < 1e-5);
}

#[test]
fn entity_sources_resolve_through_session_media() {
    let mut s = Session::new(SR, 24.0);
    let media = ucg_core::EntityId::new();
    s.media.insert(
        media,
        Arc::new(AudioBuffer::sine(44_100, 1, 1000.0, 0.5, 1.0)),
    );
    let mut clip = Clip::placeholder("m", 0.0, 1.0, 24.0);
    clip.source = ClipSource::Entity(media);
    s.add_track(Track::new("m", s.master_id()).with_clip(clip));
    let out = render(&s, (0.0, 1.0), ChannelLayout::Stereo).unwrap();
    // Resampled from 44.1 k: level is preserved.
    assert!(
        (out.rms_channel(0) - 0.5 * 0.5).abs() < 0.01,
        "{}",
        out.rms_channel(0)
    );
}
