//! H.264 sequence parameter set: what a stream declares about itself.
//!
//! Written from ITU-T H.264 §7.3.2.1.1 (`seq_parameter_set_data`) and Annex E
//! (`vui_parameters`), not from any other implementation. It reads only as far
//! as the fields the media engine uses — coded size and cropping, bit depth,
//! chroma format, and the video signal type: range and the H.273 colour code
//! points — and stops before the HRD parameters, which it does not need.
//!
//! ## Why this matters
//!
//! Without it the decoder's pictures carried no colour description at all and
//! the viewer had to assume one. A stream that *declares* BT.709 limited range
//! should be shown as declared, and a stream that declares nothing should be
//! reported as declaring nothing — those are different facts, and conflating
//! them is how a picture ends up quietly wrong.
//!
//! Absent fields take the values the standard infers: `video_full_range_flag`
//! is 0 (limited range) when the signal type is not present; colour code points
//! are "unspecified" (2) when the colour description is not present, which this
//! reports as `None` rather than as any particular matrix.

use crate::error::{Error, Result};
use crate::frame::ColorMetadata;

/// What the SPS declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sps {
    pub profile_idc: u8,
    pub level_idc: u8,
    /// 0 monochrome, 1 4:2:0, 2 4:2:2, 3 4:4:4.
    pub chroma_format_idc: u32,
    pub bit_depth_luma: u32,
    pub bit_depth_chroma: u32,
    /// Displayed size after the frame cropping rectangle.
    pub width: u32,
    pub height: u32,
    pub frame_mbs_only: bool,
    pub vui: Option<Vui>,
}

/// The parts of `vui_parameters()` the engine uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Vui {
    pub sample_aspect: Option<(u32, u32)>,
    pub video_signal_type_present: bool,
    pub full_range: bool,
    pub colour_description_present: bool,
    pub colour_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    /// `(num_units_in_tick, time_scale)` when timing info is present.
    pub timing: Option<(u32, u32)>,
}

impl Sps {
    /// The colour description as the picture should carry it.
    ///
    /// Code point 2 means "unspecified" in H.273 and is reported as `None`.
    /// Range is always known: stated, or inferred as limited by the standard.
    pub fn color(&self) -> ColorMetadata {
        let declared = |v: u8| (v != 2).then_some(v);
        match &self.vui {
            Some(v) if v.colour_description_present => ColorMetadata {
                primaries: declared(v.colour_primaries),
                transfer: declared(v.transfer_characteristics),
                matrix: declared(v.matrix_coefficients),
                full_range: Some(v.full_range),
            },
            Some(v) => ColorMetadata {
                full_range: Some(v.full_range),
                ..Default::default()
            },
            None => ColorMetadata {
                full_range: Some(false),
                ..Default::default()
            },
        }
    }
}

/// Remove emulation prevention: `00 00 03` → `00 00`.
fn rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Result<u32> {
        let byte = self
            .data
            .get(self.pos / 8)
            .ok_or_else(|| Error::Container("SPS ended early".into()))?;
        let v = (byte >> (7 - (self.pos % 8))) & 1;
        self.pos += 1;
        Ok(u32::from(v))
    }
    fn bits(&mut self, n: u32) -> Result<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Ok(v)
    }
    fn flag(&mut self) -> Result<bool> {
        Ok(self.bit()? == 1)
    }
    /// ue(v), §9.1.
    fn ue(&mut self) -> Result<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return Err(Error::Container("SPS: invalid exp-Golomb code".into()));
            }
        }
        Ok((1u32 << zeros) - 1 + self.bits(zeros)?)
    }
    /// se(v), §9.1.1.
    fn se(&mut self) -> Result<i32> {
        let k = self.ue()? as i64;
        Ok(if k % 2 == 1 {
            ((k + 1) / 2) as i32
        } else {
            (-(k / 2)) as i32
        })
    }
}

fn skip_scaling_list(b: &mut Bits, size: usize) -> Result<()> {
    let (mut last, mut next) = (8i32, 8i32);
    for _ in 0..size {
        if next != 0 {
            let delta = b.se()?;
            next = (last + delta + 256).rem_euclid(256);
        }
        if next != 0 {
            last = next;
        }
    }
    Ok(())
}

/// Parse an SPS NAL unit, with or without its one-byte NAL header.
pub fn parse(nal: &[u8]) -> Result<Sps> {
    let data = rbsp(nal);
    let body = match data.first() {
        Some(h) if h & 0x1f == 7 => &data[1..],
        _ => &data[..],
    };
    let mut b = Bits { data: body, pos: 0 };
    let profile_idc = b.bits(8)? as u8;
    let _constraints = b.bits(8)?;
    let level_idc = b.bits(8)? as u8;
    let _sps_id = b.ue()?;

    let mut chroma_format_idc = 1;
    let mut separate_colour_plane = false;
    let (mut bit_depth_luma, mut bit_depth_chroma) = (8, 8);
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma_format_idc = b.ue()?;
        if chroma_format_idc == 3 {
            separate_colour_plane = b.flag()?;
        }
        bit_depth_luma = b.ue()? + 8;
        bit_depth_chroma = b.ue()? + 8;
        let _bypass = b.flag()?;
        if b.flag()? {
            let lists = if chroma_format_idc != 3 { 8 } else { 12 };
            for i in 0..lists {
                if b.flag()? {
                    skip_scaling_list(&mut b, if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    let _log2_max_frame_num = b.ue()?;
    match b.ue()? {
        0 => {
            let _ = b.ue()?;
        }
        1 => {
            let _ = b.flag()?;
            let _ = b.se()?;
            let _ = b.se()?;
            let cycle = b.ue()?;
            for _ in 0..cycle {
                let _ = b.se()?;
            }
        }
        _ => {}
    }
    let _max_refs = b.ue()?;
    let _gaps = b.flag()?;
    let width_mbs = b.ue()? + 1;
    let height_map_units = b.ue()? + 1;
    let frame_mbs_only = b.flag()?;
    if !frame_mbs_only {
        let _mbaff = b.flag()?;
    }
    let _direct_8x8 = b.flag()?;
    let (mut cl, mut cr, mut ct, mut cb) = (0, 0, 0, 0);
    if b.flag()? {
        cl = b.ue()?;
        cr = b.ue()?;
        ct = b.ue()?;
        cb = b.ue()?;
    }
    // §7.4.2.1.1: the crop units depend on the chroma sampling.
    let chroma_array_type = if separate_colour_plane {
        0
    } else {
        chroma_format_idc
    };
    let field_factor = if frame_mbs_only { 1 } else { 2 };
    let (crop_x, crop_y) = match chroma_array_type {
        0 => (1, field_factor),
        1 => (2, 2 * field_factor),
        2 => (2, field_factor),
        _ => (1, field_factor),
    };
    let width = (width_mbs * 16)
        .checked_sub(crop_x * (cl + cr))
        .ok_or_else(|| Error::Container("SPS: cropping exceeds the coded width".into()))?;
    let height = (field_factor * height_map_units * 16)
        .checked_sub(crop_y * (ct + cb))
        .ok_or_else(|| Error::Container("SPS: cropping exceeds the coded height".into()))?;

    let vui = if b.flag()? {
        Some(parse_vui(&mut b)?)
    } else {
        None
    };
    Ok(Sps {
        profile_idc,
        level_idc,
        chroma_format_idc,
        bit_depth_luma,
        bit_depth_chroma,
        width,
        height,
        frame_mbs_only,
        vui,
    })
}

fn parse_vui(b: &mut Bits) -> Result<Vui> {
    let mut v = Vui::default();
    if b.flag()? {
        let idc = b.bits(8)?;
        v.sample_aspect = match idc {
            255 => Some((b.bits(16)?, b.bits(16)?)),
            // Table E-1.
            1 => Some((1, 1)),
            2 => Some((12, 11)),
            3 => Some((10, 11)),
            4 => Some((16, 11)),
            5 => Some((40, 33)),
            6 => Some((24, 11)),
            7 => Some((20, 11)),
            8 => Some((32, 11)),
            9 => Some((80, 33)),
            10 => Some((18, 11)),
            11 => Some((15, 11)),
            12 => Some((64, 33)),
            13 => Some((160, 99)),
            14 => Some((4, 3)),
            15 => Some((3, 2)),
            16 => Some((2, 1)),
            _ => None,
        };
    }
    if b.flag()? {
        let _overscan_appropriate = b.flag()?;
    }
    v.video_signal_type_present = b.flag()?;
    if v.video_signal_type_present {
        let _video_format = b.bits(3)?;
        v.full_range = b.flag()?;
        v.colour_description_present = b.flag()?;
        if v.colour_description_present {
            v.colour_primaries = b.bits(8)? as u8;
            v.transfer_characteristics = b.bits(8)? as u8;
            v.matrix_coefficients = b.bits(8)? as u8;
        }
    }
    if b.flag()? {
        let _ = b.ue()?;
        let _ = b.ue()?;
    }
    if b.flag()? {
        let units = b.bits(32)?;
        let scale = b.bits(32)?;
        let _fixed = b.flag()?;
        v.timing = Some((units, scale));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a bitstream from `(value, width)` fields and exp-Golomb codes.
    struct Writer {
        bits: Vec<u8>,
    }
    impl Writer {
        fn new() -> Self {
            Writer { bits: Vec::new() }
        }
        fn u(&mut self, v: u32, n: u32) -> &mut Self {
            for i in (0..n).rev() {
                self.bits.push(((v >> i) & 1) as u8);
            }
            self
        }
        fn ue(&mut self, v: u32) -> &mut Self {
            let x = v + 1;
            let len = 32 - x.leading_zeros();
            self.u(0, len - 1).u(x, len)
        }
        fn bytes(&self) -> Vec<u8> {
            let mut out = vec![0x67];
            for chunk in self.bits.chunks(8) {
                let mut b = 0u8;
                for (i, bit) in chunk.iter().enumerate() {
                    b |= bit << (7 - i);
                }
                out.push(b);
            }
            out
        }
    }

    fn baseline_1080p(vui: impl Fn(&mut Writer)) -> Vec<u8> {
        let mut w = Writer::new();
        w.u(66, 8).u(0, 8).u(40, 8).ue(0); // Baseline, level 4.0, sps id 0
        w.ue(0).ue(0).ue(0); // log2_max_frame_num, poc type 0, lsb
        w.ue(1).u(0, 1); // refs, gaps
        w.ue(119).ue(67); // 120 x 68 macroblocks = 1920 x 1088
        w.u(1, 1).u(1, 1); // frame_mbs_only, direct_8x8
        w.u(1, 1).ue(0).ue(0).ue(0).ue(4); // crop bottom 4 * 2 = 8 → 1080
        vui(&mut w);
        w.u(1, 1); // rbsp stop bit
        w.bytes()
    }

    #[test]
    fn size_cropping_and_a_declared_bt709_limited_signal() {
        let nal = baseline_1080p(|w| {
            w.u(1, 1); // vui present
            w.u(0, 1).u(0, 1); // no aspect, no overscan
            w.u(1, 1).u(5, 3).u(0, 1).u(1, 1).u(1, 8).u(1, 8).u(1, 8); // limited, 709/709/709
            w.u(0, 1).u(0, 1); // no chroma loc, no timing
        });
        let sps = parse(&nal).unwrap();
        assert_eq!((sps.width, sps.height), (1920, 1080));
        let c = sps.color();
        assert_eq!(
            (c.primaries, c.transfer, c.matrix, c.full_range),
            (Some(1), Some(1), Some(1), Some(false))
        );
    }

    #[test]
    fn unspecified_code_points_are_absent_not_assumed() {
        let nal = baseline_1080p(|w| {
            w.u(1, 1).u(0, 1).u(0, 1);
            w.u(1, 1).u(5, 3).u(1, 1).u(1, 1).u(2, 8).u(2, 8).u(2, 8); // full range, unspecified
            w.u(0, 1).u(0, 1);
        });
        let c = parse(&nal).unwrap().color();
        assert_eq!((c.primaries, c.transfer, c.matrix), (None, None, None));
        assert_eq!(c.full_range, Some(true));
    }

    #[test]
    fn no_vui_means_limited_range_and_nothing_else() {
        let nal = baseline_1080p(|w| {
            w.u(0, 1);
        });
        let c = parse(&nal).unwrap().color();
        assert_eq!(
            c,
            ColorMetadata {
                full_range: Some(false),
                ..Default::default()
            }
        );
    }

    #[test]
    fn emulation_prevention_is_removed_before_reading() {
        assert_eq!(rbsp(&[0, 0, 3, 1, 0, 0, 3, 0]), vec![0, 0, 1, 0, 0, 0]);
    }
}
