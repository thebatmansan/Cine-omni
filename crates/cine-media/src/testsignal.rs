//! A test signal to line up picture and sound: colour bars with a sync pop.
//!
//! Every second, one picture carries a white flash in the strip along the
//! bottom and the same picture's sound is a 1 kHz tone at −20 dBFS; the rest
//! is silence. The flash and the pop start on the same sample, so any device,
//! player or pipeline the file passes through can be checked for picture and
//! sound arriving together. A block in the strip moves one step per picture,
//! so a dropped or repeated picture is visible.
//!
//! The bars are the eight 75% bars — white, yellow, cyan, green, magenta, red,
//! blue, black — encoded BT.709 limited range and declared so.

use crate::encode::{H264Settings, YuvFrame};
use crate::error::{Error, Result};
use crate::mov::{ColourTags, MovReport, MovSettings, MovWriter, SoundFormat};
use crate::time::Timecode;
use std::path::Path;
use ucg_core::Rate;

/// The 75% bars, left to right, as RGB code values in `[0, 1]`.
pub const BARS: [[f64; 3]; 8] = [
    [0.75, 0.75, 0.75],
    [0.75, 0.75, 0.0],
    [0.0, 0.75, 0.75],
    [0.0, 0.75, 0.0],
    [0.75, 0.0, 0.75],
    [0.75, 0.0, 0.0],
    [0.0, 0.0, 0.75],
    [0.0, 0.0, 0.0],
];

/// Pop level: −20 dBFS.
pub const POP_AMPLITUDE: f64 = 0.1;
/// Pop frequency in hertz.
pub const POP_HZ: f64 = 1000.0;
/// Sound sample rate.
pub const SAMPLE_RATE: u32 = 48_000;

/// BT.709 limited-range Y, Cb, Cr codes for an RGB code value.
pub fn bt709_limited(rgb: [f64; 3]) -> (u8, u8, u8) {
    let (kr, kb) = (0.2126, 0.0722);
    let y = kr * rgb[0] + (1.0 - kr - kb) * rgb[1] + kb * rgb[2];
    let cb = (rgb[2] - y) / (2.0 * (1.0 - kb));
    let cr = (rgb[0] - y) / (2.0 * (1.0 - kr));
    let code = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    (
        code(16.0 + 219.0 * y),
        code(128.0 + 224.0 * cb),
        code(128.0 + 224.0 * cr),
    )
}

/// Whether picture `k` at `rate` is the first picture of a second.
pub fn is_pop(rate: Rate, k: u64) -> bool {
    // Picture k starts at k·den/num seconds, inside second s = floor of that.
    // It is the pop when it is the first picture starting at or after s.
    let num = u128::from(rate.num());
    let den = u128::from(rate.den());
    let k = u128::from(k);
    let second = k * den / num;
    k == (second * num).div_ceil(den)
}

/// The timecode a bars-and-pops roll starts at.
///
/// One hour is the convention for the first frame of programme, and a test
/// roll that starts there is one a conform can be checked against without
/// anybody having to remember an arbitrary number. A drop-frame rate gets a
/// drop-frame label, because at 29.97 the non-drop one would be a different
/// instant.
pub const START: &str = "01:00:00:00";

/// Write `seconds` of bars and pops, `width × height` at `rate`, to `path`.
///
/// The movie carries source timecode from [`START`]: test media that has none
/// cannot exercise the paths that read it.
pub fn bars_and_pops(
    path: &Path,
    width: usize,
    height: usize,
    rate: Rate,
    seconds: u32,
) -> Result<MovReport> {
    bars_and_pops_from(path, width, height, rate, seconds, start_timecode(rate))
}

/// [`START`] at `rate`, drop-frame where the rate is.
pub fn start_timecode(rate: Rate) -> Option<Timecode> {
    // Drop-frame is written with a semicolon before the frames, which is the
    // only thing in a timecode string that says which of two different
    // instants the label means.
    let drop = rate.supports_drop_frame();
    let text = match (drop, START.rsplit_once(':')) {
        (true, Some((head, frames))) => format!("{head};{frames}"),
        _ => START.to_string(),
    };
    Timecode::parse(&text, rate).ok()
}

/// Write `seconds` of bars and pops starting at `timecode`.
pub fn bars_and_pops_from(
    path: &Path,
    width: usize,
    height: usize,
    rate: Rate,
    seconds: u32,
    timecode: Option<Timecode>,
) -> Result<MovReport> {
    if height < 16 {
        return Err(Error::Unsupported(
            "bars need a picture at least 16 lines tall".into(),
        ));
    }
    let frames = (u64::from(seconds) * u64::from(rate.num())).div_ceil(u64::from(rate.den()));
    let mut w = MovWriter::create(
        path,
        width,
        height,
        rate,
        MovSettings {
            h264: H264Settings { qp: 12, gop: 24 },
            sound: Some(SoundFormat {
                sample_rate: SAMPLE_RATE,
                channels: 2,
            }),
            colour: ColourTags::BT709,
            timecode,
            first_picture: 0,
        },
    )?;
    let strip = (height / 8) & !1;
    let (cw, ch) = (width / 2, height / 2);
    for k in 0..frames {
        let pop = is_pop(rate, k);
        let mut y = vec![0u8; width * height];
        let mut u = vec![128u8; cw * ch];
        let mut v = vec![128u8; cw * ch];
        for row in 0..height {
            for col in 0..width {
                let (yy, cb, cr) = if row >= height - strip {
                    let block = (k as usize * 4) % width.saturating_sub(8).max(1);
                    if pop || (col >= block && col < block + 8) {
                        (235, 128, 128)
                    } else {
                        (16, 128, 128)
                    }
                } else {
                    bt709_limited(BARS[(col * 8 / width).min(7)])
                };
                y[row * width + col] = yy;
                if row % 2 == 0 && col % 2 == 0 {
                    u[(row / 2) * cw + col / 2] = cb;
                    v[(row / 2) * cw + col / 2] = cr;
                }
            }
        }
        let due = w.sound_frames_due();
        let mut sound = Vec::with_capacity(due as usize * 2);
        for i in 0..due {
            let s = if pop {
                (POP_AMPLITUDE
                    * (std::f64::consts::TAU * POP_HZ * i as f64 / f64::from(SAMPLE_RATE)).sin())
                    as f32
            } else {
                0.0
            };
            sound.push(s);
            sound.push(s);
        }
        w.write_frame(
            &YuvFrame {
                width,
                height,
                y,
                u,
                v,
            },
            &sound,
        )?;
    }
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pops_fall_on_the_first_picture_of_each_second() {
        let pops = |rate: Rate, n: u64| (0..n).filter(|k| is_pop(rate, *k)).collect::<Vec<_>>();
        assert_eq!(pops(Rate::F24, 73), vec![0, 24, 48, 72]);
        // 29.97: second 1 starts at 30000/1001 pictures = 29.97, so picture 30.
        let ntsc = Rate::new(30000, 1001).unwrap();
        assert_eq!(pops(ntsc, 91), vec![0, 30, 60, 90]);
        // 23.976: second 2 starts at 47.95 pictures, so picture 48.
        let film = Rate::new(24000, 1001).unwrap();
        assert_eq!(pops(film, 73), vec![0, 24, 48, 72]);
    }

    #[test]
    fn seventy_five_percent_white_is_the_standard_code() {
        // 75% white is Y 180 in BT.709 limited range, with neutral chroma.
        assert_eq!(bt709_limited([0.75, 0.75, 0.75]), (180, 128, 128));
        assert_eq!(bt709_limited([0.0, 0.0, 0.0]), (16, 128, 128));
    }
}
