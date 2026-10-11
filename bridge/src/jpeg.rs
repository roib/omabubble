//! Strip metadata from a JPEG before it leaves the Mac.
//!
//! `sips` keeps the EXIF block when it converts, and that includes location.
//! This drops every EXIF, XMP and other application segment and every
//! comment, keeping only what's needed to show the picture (and its colour
//! profile). The EXIF orientation is read first and returned, because the
//! picture's pixels are stored as the camera recorded them.

use anyhow::{Result, bail};

const SOI: u8 = 0xD8;
const SOS: u8 = 0xDA;
const APP0: u8 = 0xE0;
const APP1: u8 = 0xE1;
const APP2: u8 = 0xE2;
const APP15: u8 = 0xEF;
const COM: u8 = 0xFE;

/// The JPEG without metadata, and its EXIF orientation (1–8) if it had one.
pub fn sanitize(jpeg: &[u8]) -> Result<(Vec<u8>, Option<u8>)> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != SOI {
        bail!("not a JPEG");
    }
    let mut out = Vec::with_capacity(jpeg.len());
    out.extend_from_slice(&jpeg[..2]);
    let mut orientation = None;
    let mut i = 2;
    loop {
        // Fill bytes (0xFF padding) may precede a marker.
        while jpeg.get(i) == Some(&0xFF) && jpeg.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        let (Some(&0xFF), Some(&marker)) = (jpeg.get(i), jpeg.get(i + 1)) else {
            bail!("malformed JPEG");
        };
        // Markers without a length field.
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            out.extend_from_slice(&jpeg[i..i + 2]);
            i += 2;
            if marker == 0xD9 {
                return Ok((out, orientation));
            }
            continue;
        }
        let len = match (jpeg.get(i + 2), jpeg.get(i + 3)) {
            (Some(&hi), Some(&lo)) => usize::from(u16::from_be_bytes([hi, lo])),
            _ => bail!("malformed JPEG"),
        };
        let end = i + 2 + len;
        if len < 2 || end > jpeg.len() {
            bail!("malformed JPEG");
        }
        let payload = &jpeg[i + 4..end];
        if marker == SOS {
            // Image data from here on; markers inside it are part of the picture.
            out.extend_from_slice(&jpeg[i..]);
            return Ok((out, orientation));
        }
        let keep = match marker {
            APP0 => true,
            APP1 => {
                if let Some(rest) = payload.strip_prefix(b"Exif\0\0") {
                    orientation = orientation.or(exif_orientation(rest));
                }
                false
            }
            // The colour profile only describes colours.
            APP2 => payload.starts_with(b"ICC_PROFILE\0"),
            0xE3..=APP15 | COM => false,
            _ => true,
        };
        if keep {
            out.extend_from_slice(&jpeg[i..end]);
        }
        i = end;
    }
}

/// Orientation tag (0x0112) from IFD0 of a TIFF-structured EXIF block.
fn exif_orientation(tiff: &[u8]) -> Option<u8> {
    let le = match tiff.get(..4)? {
        [b'I', b'I', 42, 0] => true,
        [b'M', b'M', 0, 42] => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b = tiff.get(at..at + 2)?;
        Some(if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b = tiff.get(at..at + 4)?;
        let b = [b[0], b[1], b[2], b[3]];
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let ifd = u32_at(4)? as usize;
    let count = usize::from(u16_at(ifd)?);
    (0..count)
        .find_map(|n| {
            let entry = ifd + 2 + n * 12;
            (u16_at(entry)? == 0x0112)
                .then(|| u16_at(entry + 8))
                .flatten()
        })
        .and_then(|v| u8::try_from(v).ok())
        .filter(|v| (1..=8).contains(v))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut s = vec![0xFF, marker];
        s.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        s.extend_from_slice(payload);
        s
    }

    /// EXIF with orientation `o` and a GPS IFD pointer, in either byte order.
    fn exif(o: u16, little: bool) -> Vec<u8> {
        let w16 = |v: u16| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let w32 = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let mut t = Vec::new();
        t.extend_from_slice(if little { b"II*\0" } else { b"MM\0*" });
        t.extend_from_slice(&w32(8));
        t.extend_from_slice(&w16(2)); // two entries
        // GPS IFD pointer (0x8825), then orientation.
        t.extend_from_slice(&w16(0x8825));
        t.extend_from_slice(&w16(4));
        t.extend_from_slice(&w32(1));
        t.extend_from_slice(&w32(0x40));
        t.extend_from_slice(&w16(0x0112));
        t.extend_from_slice(&w16(3));
        t.extend_from_slice(&w32(1));
        t.extend_from_slice(&w16(o));
        t.extend_from_slice(&[0, 0]);
        t.extend_from_slice(&w32(0));
        t.extend_from_slice(b"GPS 37.7749 N 122.4194 W");
        let mut p = b"Exif\0\0".to_vec();
        p.extend(t);
        p
    }

    /// A JPEG-shaped file with metadata in every place it can hide.
    pub fn jpeg_with_metadata(o: u16, little: bool) -> Vec<u8> {
        let mut j = vec![0xFF, SOI];
        j.extend(segment(APP0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0"));
        j.extend(segment(APP1, &exif(o, little)));
        j.extend(segment(
            APP1,
            b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>GPS</x:xmpmeta>",
        ));
        j.extend(segment(APP2, b"ICC_PROFILE\0\x01\x01colour"));
        j.extend(segment(0xED, b"Photoshop 3.0\0IPTC caption"));
        j.extend(segment(COM, b"a comment"));
        j.extend(segment(0xDB, b"\0quant table"));
        j.extend(segment(SOS, b"\x01\x01\0\0?\0"));
        j.extend_from_slice(b"\x12\x34\xFF\x00\x56 image data");
        j.extend_from_slice(&[0xFF, 0xD9]);
        j
    }

    #[test]
    fn strips_metadata_keeps_picture() {
        for little in [true, false] {
            let (out, o) = sanitize(&jpeg_with_metadata(6, little)).unwrap();
            assert_eq!(o, Some(6));
            let text = String::from_utf8_lossy(&out);
            for gone in ["Exif", "GPS", "xmpmeta", "IPTC", "a comment"] {
                assert!(!text.contains(gone), "{gone} survived");
            }
            for kept in ["JFIF", "ICC_PROFILE", "quant table", "image data"] {
                assert!(text.contains(kept), "{kept} was dropped");
            }
            assert_eq!(&out[out.len() - 2..], &[0xFF, 0xD9]);
        }
    }

    /// Checks a real file: `OMABUBBLE_SAMPLE_JPEG=photo.jpg cargo test -p bridge -- --ignored`.
    #[test]
    #[ignore]
    fn sample_from_env() {
        let path = std::env::var("OMABUBBLE_SAMPLE_JPEG").expect("set OMABUBBLE_SAMPLE_JPEG");
        let (out, o) = sanitize(&std::fs::read(path).unwrap()).unwrap();
        println!(
            "orientation: {o:?}; Exif left: {}",
            out.windows(6).any(|w| w == b"Exif\0\0")
        );
        assert!(!out.windows(6).any(|w| w == b"Exif\0\0"));
    }

    #[test]
    fn rejects_non_jpeg_and_bad_values() {
        assert!(sanitize(b"\x89PNG not a jpeg").is_err());
        assert!(
            sanitize(&[0xFF, SOI, 0xFF, 0xE1, 0xFF, 0xFF]).is_err(),
            "length runs past the end"
        );
        let (_, o) = sanitize(&jpeg_with_metadata(42, true)).unwrap();
        assert_eq!(o, None, "out-of-range orientation is ignored");
    }
}
