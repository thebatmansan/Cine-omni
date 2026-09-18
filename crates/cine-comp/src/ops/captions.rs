//! Burning captions into pictures.
//!
//! Open captions are part of the picture: a festival screener, a social cut,
//! a check copy for a translator. The layout is the one the IMSC document this
//! project writes declares, so a burned-in caption and a delivered caption file
//! put the same words in the same place: a 32 × 15 cell grid, white text on a
//! black box, inside the 10% safe area, bottom rows or top rows.
//!
//! Text is rasterised with `ab_glyph`, already in the build for the window's
//! title bar. **Fonts come only from the system font directories**, never from
//! a project: the font parser underneath is unmaintained (see `deny.toml`), and
//! a font file shipped inside a project would be data an outsider controls. A
//! machine with none of the fonts looked for refuses to burn in, naming the
//! paths it tried.
//!
//! Compositing is in whatever encoding the picture is in, which for burn-in is
//! the display encoding the deliverable is written in: white is 1.0.

use crate::error::{Error, Result};
use crate::image::Image;
use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use std::path::{Path, PathBuf};

/// Where a caption sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Bottom,
    Top,
}

/// System fonts tried, in order: sans faces common on the systems this runs on.
pub const SYSTEM_FONTS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "C:\\Windows\\Fonts\\arial.ttf",
];

/// A font loaded from the system for burning in.
pub struct CaptionFont {
    font: FontVec,
    pub path: PathBuf,
}

impl CaptionFont {
    /// The first system font that exists and parses.
    pub fn system() -> Result<CaptionFont> {
        for candidate in SYSTEM_FONTS {
            let path = Path::new(candidate);
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(font) = FontVec::try_from_vec(bytes) {
                    return Ok(CaptionFont {
                        font,
                        path: path.to_path_buf(),
                    });
                }
            }
        }
        Err(Error::Refused(format!(
            "no system font to burn captions in with; looked for {}",
            SYSTEM_FONTS.join(", ")
        )))
    }
}

/// Where a line landed, for tests and for reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineBox {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// Burn `lines` into `image`. Returns where each line's box was drawn.
///
/// Refused, not shrunk, when a line is wider than the safe area: a caption that
/// silently changed size would be a different caption from the one delivered as
/// a file.
pub fn burn_in(
    image: &mut Image,
    lines: &[String],
    placement: Placement,
    font: &CaptionFont,
) -> Result<Vec<LineBox>> {
    let (w, h) = (image.width, image.height);
    let lines: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() || w == 0 || h == 0 {
        return Ok(Vec::new());
    }
    // A row of the 15-row grid; text is four fifths of a row tall.
    let row = h as f32 / 15.0;
    let scale = PxScale::from(row * 0.8);
    let scaled = font.font.as_scaled(scale);
    let pad = (row * 0.25).round() as usize;
    let (safe_left, safe_right) = (w as f32 * 0.1, w as f32 * 0.9);
    let (safe_top, safe_bottom) = (h as f32 * 0.1, h as f32 * 0.9);

    let measure = |text: &str| -> f32 {
        let mut width = 0.0;
        let mut previous = None;
        for c in text.chars() {
            let id = scaled.glyph_id(c);
            if let Some(p) = previous {
                width += scaled.kern(p, id);
            }
            width += scaled.h_advance(id);
            previous = Some(id);
        }
        width
    };

    let count = lines.len();
    let first_top = match placement {
        Placement::Top => safe_top,
        Placement::Bottom => safe_bottom - row * count as f32,
    };
    // Every line is measured before any is drawn, so a refusal leaves the
    // picture exactly as it was.
    let widths: Vec<f32> = lines
        .iter()
        .map(|line| measure(line) + 2.0 * pad as f32)
        .collect();
    for (line, box_width) in lines.iter().zip(&widths) {
        if *box_width > safe_right - safe_left {
            return Err(Error::Refused(format!(
                "the caption line \"{line}\" is {:.0} pixels wide and the safe area is {:.0}",
                box_width,
                safe_right - safe_left
            )));
        }
    }
    let mut boxes = Vec::with_capacity(count);
    for (i, (line, box_width)) in lines.iter().zip(widths).enumerate() {
        let top = first_top + row * i as f32;
        let left = (w as f32 - box_width) / 2.0;
        let lb = LineBox {
            x: left.round().max(0.0) as usize,
            y: top.round().max(0.0) as usize,
            width: box_width.round() as usize,
            height: row.round() as usize,
        };
        // The box: opaque black under the text.
        for y in lb.y..(lb.y + lb.height).min(h) {
            for x in lb.x..(lb.x + lb.width).min(w) {
                let a = image.get_rgba(x, y)[3];
                image.set_rgba(x, y, [0.0, 0.0, 0.0, a.max(1.0)]);
            }
        }
        // The text: white, by coverage, baseline so the row is centred.
        let baseline = top + (row + scaled.ascent() + scaled.descent()) / 2.0;
        let mut caret = left + pad as f32;
        let mut previous = None;
        for c in line.chars() {
            let id = scaled.glyph_id(c);
            if let Some(p) = previous {
                caret += scaled.kern(p, id);
            }
            let glyph = id.with_scale_and_position(scale, point(caret, baseline));
            caret += scaled.h_advance(id);
            previous = Some(id);
            let Some(outline) = font.font.outline_glyph(glyph) else {
                continue; // a space
            };
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let x = bounds.min.x as i64 + i64::from(gx);
                let y = bounds.min.y as i64 + i64::from(gy);
                if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
                    return;
                }
                let (x, y) = (x as usize, y as usize);
                let under = image.get_rgba(x, y);
                let mix = |v: f32| v + (1.0 - v) * coverage;
                image.set_rgba(
                    x,
                    y,
                    [mix(under[0]), mix(under[1]), mix(under[2]), under[3]],
                );
            });
        }
        boxes.push(lb);
    }
    Ok(boxes)
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! font_or_skip {
        () => {
            match CaptionFont::system() {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("SKIP: {e}");
                    return;
                }
            }
        };
    }

    fn grey(w: usize, h: usize) -> Image {
        Image::solid(w, h, [0.5, 0.5, 0.5, 1.0])
    }

    fn whites_in(image: &Image, b: &LineBox) -> usize {
        let mut n = 0;
        for y in b.y..b.y + b.height {
            for x in b.x..b.x + b.width {
                if image.get_rgba(x, y)[0] > 0.9 {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn a_bottom_caption_sits_in_the_lower_safe_area_white_on_black() {
        let font = font_or_skip!();
        let mut image = grey(1920, 1080);
        let boxes = burn_in(
            &mut image,
            &["We ride at dawn.".into()],
            Placement::Bottom,
            &font,
        )
        .unwrap();
        assert_eq!(boxes.len(), 1);
        let b = boxes[0];
        assert!(
            b.y + b.height <= 972 && b.y > 850,
            "inside the lower safe area: {b:?}"
        );
        let centre = b.x + b.width / 2;
        assert!((centre as i64 - 960).abs() <= 1, "centred: {b:?}");
        assert!(
            whites_in(&image, &b) > 1000,
            "there is white text in the box"
        );
        // The box's corner is black, and outside the box the picture is untouched.
        assert_eq!(image.get_rgba(b.x + 1, b.y + 1)[..3], [0.0, 0.0, 0.0]);
        assert_eq!(image.get_rgba(10, 10), [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(
            image.get_rgba(b.x.saturating_sub(2), b.y + 5),
            [0.5, 0.5, 0.5, 1.0]
        );
    }

    #[test]
    fn two_lines_stack_upward_from_the_bottom_and_a_top_caption_is_at_the_top() {
        let font = font_or_skip!();
        let mut image = grey(1280, 720);
        let boxes = burn_in(
            &mut image,
            &["Not before.".into(), "Not ever.".into()],
            Placement::Bottom,
            &font,
        )
        .unwrap();
        assert_eq!(boxes.len(), 2);
        assert!(
            boxes[0].y < boxes[1].y,
            "the first line is above the second"
        );
        assert!(boxes[1].y + boxes[1].height <= 648);

        let mut image = grey(1280, 720);
        let top = burn_in(
            &mut image,
            &["HARBOUR OFFICE".into()],
            Placement::Top,
            &font,
        )
        .unwrap();
        assert!(top[0].y >= 72 && top[0].y < 90, "{:?}", top[0]);
    }

    #[test]
    fn a_line_too_wide_for_the_safe_area_is_refused_not_shrunk() {
        let font = font_or_skip!();
        let mut image = grey(640, 360);
        let long = "This caption line is far too long to fit inside the safe area of the picture"
            .to_string();
        let e = burn_in(&mut image, &[long], Placement::Bottom, &font).unwrap_err();
        assert!(e.to_string().contains("safe area"), "{e}");
        assert_eq!(
            image.get_rgba(320, 300),
            [0.5, 0.5, 0.5, 1.0],
            "nothing was drawn"
        );
    }

    /// Only the second line is too wide: the first must not have been drawn.
    #[test]
    fn a_refusal_leaves_the_picture_untouched_even_after_a_line_that_fit() {
        let font = font_or_skip!();
        let mut image = grey(640, 360);
        let before = image.data.clone();
        let e = burn_in(
            &mut image,
            &[
                "Short.".into(),
                "and this second line is very much too wide to fit the safe area of a small picture".into(),
            ],
            Placement::Bottom,
            &font,
        )
        .unwrap_err();
        assert!(e.to_string().contains("safe area"), "{e}");
        assert_eq!(image.data, before);
    }

    #[test]
    fn text_scales_with_the_picture() {
        let font = font_or_skip!();
        let mut small = grey(1280, 720);
        let mut large = grey(3840, 2160);
        let s = burn_in(&mut small, &["Scale".into()], Placement::Bottom, &font).unwrap()[0];
        let l = burn_in(&mut large, &["Scale".into()], Placement::Bottom, &font).unwrap()[0];
        let ratio = l.width as f64 / s.width as f64;
        assert!(
            (ratio - 3.0).abs() < 0.1,
            "three times the picture, three times the text: {ratio:.3}"
        );
    }
}
