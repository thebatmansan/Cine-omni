//! A small PDF writer: enough for a call sheet, and nothing more.
//!
//! A call sheet is printed and taped to a wall, so it has to be a PDF. Writing
//! one by hand is a few hundred lines because the parts a document like this
//! needs — pages, a content stream, the standard fonts, an xref table — are
//! the simple parts of the format. The alternative was a dependency that
//! carries font embedding, image codecs and colour management this does not
//! use.
//!
//! What it does: US Letter or A4 pages, Helvetica and Helvetica-Bold at any
//! size, lines and filled rectangles, and text placed by coordinate.
//! Everything is in points, with the origin at the bottom left, as PDF has it.
//!
//! What it does not: no embedded fonts (so no character outside WinAnsi, and
//! [`Page::text`] says so by substituting `?`), no images, no transparency, no
//! compression, no links, no tagging, no `/Encrypt`. Files are written
//! uncompressed, which for a one-page call sheet is a few kilobytes and makes
//! the output diffable.

use std::fmt::Write as _;

/// A page size in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

impl PageSize {
    /// 8.5 × 11 inches.
    pub const LETTER: PageSize = PageSize {
        width: 612.0,
        height: 792.0,
    };
    /// 210 × 297 mm.
    pub const A4: PageSize = PageSize {
        width: 595.28,
        height: 841.89,
    };
}

/// One of the standard fonts every PDF reader has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    Helvetica,
    HelveticaBold,
}

impl Font {
    fn resource(&self) -> &'static str {
        match self {
            Font::Helvetica => "F1",
            Font::HelveticaBold => "F2",
        }
    }

    fn base(&self) -> &'static str {
        match self {
            Font::Helvetica => "Helvetica",
            Font::HelveticaBold => "Helvetica-Bold",
        }
    }

    /// Width of `text` at `size`, in points.
    ///
    /// From the Adobe Font Metrics for the standard 14: enough to centre a
    /// title and to know when a line will not fit, which is all the layout
    /// here needs.
    pub fn width_of(&self, text: &str, size: f32) -> f32 {
        let per_mille: f32 = text.chars().map(|c| f32::from(self.char_width(c))).sum();
        per_mille * size / 1000.0
    }

    fn char_width(&self, c: char) -> u16 {
        // Helvetica and Helvetica-Bold differ; both are tabulated by AFM. The
        // common cases are spelled out and everything else takes the average,
        // which is what a monospaced fallback would do anyway.
        let bold = *self == Font::HelveticaBold;
        match c {
            ' ' => 278,
            '.' | ',' | ':' | ';' | '\'' | '|' => 278,
            'i' | 'j' | 'l' | 'I' | '!' => 222 + u16::from(bold) * 56,
            'f' | 't' | 'r' => 278 + u16::from(bold) * 56,
            'm' | 'M' | 'W' | 'w' => 833 + u16::from(bold) * 56,
            'A'..='Z' => 667 + u16::from(bold) * 55,
            '0'..='9' => 556,
            _ => 556 + u16::from(bold) * 56,
        }
    }
}

/// A page being drawn on.
pub struct Page {
    size: PageSize,
    content: String,
}

impl Page {
    fn new(size: PageSize) -> Page {
        Page {
            size,
            content: String::new(),
        }
    }

    pub fn size(&self) -> PageSize {
        self.size
    }

    /// Draw text with its baseline at `(x, y)`, measured from the bottom left.
    pub fn text(&mut self, x: f32, y: f32, size: f32, font: Font, text: &str) {
        let _ = writeln!(
            self.content,
            "BT /{} {size} Tf 1 0 0 1 {x:.2} {y:.2} Tm ({}) Tj ET",
            font.resource(),
            escape(text)
        );
    }

    /// Draw text ending at `x` rather than starting there.
    pub fn text_right(&mut self, x: f32, y: f32, size: f32, font: Font, text: &str) {
        self.text(x - font.width_of(text, size), y, size, font, text);
    }

    /// A line of `width` points between two coordinates.
    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, grey: f32) {
        let _ = writeln!(
            self.content,
            "{g:.3} G {width:.2} w {x1:.2} {y1:.2} m {x2:.2} {y2:.2} l S",
            g = grey.clamp(0.0, 1.0)
        );
    }

    /// A filled rectangle, for a banner or a shaded row.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, grey: f32) {
        let _ = writeln!(
            self.content,
            "{g:.3} g {x:.2} {y:.2} {w:.2} {h:.2} re f",
            g = grey.clamp(0.0, 1.0)
        );
    }
}

/// A document being built.
pub struct Pdf {
    pages: Vec<Page>,
    title: String,
}

impl Pdf {
    pub fn new(title: impl Into<String>) -> Pdf {
        Pdf {
            pages: Vec::new(),
            title: title.into(),
        }
    }

    /// Start a page and hand it back for drawing.
    pub fn page(&mut self, size: PageSize) -> &mut Page {
        self.pages.push(Page::new(size));
        self.pages.last_mut().expect("just pushed")
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Serialise the document.
    pub fn finish(&self) -> Vec<u8> {
        // Object numbering: 1 catalog, 2 pages, 3 info, 4 and 5 the fonts,
        // then a pair (page, content) for each page.
        let mut objects: Vec<String> = Vec::new();
        let first_page_object = 6;
        let page_ids: Vec<usize> = (0..self.pages.len())
            .map(|i| first_page_object + i * 2)
            .collect();

        objects.push("<< /Type /Catalog /Pages 2 0 R >>".to_string());
        objects.push(format!(
            "<< /Type /Pages /Count {} /Kids [{}] >>",
            self.pages.len(),
            page_ids
                .iter()
                .map(|id| format!("{id} 0 R"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
        objects.push(format!(
            "<< /Title ({}) /Producer (CINE-OMNI) >>",
            escape(&self.title)
        ));
        for font in [Font::Helvetica, Font::HelveticaBold] {
            objects.push(format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /{} /Encoding /WinAnsiEncoding >>",
                font.base()
            ));
        }
        for (i, page) in self.pages.iter().enumerate() {
            let content_id = first_page_object + i * 2 + 1;
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] \
                 /Resources << /Font << /F1 4 0 R /F2 5 0 R >> >> /Contents {content_id} 0 R >>",
                page.size.width, page.size.height
            ));
            objects.push(format!(
                "<< /Length {} >>\nstream\n{}endstream",
                page.content.len(),
                page.content
            ));
        }

        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::with_capacity(objects.len());
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            let _ = write!(out, "{} 0 obj\n{body}\nendobj\n", i + 1);
        }
        let xref_at = out.len();
        let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
        for offset in &offsets {
            let _ = writeln!(out, "{offset:010} 00000 n ");
        }
        let _ = write!(
            out,
            "trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        );
        out.into_bytes()
    }
}

/// PDF string escaping, and the honest treatment of what the standard fonts
/// cannot show: a character outside WinAnsi becomes `?` rather than silently
/// becoming a different letter.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\n' | '\r' | '\t' => out.push(' '),
            c if (c as u32) < 32 => out.push(' '),
            c if (c as u32) < 127 => out.push(c),
            // WinAnsi covers Latin-1 with a few substitutions; anything else
            // has no glyph in a standard font.
            c if (c as u32) < 256 => {
                let _ = write!(out, "\\{:03o}", c as u32);
            }
            _ => out.push('?'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_has_the_structure_a_reader_expects() {
        let mut pdf = Pdf::new("Test");
        let page = pdf.page(PageSize::LETTER);
        page.text(72.0, 700.0, 12.0, Font::Helvetica, "HELLO");
        let bytes = pdf.finish();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.contains("/Type /Catalog"));
        assert!(text.contains("/MediaBox [0 0 612.00 792.00]"));
        assert!(text.ends_with("%%EOF\n"));
        // The xref offsets must actually point at their objects.
        let xref_at: usize = text
            .rsplit("startxref\n")
            .next()
            .and_then(|t| t.split('\n').next())
            .and_then(|n| n.trim().parse().ok())
            .expect("startxref");
        assert!(
            text[xref_at..].starts_with("xref"),
            "xref is where the trailer says"
        );
        for (i, line) in text[xref_at..]
            .lines()
            .skip(3) // xref, range, the free entry
            .take(2)
            .enumerate()
        {
            let offset: usize = line[..10].parse().expect("an offset");
            assert!(
                text[offset..].starts_with(&format!("{} 0 obj", i + 1)),
                "object {} is at the offset the table gives",
                i + 1
            );
        }
    }

    #[test]
    fn text_is_escaped_and_unprintable_characters_are_not_invented() {
        let mut pdf = Pdf::new("x");
        let page = pdf.page(PageSize::A4);
        page.text(0.0, 0.0, 10.0, Font::Helvetica, "a (b) \\ c — 日本");
        let text = String::from_utf8_lossy(&pdf.finish()).to_string();
        assert!(text.contains("a \\(b\\) \\\\ c ? ??"), "{text}");
    }

    #[test]
    fn width_is_close_enough_to_centre_a_title() {
        let w = Font::HelveticaBold.width_of("CALL SHEET", 18.0);
        assert!(w > 80.0 && w < 140.0, "{w}");
        assert!(
            Font::HelveticaBold.width_of("MMM", 12.0) > Font::Helvetica.width_of("iii", 12.0),
            "wide letters are wider than narrow ones"
        );
    }
}
