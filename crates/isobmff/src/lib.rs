//! ISO BMFF (MP4) and QuickTime (MOV) demuxer and muxer: a re-export of
//! [`craft_media::isobmff`] from craft-libs. Supported boxes and limitations are in the
//! craft-media README.
//!
//! [`open`] adds one thing on top: Dolby Vision tracks (`dvh1` / `dvhe` / `dva1` / `dvav`, what an
//! iPhone records by default) are read as the HEVC / H.264 stream their `hvcC` / `avcC` box
//! describes. Their base layer is that stream (Dolby Vision profiles 8.4 and 9 are HLG / SDR
//! compatible, ETSI TS 103 572 §5), so every decoder takes it; craft-media leaves them unknown.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub use craft_media::isobmff::*;

/// Open an MP4 / MOV file ([`craft_media::isobmff::open`]), with Dolby Vision tracks read as their
/// HEVC / H.264 base layer.
pub fn open<S: ByteSource>(src: S) -> Result<Mp4File> {
    let mut file = craft_media::isobmff::open(src)?;
    for entry in file.tracks.iter_mut().flat_map(|t| t.entries.iter_mut()) {
        dolby_vision_base_layer(entry);
    }
    Ok(file)
}

/// Bytes of a VisualSampleEntry before its child boxes (ISO/IEC 14496-12 §12.1.3), counted from
/// the end of the entry's box header.
const VISUAL_ENTRY_FIELDS: usize = 78;

/// A Dolby Vision entry craft-media left unknown, made the HEVC / H.264 entry of its base layer.
/// Anything else (or a malformed configuration box) is left as it is.
fn dolby_vision_base_layer(entry: &mut SampleEntry) {
    let CodecConfig::Unknown { fourcc, raw } = &entry.codec else { return };
    let (want, plain) = match &fourcc.0 {
        b"dvh1" | b"dvhe" => (b"hvcC", b"hvc1"),
        b"dva1" | b"dvav" => (b"avcC", b"avc1"),
        _ => return,
    };
    let Some(payload) = raw.get(VISUAL_ENTRY_FIELDS..).and_then(|boxes| child_box(boxes, want)) else { return };
    let codec = if want == b"hvcC" { HevcConfig::parse(payload).map(CodecConfig::Hevc) } else { AvcConfig::parse(payload).map(CodecConfig::Avc) };
    if let Ok(codec) = codec {
        entry.codec = codec;
        entry.format = FourCc(*plain);
    }
}

/// The payload of the first `kind` box in a run of boxes (32-bit and 64-bit sizes; a box running
/// to the end). None when it is missing or a size is out of bounds.
fn child_box<'a>(mut data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    // a sample entry holds a handful of boxes: a bound keeps a hostile run of tiny boxes cheap
    for _ in 0..256 {
        let size = u64::from(u32::from_be_bytes(data.get(0..4)?.try_into().ok()?));
        let name = data.get(4..8)?;
        let (header, size) = match size {
            0 => (8u64, data.len() as u64),
            1 => (16, u64::from_be_bytes(data.get(8..16)?.try_into().ok()?)),
            n => (8, n),
        };
        if size < header || size > data.len() as u64 {
            return None;
        }
        let (header, size) = (usize::try_from(header).ok()?, usize::try_from(size).ok()?);
        if name == kind {
            return data.get(header..size);
        }
        data = data.get(size..)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A one-sample file with `entry`, its stsd fourcc then replaced by `as_fourcc`.
    fn file_with(entry: SampleEntry, as_fourcc: &[u8; 4]) -> Vec<u8> {
        let from = entry.format.0;
        let mut w = Mp4Writer::new(Cursor::new(Vec::new()), WriterOptions::new(Brand::Mov)).unwrap();
        let t = w.add_track(TrackConfig::new(entry, 600)).unwrap();
        w.write_sample(t, WriteSample { data: &[0, 0, 0, 2, 0x26, 1], duration: 20, composition_offset: 0, is_sync: true }).unwrap();
        let mut bytes = w.finish().unwrap().into_inner();
        let stsd = bytes.windows(4).position(|x| x == b"stsd").unwrap();
        let at = stsd + bytes[stsd..].windows(4).position(|x| x == from).unwrap();
        bytes[at..at + 4].copy_from_slice(as_fourcc);
        bytes
    }

    fn hevc() -> SampleEntry {
        let cfg = HevcConfig { general_profile_idc: 2, chroma_format_idc: 1, bit_depth_luma: 10, bit_depth_chroma: 10, length_size: 4, ..Default::default() };
        SampleEntry::hevc(cfg, 3840, 2160)
    }

    #[test]
    fn an_iphone_dolby_vision_track_reads_as_hevc() {
        for fourcc in [b"dvh1", b"dvhe"] {
            let bytes = file_with(hevc(), fourcc);
            assert!(
                matches!(craft_media::isobmff::open(bytes.as_slice()).unwrap().tracks[0].entries[0].codec, CodecConfig::Unknown { .. }),
                "craft-media alone"
            );
            let e = &open(bytes.as_slice()).unwrap().tracks[0].entries[0];
            assert!(matches!(&e.codec, CodecConfig::Hevc(c) if c.bit_depth_luma == 10), "{:?}", e.codec);
            assert_eq!(&e.format.0, b"hvc1");
        }
    }

    #[test]
    fn a_dolby_vision_avc_track_reads_as_h264() {
        let avc = SampleEntry::avc(AvcConfig::new(vec![vec![0x67, 0x64, 0, 0x28, 1, 2]], vec![vec![0x68, 3]], 4), 1920, 1080);
        let e = &open(file_with(avc, b"dva1").as_slice()).unwrap().tracks[0].entries[0];
        assert!(matches!(e.codec, CodecConfig::Avc(_)));
        assert_eq!(&e.format.0, b"avc1");
    }

    #[test]
    fn hostile_child_boxes_are_ignored() {
        assert_eq!(child_box(&[], b"hvcC"), None);
        assert_eq!(child_box(&[0, 0, 0, 4, b'h', b'v', b'c', b'C'], b"hvcC"), None, "size below the header");
        assert_eq!(child_box(&[0, 0, 0, 99, b'h', b'v', b'c', b'C'], b"hvcC"), None, "size past the end");
        assert_eq!(child_box(&[0, 0, 0, 1, b'h', b'v', b'c', b'C', 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff], b"hvcC"), None);
        assert_eq!(child_box(&[0, 0, 0, 10, b'h', b'v', b'c', b'C', 7, 8], b"hvcC"), Some(&[7u8, 8][..]));
        assert_eq!(child_box(&[0, 0, 0, 0, b'h', b'v', b'c', b'C', 7], b"hvcC"), Some(&[7u8][..]), "runs to the end");
        // truncated or garbage configuration: the track stays unknown, nothing panics
        let mut e = hevc();
        e.codec = CodecConfig::Unknown { fourcc: FourCc(*b"dvh1"), raw: vec![0; VISUAL_ENTRY_FIELDS + 9] };
        dolby_vision_base_layer(&mut e);
        assert!(matches!(e.codec, CodecConfig::Unknown { .. }));
    }
}
