//! The SPS parser against real streams: the size it reads must be the size the
//! decoder produces, and the colour it reads must be what the pictures carry.

use cine_media::h264_sps;
use cine_media::VideoReader;
use std::path::PathBuf;

fn assets() -> Vec<PathBuf> {
    [
        "/home/finstein-emp/Downloads/Super_cinematic_image_to_vid.mp4",
        "/home/finstein-emp/Downloads/A_solitary_monochrome_crimson.mp4",
    ]
    .iter()
    .map(PathBuf::from)
    .filter(|p| p.exists())
    .collect()
}

fn first_sps(path: &PathBuf) -> Vec<u8> {
    let file = std::fs::File::open(path).unwrap();
    let size = file.metadata().unwrap().len();
    let mp4 = mp4::Mp4Reader::read_header(std::io::BufReader::new(file), size).unwrap();
    let track = mp4
        .tracks()
        .values()
        .find(|t| t.track_type().ok() == Some(mp4::TrackType::Video))
        .unwrap();
    track.sequence_parameter_set().unwrap().to_vec()
}

#[test]
fn the_declared_size_is_the_decoded_size_and_the_colour_reaches_the_picture() {
    let found = assets();
    if found.is_empty() {
        eprintln!("SKIP: no real H.264 media on this machine");
        return;
    }
    for path in found {
        let sps = h264_sps::parse(&first_sps(&path)).expect("the real SPS parses");
        let mut reader = VideoReader::open_path(&path).unwrap();
        let frame = reader.frame_at_index(0).unwrap().unwrap();
        println!(
            "MEASURED {}: profile {} level {} chroma {} {}-bit, {}x{}, vui {:?} → colour {:?}",
            path.file_name().unwrap().to_string_lossy(),
            sps.profile_idc,
            sps.level_idc,
            sps.chroma_format_idc,
            sps.bit_depth_luma,
            sps.width,
            sps.height,
            sps.vui,
            sps.color()
        );
        assert_eq!(
            (sps.width, sps.height),
            (frame.width, frame.height),
            "{}",
            path.display()
        );
        assert_eq!(
            frame.color,
            sps.color(),
            "the picture carries what the stream declares"
        );
    }
}
