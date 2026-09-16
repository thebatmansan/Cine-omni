//! OpenEXR files: reading, writing, and the windows they carry.
//!
//! EXR is the format a compositor lives in, and the two things it has that
//! simpler formats do not are exactly the two this crate already models:
//!
//! * a **data window**, the rectangle pixels actually exist in, which may sit
//!   anywhere on the canvas and may be larger or smaller than the frame. It is
//!   [`Image`]'s `x`/`y` and size, so a plate read from disk keeps the window
//!   its renderer wrote and a comp written back keeps the window it grew to;
//! * a **display window**, the frame those pixels are composed against, which
//!   is carried separately and never confused with the data.
//!
//! Layers (AOVs) are addressed by name. Reading asks for one; [`probe`] says
//! what a file holds without decoding it.
//!
//! Float samples are read as f32 whatever the file stores (f16, f32 or u32);
//! writing is f32, which is lossless for anything this compositor computed.
//! Deep EXR is not read here — `DeepImage` is a different model and reading a
//! deep file as flat would silently discard its samples.

use crate::error::{Error, Result};
use crate::image::{ChannelSet, Image};
use exr::prelude as x;
use exr::prelude::{ReadChannels, ReadLayers, WritableImage};
use std::path::Path;

/// What a file holds, without decoding its pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct ExrInfo {
    /// The frame the layers are composed against: `(x, y, width, height)`.
    pub display_window: (i64, i64, usize, usize),
    pub layers: Vec<ExrLayerInfo>,
}

/// One layer of a file.
#[derive(Clone, Debug, PartialEq)]
pub struct ExrLayerInfo {
    /// `None` for the unnamed (default) layer.
    pub name: Option<String>,
    /// The layer's data window: `(x, y, width, height)`.
    pub data_window: (i64, i64, usize, usize),
    /// Channel names as the file spells them, in file order.
    pub channels: Vec<String>,
    /// True when the layer carries deep samples, which this reader refuses.
    pub deep: bool,
}

impl ExrLayerInfo {
    /// Whether the channels can be read as colour (R/G/B, or a single Y).
    pub fn is_colour(&self) -> bool {
        let has = |n: &str| self.channels.iter().any(|c| c == n);
        (has("R") && has("G") && has("B")) || has("Y")
    }
}

fn io(path: &Path, e: impl std::fmt::Display) -> Error {
    Error::MissingSource(format!("{}: {e}", path.display()))
}

fn bounds(b: x::IntegerBounds) -> (i64, i64, usize, usize) {
    (
        b.position.x() as i64,
        b.position.y() as i64,
        b.size.width(),
        b.size.height(),
    )
}

/// What an EXR file holds: its display window, and each layer's window and
/// channels. Reads the header only.
pub fn probe(path: &Path) -> Result<ExrInfo> {
    let meta = x::MetaData::read_from_file(path, false).map_err(|e| io(path, e))?;
    let display_window = meta
        .headers
        .first()
        .map(|h| bounds(h.shared_attributes.display_window))
        .unwrap_or((0, 0, 0, 0));
    let layers = meta
        .headers
        .iter()
        .map(|h| ExrLayerInfo {
            name: h.own_attributes.layer_name.as_ref().map(|t| t.to_string()),
            data_window: bounds(h.data_window()),
            channels: h.channels.list.iter().map(|c| c.name.to_string()).collect(),
            deep: h.deep,
        })
        .collect();
    Ok(ExrInfo {
        display_window,
        layers,
    })
}

/// Read one layer as RGBA, placed on the canvas at its data window.
///
/// `layer` names the layer (an AOV); `None` takes the first that carries
/// colour. A layer without alpha is opaque, which is what an EXR without an
/// `A` channel means.
pub fn read(path: &Path, layer: Option<&str>) -> Result<Image> {
    let info = probe(path)?;
    if info.layers.iter().all(|l| l.deep) && !info.layers.is_empty() {
        return Err(Error::MissingSource(format!(
            "{}: every layer is deep; read it as a deep image, not as flat pixels",
            path.display()
        )));
    }
    let image = x::read()
        .no_deep_data()
        .largest_resolution_level()
        .all_channels()
        .all_layers()
        .all_attributes()
        .from_file(path)
        .map_err(|e| io(path, e))?;

    let wanted = image
        .layer_data
        .iter()
        .find(|l| match layer {
            Some(name) => l
                .attributes
                .layer_name
                .as_ref()
                .map(|t| t.to_string().as_str() == name)
                .unwrap_or(false),
            None => l
                .channel_data
                .list
                .iter()
                .any(|c| c.name.to_string() == "R"),
        })
        .ok_or_else(|| {
            let names: Vec<String> = image
                .layer_data
                .iter()
                .map(|l| {
                    l.attributes
                        .layer_name
                        .as_ref()
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "(unnamed)".into())
                })
                .collect();
            Error::MissingSource(match layer {
                Some(name) => format!(
                    "{}: no layer `{name}`; it has {}",
                    path.display(),
                    names.join(", ")
                ),
                None => format!("{}: no layer with colour channels", path.display()),
            })
        })?;

    let (w, h) = (wanted.size.width(), wanted.size.height());
    let (px, py) = (
        wanted.attributes.layer_position.x() as i64,
        wanted.attributes.layer_position.y() as i64,
    );
    let mut out = Image::new(w, h, ChannelSet::Rgba).placed(px, py);
    let mut seen_alpha = false;
    for channel in wanted.channel_data.list.iter() {
        let slot = match channel.name.to_string().as_str() {
            "R" => 0,
            "G" => 1,
            "B" => 2,
            "A" => 3,
            // A luminance-only layer is grey, not a red one.
            "Y" if !wanted
                .channel_data
                .list
                .iter()
                .any(|c| c.name.to_string() == "R") =>
            {
                4
            }
            _ => continue,
        };
        if slot == 3 {
            seen_alpha = true;
        }
        for (i, sample) in channel.sample_data.values_as_f32().enumerate() {
            if i >= w * h {
                break;
            }
            let (x, y) = (i % w, i / w);
            let mut px = out.get_rgba(x, y);
            if slot == 4 {
                px[0] = sample;
                px[1] = sample;
                px[2] = sample;
            } else {
                px[slot] = sample;
            }
            out.set_rgba(x, y, px);
        }
    }
    if !seen_alpha {
        for y in 0..h {
            for x in 0..w {
                let mut px = out.get_rgba(x, y);
                px[3] = 1.0;
                out.set_rgba(x, y, px);
            }
        }
    }
    Ok(out)
}

/// Write an image as a single-layer RGBA EXR.
///
/// The data window is the image's own window, so what was computed is what is
/// stored — including pixels outside the frame. `display_window` is the frame;
/// `None` uses the image's window, which is what a renderer means when it has
/// no separate frame in mind.
pub fn write(
    path: &Path,
    image: &Image,
    display_window: Option<(i64, i64, usize, usize)>,
) -> Result<()> {
    let rgba = image.to_rgba();
    let (dx, dy, dw, dh) = display_window.unwrap_or_else(|| rgba.window());
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| io(path, e))?;
        }
    }
    let channels = x::SpecificChannels::rgba(|position: x::Vec2<usize>| {
        let p = rgba.get_rgba(position.x(), position.y());
        (p[0], p[1], p[2], p[3])
    });
    let layer = x::Layer::new(
        (rgba.width, rgba.height),
        x::LayerAttributes {
            layer_position: x::Vec2(rgba.x as i32, rgba.y as i32),
            ..x::LayerAttributes::default()
        },
        x::Encoding::FAST_LOSSLESS,
        channels,
    );
    let mut written = x::Image::from_layer(layer);
    written.attributes.display_window = x::IntegerBounds::new((dx as i32, dy as i32), (dw, dh));
    written.write().to_file(path).map_err(|e| io(path, e))?;
    Ok(())
}
