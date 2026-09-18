//! Writing QuickTime movies: H.264 picture and uncompressed sound.
//!
//! The `mp4` crate's writer can carry only AAC sound, and there is no
//! pure-Rust AAC encoder this build can use. Uncompressed PCM in a QuickTime
//! movie is the ordinary professional interchange for sound that must not be
//! altered, so that is what this writes: H.264 from [`crate::encode`]'s
//! encoder, and 24-bit little-endian linear PCM declared by a version 2 sound
//! description (`lpcm`) with an explicit channel layout.
//!
//! ## Exact time
//!
//! The picture track's timescale is the frame rate's numerator and every
//! sample lasts its denominator. The sound track's timescale is its sample
//! rate and every sample is one sample frame. Sound is written frame by frame
//! with the picture: the sound for picture `k` is the samples from
//! `floor(k·den·rate/num)` up to `floor((k+1)·den·rate/num)`, so the sound
//! never drifts from the picture by more than one sample, at any frame rate,
//! and the two tracks end together. Both tracks carry an edit list that starts
//! them at movie time zero.
//!
//! ## Layout
//!
//! `ftyp`, then one 64-bit `mdat` whose size is patched when the movie is
//! finished, then `moov`. Each picture is a chunk followed by a chunk of its
//! sound, so a reader streaming the file never has to seek far. Only the
//! sample tables are held in memory, not the media.
//!
//! ## Colour
//!
//! The samples are declared limited-range BT.709 matrix, square pixels
//! (`pasp`), and whatever primaries and transfer the caller says they are
//! ([`ColourTags`]) — including H.273's "unspecified" when it does not know.
//! Nothing here converts colour; it only declares what it was given.
//!
//! What this is not: other codecs, other bit depths of sound, more than eight
//! channels, timecode tracks, or fragmented movies.

use crate::encode::{H264Settings, YuvFrame};
use crate::error::{Error, Result};
use rusty_h264::{Encoder, EncoderConfig};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use ucg_core::Rate;

/// Uncompressed sound in the movie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// H.273 code points the picture is declared with. The matrix is always BT.709
/// (1): that is what the samples are encoded with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColourTags {
    pub primaries: u16,
    pub transfer: u16,
}

impl ColourTags {
    /// BT.709 primaries and transfer.
    pub const BT709: ColourTags = ColourTags {
        primaries: 1,
        transfer: 1,
    };
    /// BT.709 primaries, IEC 61966-2-1 (sRGB) transfer.
    pub const SRGB: ColourTags = ColourTags {
        primaries: 1,
        transfer: 13,
    };
    /// Nothing is known about primaries or transfer, and the file says so.
    pub const UNSPECIFIED: ColourTags = ColourTags {
        primaries: 2,
        transfer: 2,
    };
}

/// How a movie is written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovSettings {
    pub h264: H264Settings,
    pub sound: Option<SoundFormat>,
    pub colour: ColourTags,
    /// The picture number, in the sequence being written, of the movie's first
    /// picture. Sound is divided among pictures by absolute sequence position,
    /// so a movie of frames 100–199 holds exactly the samples a movie of the
    /// whole sequence holds for those frames.
    pub first_picture: u64,
}

/// Bits per sound sample written.
pub const SOUND_BITS: u32 = 24;

/// What a movie holds once finished.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MovReport {
    pub frames: u64,
    pub keyframes: u64,
    pub video_bytes: u64,
    pub sound_frames: u64,
    /// Sound samples beyond full scale, clipped to it.
    pub sound_clipped: u64,
    pub file_bytes: u64,
}

struct Chunk {
    offset: u64,
    samples: u32,
}

/// A QuickTime movie being written.
pub struct MovWriter {
    out: BufWriter<File>,
    pos: u64,
    mdat_start: u64,
    width: usize,
    height: usize,
    rate: Rate,
    sound: Option<SoundFormat>,
    colour: ColourTags,
    first_picture: u64,
    encoder: Encoder,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    video_sizes: Vec<u32>,
    video_chunks: Vec<Chunk>,
    sync: Vec<u32>,
    sound_chunks: Vec<Chunk>,
    report: MovReport,
}

impl MovWriter {
    /// Start a movie of `width × height` pictures at `rate`, with sound when
    /// `sound` is given.
    pub fn create(
        path: &Path,
        width: usize,
        height: usize,
        rate: Rate,
        settings: MovSettings,
    ) -> Result<MovWriter> {
        let sound = settings.sound;
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(Error::Unsupported(format!(
                "{width}x{height}: 4:2:0 needs an even, non-zero width and height"
            )));
        }
        if width > 0xFFFF || height > 0xFFFF {
            return Err(Error::Unsupported(format!(
                "{width}x{height} is larger than a sample description can declare"
            )));
        }
        if let Some(s) = sound {
            if s.channels == 0 || s.channels > 8 {
                return Err(Error::Unsupported(format!(
                    "{} sound channels: this writer declares 1 to 8",
                    s.channels
                )));
            }
            if s.sample_rate == 0 {
                return Err(Error::Unsupported("a sound rate of zero".into()));
            }
        }
        let mut cfg = EncoderConfig::baseline(width, height);
        cfg.qp = settings.h264.qp.min(51);
        cfg.gop_size = settings.h264.gop.max(1);
        let encoder = Encoder::new(cfg)
            .map_err(|e| Error::Unsupported(format!("the encoder refused: {e:?}")))?;
        let mut out = BufWriter::new(File::create(path).map_err(Error::Io)?);
        let mut head = Vec::new();
        atom(&mut head, b"ftyp", |b| {
            b.extend_from_slice(b"qt  ");
            b.extend_from_slice(&0x0200u32.to_be_bytes());
            b.extend_from_slice(b"qt  ");
        });
        let mdat_start = head.len() as u64;
        // 64-bit mdat: size 1, then the real size, patched at the end.
        head.extend_from_slice(&1u32.to_be_bytes());
        head.extend_from_slice(b"mdat");
        head.extend_from_slice(&0u64.to_be_bytes());
        out.write_all(&head).map_err(Error::Io)?;
        Ok(MovWriter {
            out,
            pos: head.len() as u64,
            mdat_start,
            width,
            height,
            rate,
            sound,
            colour: settings.colour,
            first_picture: settings.first_picture,
            encoder,
            sps: None,
            pps: None,
            video_sizes: Vec::new(),
            video_chunks: Vec::new(),
            sync: Vec::new(),
            sound_chunks: Vec::new(),
            report: MovReport::default(),
        })
    }

    /// How many sound sample frames belong with picture `k`.
    pub fn sound_frames_for(rate: Rate, sample_rate: u32, k: u64) -> u64 {
        let at = |k: u64| -> u64 {
            (u128::from(k) * u128::from(rate.den()) * u128::from(sample_rate)
                / u128::from(rate.num())) as u64
        };
        at(k + 1) - at(k)
    }

    /// Sound sample frames the next picture must be written with.
    pub fn sound_frames_due(&self) -> u64 {
        match self.sound {
            Some(s) => MovWriter::sound_frames_for(
                self.rate,
                s.sample_rate,
                self.first_picture + self.report.frames,
            ),
            None => 0,
        }
    }

    /// Append one picture and, when the movie has sound, exactly
    /// [`MovWriter::sound_frames_due`] frames of interleaved sound.
    pub fn write_frame(&mut self, picture: &YuvFrame, sound: &[f32]) -> Result<()> {
        if picture.width != self.width || picture.height != self.height {
            return Err(Error::Unsupported(format!(
                "picture {} is {}x{}, the movie is {}x{}",
                self.report.frames, picture.width, picture.height, self.width, self.height
            )));
        }
        let due = self.sound_frames_due();
        let channels = self.sound.map_or(0, |s| usize::from(s.channels));
        if sound.len() != due as usize * channels {
            return Err(Error::Unsupported(format!(
                "picture {} needs {due} sound frames of {channels} channels; {} samples were given",
                self.report.frames,
                sound.len()
            )));
        }

        let unit = self.encoder.encode(picture);
        let nals = crate::encode::nal_units(&unit);
        let mut sample = Vec::with_capacity(unit.len() + 16);
        let mut sync = false;
        for nal in &nals {
            let kind = nal.first().map(|b| b & 0x1F).unwrap_or(0);
            match kind {
                7 => {
                    if self.sps.is_none() {
                        self.sps = Some(nal.to_vec());
                    }
                    continue;
                }
                8 => {
                    if self.pps.is_none() {
                        self.pps = Some(nal.to_vec());
                    }
                    continue;
                }
                9 => continue,
                _ => {}
            }
            sync |= kind == 5;
            sample.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            sample.extend_from_slice(nal);
        }
        if self.sps.is_none() || self.pps.is_none() {
            return Err(Error::Unsupported(
                "the first access unit carries no SPS and PPS".into(),
            ));
        }
        self.video_chunks.push(Chunk {
            offset: self.pos,
            samples: 1,
        });
        self.put(&sample)?;
        self.video_sizes.push(sample.len() as u32);
        if sync {
            self.sync.push(self.report.frames as u32 + 1);
            self.report.keyframes += 1;
        }
        self.report.video_bytes += sample.len() as u64;

        if due > 0 {
            let mut bytes = Vec::with_capacity(sound.len() * 3);
            // Scaled by 2^23, the convention the reader decodes with, so a
            // sample read from a 24-bit file and written again is the same
            // integer; +1.0 itself is the one in-range value that clips.
            for &s in sound {
                let scaled = f64::from(s) * 8_388_608.0;
                if !(-8_388_608.0..=8_388_607.0).contains(&scaled) {
                    self.report.sound_clipped += 1;
                }
                let v = scaled.round().clamp(-8_388_608.0, 8_388_607.0) as i32;
                bytes.extend_from_slice(&v.to_le_bytes()[..3]);
            }
            self.sound_chunks.push(Chunk {
                offset: self.pos,
                samples: due as u32,
            });
            self.put(&bytes)?;
            self.report.sound_frames += due;
        }
        self.report.frames += 1;
        Ok(())
    }

    fn put(&mut self, bytes: &[u8]) -> Result<()> {
        self.out.write_all(bytes).map_err(Error::Io)?;
        self.pos += bytes.len() as u64;
        Ok(())
    }

    /// Write the movie's index and close it.
    pub fn finish(mut self) -> Result<MovReport> {
        if self.report.frames == 0 {
            return Err(Error::Unsupported(
                "a movie needs at least one picture".into(),
            ));
        }
        let mdat_size = self.pos - self.mdat_start;
        let video_duration = self.report.frames * u64::from(self.rate.den());
        let movie_scale = self.rate.num();
        let sound_duration = self.report.sound_frames;
        let moov = {
            let mut m = Vec::new();
            atom(&mut m, b"moov", |b| {
                let next_track = if self.sound.is_some() { 3 } else { 2 };
                full_atom(b, b"mvhd", 1, 0, |b| {
                    b.extend_from_slice(&0u64.to_be_bytes());
                    b.extend_from_slice(&0u64.to_be_bytes());
                    b.extend_from_slice(&movie_scale.to_be_bytes());
                    b.extend_from_slice(&video_duration.to_be_bytes());
                    b.extend_from_slice(&0x0001_0000u32.to_be_bytes());
                    b.extend_from_slice(&0x0100u16.to_be_bytes());
                    b.extend_from_slice(&[0; 10]);
                    matrix(b);
                    b.extend_from_slice(&[0; 24]);
                    b.extend_from_slice(&(next_track as u32).to_be_bytes());
                });
                self.video_trak(b, video_duration);
                if let Some(sound) = self.sound {
                    // The sound track's length in movie time, rounded up: the
                    // edit list must cover every sample.
                    let movie_len = (u128::from(sound_duration) * u128::from(movie_scale))
                        .div_ceil(u128::from(sound.sample_rate))
                        as u64;
                    self.sound_trak(b, sound, sound_duration, movie_len);
                }
            });
            m
        };
        self.put(&moov)?;
        self.out.flush().map_err(Error::Io)?;
        let mut file = self
            .out
            .into_inner()
            .map_err(|e| Error::Io(e.into_error()))?;
        file.seek(SeekFrom::Start(self.mdat_start + 8))
            .map_err(Error::Io)?;
        file.write_all(&mdat_size.to_be_bytes())
            .map_err(Error::Io)?;
        file.sync_all().map_err(Error::Io)?;
        self.report.file_bytes = self.pos;
        Ok(self.report)
    }

    fn video_trak(&self, b: &mut Vec<u8>, duration: u64) {
        let (w, h) = (self.width as u32, self.height as u32);
        atom(b, b"trak", |b| {
            tkhd(b, 1, duration, 0, w, h);
            edts(b, duration);
            atom(b, b"mdia", |b| {
                mdhd(b, self.rate.num(), duration);
                hdlr(b, b"vide", "VideoHandler");
                atom(b, b"minf", |b| {
                    full_atom(b, b"vmhd", 0, 1, |b| {
                        b.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
                    });
                    dinf(b);
                    atom(b, b"stbl", |b| {
                        full_atom(b, b"stsd", 0, 0, |b| {
                            b.extend_from_slice(&1u32.to_be_bytes());
                            atom(b, b"avc1", |b| {
                                b.extend_from_slice(&[0; 6]);
                                b.extend_from_slice(&1u16.to_be_bytes());
                                b.extend_from_slice(&[0; 16]);
                                b.extend_from_slice(&(w as u16).to_be_bytes());
                                b.extend_from_slice(&(h as u16).to_be_bytes());
                                b.extend_from_slice(&0x0048_0000u32.to_be_bytes());
                                b.extend_from_slice(&0x0048_0000u32.to_be_bytes());
                                b.extend_from_slice(&0u32.to_be_bytes());
                                b.extend_from_slice(&1u16.to_be_bytes());
                                let mut name = [0u8; 32];
                                let label = b"CINE-OMNI H.264";
                                name[0] = label.len() as u8;
                                name[1..=label.len()].copy_from_slice(label);
                                b.extend_from_slice(&name);
                                b.extend_from_slice(&0x0018u16.to_be_bytes());
                                b.extend_from_slice(&0xFFFFu16.to_be_bytes());
                                let sps = self.sps.as_deref().unwrap_or_default();
                                let pps = self.pps.as_deref().unwrap_or_default();
                                atom(b, b"avcC", |b| {
                                    b.push(1);
                                    b.push(sps.get(1).copied().unwrap_or(66));
                                    b.push(sps.get(2).copied().unwrap_or(0));
                                    b.push(sps.get(3).copied().unwrap_or(30));
                                    b.push(0xFF);
                                    b.push(0xE1);
                                    b.extend_from_slice(&(sps.len() as u16).to_be_bytes());
                                    b.extend_from_slice(sps);
                                    b.push(1);
                                    b.extend_from_slice(&(pps.len() as u16).to_be_bytes());
                                    b.extend_from_slice(pps);
                                });
                                atom(b, b"colr", |b| {
                                    b.extend_from_slice(b"nclc");
                                    b.extend_from_slice(&self.colour.primaries.to_be_bytes());
                                    b.extend_from_slice(&self.colour.transfer.to_be_bytes());
                                    b.extend_from_slice(&1u16.to_be_bytes());
                                });
                                atom(b, b"pasp", |b| {
                                    b.extend_from_slice(&1u32.to_be_bytes());
                                    b.extend_from_slice(&1u32.to_be_bytes());
                                });
                            });
                        });
                        stts(b, self.report.frames, self.rate.den());
                        full_atom(b, b"stss", 0, 0, |b| {
                            b.extend_from_slice(&(self.sync.len() as u32).to_be_bytes());
                            for s in &self.sync {
                                b.extend_from_slice(&s.to_be_bytes());
                            }
                        });
                        stsc(b, &self.video_chunks);
                        full_atom(b, b"stsz", 0, 0, |b| {
                            b.extend_from_slice(&0u32.to_be_bytes());
                            b.extend_from_slice(&(self.video_sizes.len() as u32).to_be_bytes());
                            for s in &self.video_sizes {
                                b.extend_from_slice(&s.to_be_bytes());
                            }
                        });
                        co64(b, &self.video_chunks);
                    });
                });
            });
        });
    }

    fn sound_trak(&self, b: &mut Vec<u8>, sound: SoundFormat, frames: u64, movie_len: u64) {
        let bytes_per_frame = u32::from(sound.channels) * (SOUND_BITS / 8);
        atom(b, b"trak", |b| {
            tkhd(b, 2, movie_len, 0x0100, 0, 0);
            edts(b, movie_len);
            atom(b, b"mdia", |b| {
                mdhd(b, sound.sample_rate, frames);
                hdlr(b, b"soun", "SoundHandler");
                atom(b, b"minf", |b| {
                    full_atom(b, b"smhd", 0, 0, |b| {
                        b.extend_from_slice(&[0, 0, 0, 0]);
                    });
                    dinf(b);
                    atom(b, b"stbl", |b| {
                        full_atom(b, b"stsd", 0, 0, |b| {
                            b.extend_from_slice(&1u32.to_be_bytes());
                            atom(b, b"lpcm", |b| {
                                b.extend_from_slice(&[0; 6]);
                                b.extend_from_slice(&1u16.to_be_bytes());
                                // Sound sample description, version 2.
                                b.extend_from_slice(&2u16.to_be_bytes());
                                b.extend_from_slice(&0u16.to_be_bytes());
                                b.extend_from_slice(&0u32.to_be_bytes());
                                b.extend_from_slice(&3u16.to_be_bytes());
                                b.extend_from_slice(&16u16.to_be_bytes());
                                b.extend_from_slice(&0xFFFEu16.to_be_bytes());
                                b.extend_from_slice(&0u16.to_be_bytes());
                                b.extend_from_slice(&65536u32.to_be_bytes());
                                b.extend_from_slice(&72u32.to_be_bytes());
                                b.extend_from_slice(
                                    &f64::from(sound.sample_rate).to_bits().to_be_bytes(),
                                );
                                b.extend_from_slice(&u32::from(sound.channels).to_be_bytes());
                                b.extend_from_slice(&0x7F00_0000u32.to_be_bytes());
                                b.extend_from_slice(&SOUND_BITS.to_be_bytes());
                                // Signed integer, packed, little-endian.
                                b.extend_from_slice(&(0x4u32 | 0x8).to_be_bytes());
                                b.extend_from_slice(&bytes_per_frame.to_be_bytes());
                                b.extend_from_slice(&1u32.to_be_bytes());
                                full_atom(b, b"chan", 0, 0, |b| {
                                    b.extend_from_slice(
                                        &channel_layout_tag(sound.channels).to_be_bytes(),
                                    );
                                    b.extend_from_slice(&0u32.to_be_bytes());
                                    b.extend_from_slice(&0u32.to_be_bytes());
                                });
                            });
                        });
                        stts(b, frames, 1);
                        stsc(b, &self.sound_chunks);
                        full_atom(b, b"stsz", 0, 0, |b| {
                            b.extend_from_slice(&bytes_per_frame.to_be_bytes());
                            b.extend_from_slice(&(frames as u32).to_be_bytes());
                        });
                        co64(b, &self.sound_chunks);
                    });
                });
            });
        });
    }
}

/// Core Audio channel layout tags: mono, stereo, else "discrete channels".
fn channel_layout_tag(channels: u16) -> u32 {
    match channels {
        1 => 100 << 16 | 1,
        2 => 101 << 16 | 2,
        n => 147 << 16 | u32::from(n),
    }
}

fn atom(b: &mut Vec<u8>, kind: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let start = b.len();
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(kind);
    body(b);
    let size = (b.len() - start) as u32;
    b[start..start + 4].copy_from_slice(&size.to_be_bytes());
}

fn full_atom(
    b: &mut Vec<u8>,
    kind: &[u8; 4],
    version: u8,
    flags: u32,
    body: impl FnOnce(&mut Vec<u8>),
) {
    atom(b, kind, |b| {
        b.push(version);
        b.extend_from_slice(&flags.to_be_bytes()[1..]);
        body(b);
    });
}

fn matrix(b: &mut Vec<u8>) {
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        b.extend_from_slice(&v.to_be_bytes());
    }
}

fn tkhd(b: &mut Vec<u8>, id: u32, duration: u64, volume: u16, width: u32, height: u32) {
    full_atom(b, b"tkhd", 1, 0x3, |b| {
        b.extend_from_slice(&0u64.to_be_bytes());
        b.extend_from_slice(&0u64.to_be_bytes());
        b.extend_from_slice(&id.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&duration.to_be_bytes());
        b.extend_from_slice(&[0; 8]);
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(&volume.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
        matrix(b);
        b.extend_from_slice(&(width << 16).to_be_bytes());
        b.extend_from_slice(&(height << 16).to_be_bytes());
    });
}

/// An edit list that plays the whole track from movie time zero.
fn edts(b: &mut Vec<u8>, movie_duration: u64) {
    atom(b, b"edts", |b| {
        full_atom(b, b"elst", 1, 0, |b| {
            b.extend_from_slice(&1u32.to_be_bytes());
            b.extend_from_slice(&movie_duration.to_be_bytes());
            b.extend_from_slice(&0i64.to_be_bytes());
            b.extend_from_slice(&1u16.to_be_bytes());
            b.extend_from_slice(&0u16.to_be_bytes());
        });
    });
}

fn mdhd(b: &mut Vec<u8>, timescale: u32, duration: u64) {
    full_atom(b, b"mdhd", 1, 0, |b| {
        b.extend_from_slice(&0u64.to_be_bytes());
        b.extend_from_slice(&0u64.to_be_bytes());
        b.extend_from_slice(&timescale.to_be_bytes());
        b.extend_from_slice(&duration.to_be_bytes());
        // Language "und", packed ISO-639-2/T.
        b.extend_from_slice(&0x55C4u16.to_be_bytes());
        b.extend_from_slice(&0u16.to_be_bytes());
    });
}

fn hdlr(b: &mut Vec<u8>, subtype: &[u8; 4], name: &str) {
    full_atom(b, b"hdlr", 0, 0, |b| {
        b.extend_from_slice(b"mhlr");
        b.extend_from_slice(subtype);
        b.extend_from_slice(&[0; 12]);
        b.extend_from_slice(name.as_bytes());
        b.push(0);
    });
}

fn dinf(b: &mut Vec<u8>) {
    atom(b, b"dinf", |b| {
        full_atom(b, b"dref", 0, 0, |b| {
            b.extend_from_slice(&1u32.to_be_bytes());
            // The media is in this file.
            full_atom(b, b"url ", 0, 1, |_| {});
        });
    });
}

fn stts(b: &mut Vec<u8>, count: u64, delta: u32) {
    full_atom(b, b"stts", 0, 0, |b| {
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&(count as u32).to_be_bytes());
        b.extend_from_slice(&delta.to_be_bytes());
    });
}

fn stsc(b: &mut Vec<u8>, chunks: &[Chunk]) {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for (i, c) in chunks.iter().enumerate() {
        if runs.last().map(|r| r.1) != Some(c.samples) {
            runs.push((i as u32 + 1, c.samples));
        }
    }
    full_atom(b, b"stsc", 0, 0, |b| {
        b.extend_from_slice(&(runs.len() as u32).to_be_bytes());
        for (first, samples) in runs {
            b.extend_from_slice(&first.to_be_bytes());
            b.extend_from_slice(&samples.to_be_bytes());
            b.extend_from_slice(&1u32.to_be_bytes());
        }
    });
}

fn co64(b: &mut Vec<u8>, chunks: &[Chunk]) {
    full_atom(b, b"co64", 0, 0, |b| {
        b.extend_from_slice(&(chunks.len() as u32).to_be_bytes());
        for c in chunks {
            b.extend_from_slice(&c.offset.to_be_bytes());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_follows_the_picture_to_the_sample_at_any_rate() {
        for (num, den) in [
            (24, 1),
            (24000, 1001),
            (30000, 1001),
            (25, 1),
            (60000, 1001),
        ] {
            let rate = Rate::new(num, den).unwrap();
            for sr in [44_100u32, 48_000, 96_000] {
                let frames = 10_000u64;
                let total: u64 = (0..frames)
                    .map(|k| MovWriter::sound_frames_for(rate, sr, k))
                    .sum();
                // Exactly the samples in that much time, floored.
                let exact = (u128::from(frames) * u128::from(den) * u128::from(sr)
                    / u128::from(num)) as u64;
                assert_eq!(total, exact, "{num}/{den} at {sr}");
            }
        }
        // 29.97 at 48 kHz alternates 1601 and 1602 in a five-frame pattern.
        let ntsc = Rate::new(30000, 1001).unwrap();
        let five: Vec<u64> = (0..5)
            .map(|k| MovWriter::sound_frames_for(ntsc, 48_000, k))
            .collect();
        assert_eq!(five.iter().sum::<u64>(), 8008);
    }
}
