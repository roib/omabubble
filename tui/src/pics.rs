//! Picture thumbnails: what's loaded, what's wanted, and the terminal
//! graphics for each. Pictures live in memory only.

use base64::Engine;
use image::DynamicImage;
use ratatui::layout::{Rect, Size};
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

/// Rows a thumbnail occupies in the conversation, loaded or not.
pub const THUMB_ROWS: u16 = 8;
/// Long side requested from the bridge for thumbnails.
pub const THUMB_PX: u32 = 800;
/// Long side requested for the full-pane viewer.
pub const FULL_PX: u32 = 1600;
/// Widest a thumbnail gets, in columns.
pub const THUMB_MAX_COLS: u16 = 48;
/// Decoded pictures kept; the least recently drawn go first.
const KEEP: usize = 40;

/// A picture at one size: (attachment id, long side in pixels).
pub type Key = (i64, u32);

pub enum Pic {
    Loading,
    Ready {
        img: DynamicImage,
        /// Encoded for the terminal to fit this many cells.
        proto: Option<(Size, Protocol)>,
    },
    Failed,
}

#[derive(Default)]
pub struct Pics {
    /// `None`: pictures are off (config, or tests).
    pub picker: Option<Picker>,
    map: HashMap<Key, Pic>,
    /// Ready pictures, least recently used first.
    lru: VecDeque<Key>,
    wanted: Vec<Key>,
    /// Where pictures were drawn last frame.
    pub drawn: Vec<(Rect, i64)>,
    /// Pictures moved or vanished: clear the screen so no pixels linger.
    pub needs_clear: bool,
    /// Pictures drawn so far this frame, by any pane.
    frame: Vec<(Rect, i64)>,
    /// Files on this machine previewed in the attach prompt.
    local: HashMap<PathBuf, Local>,
}

/// A file previewed before attaching it.
pub enum Local {
    Loading,
    Ready {
        img: DynamicImage,
        /// The original's size, and its format ("PNG").
        width: u32,
        height: u32,
        format: String,
        proto: Option<(Size, Protocol)>,
    },
    /// Not a picture this can draw: a short description instead.
    Note(String),
}

/// What a background decode of a local file came to.
pub type Decoded = Result<(DynamicImage, u32, u32, String), String>;

/// Picture types the preview decodes; anything else just gets a note.
const PREVIEWABLE: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp"];
/// Long side kept for previews: plenty for a pane, quick to encode.
const PREVIEW_PX: u32 = 1024;

/// Decodes a file for preview (run off the UI thread), the right way up and
/// shrunk to `PREVIEW_PX`.
pub fn decode_local(path: &Path) -> Decoded {
    let reader = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| e.to_string())?;
    let format = reader.format().map_or_else(
        || "picture".to_string(),
        |f| format!("{f:?}").to_uppercase(),
    );
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = image::ImageDecoder::orientation(&mut decoder).ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    let (width, height) = (img.width(), img.height());
    if width.max(height) > PREVIEW_PX {
        img = img.thumbnail(PREVIEW_PX, PREVIEW_PX);
    }
    Ok((img, width, height, format))
}

impl Pics {
    pub fn enabled(&self) -> bool {
        self.picker.is_some()
    }

    /// The thumbnail's state.
    pub fn get(&self, id: i64) -> Option<&Pic> {
        self.get_at((id, THUMB_PX))
    }

    pub fn get_at(&self, key: Key) -> Option<&Pic> {
        self.map.get(&key)
    }

    /// Ask for a thumbnail that isn't loaded, loading or failed yet.
    pub fn want(&mut self, id: i64) {
        self.want_at((id, THUMB_PX));
    }

    pub fn want_at(&mut self, key: Key) {
        if self.enabled() && !self.map.contains_key(&key) && !self.wanted.contains(&key) {
            self.wanted.push(key);
        }
    }

    /// Wanted pictures to request now; they count as loading from here on.
    pub fn take_wanted(&mut self) -> Vec<Key> {
        let keys = std::mem::take(&mut self.wanted);
        for &key in &keys {
            self.map.insert(key, Pic::Loading);
        }
        keys
    }

    /// A thumbnail from the bridge (see `loaded_at`).
    #[cfg(test)]
    pub fn loaded(&mut self, id: i64, b64: &str, orientation: Option<u8>) {
        self.loaded_at((id, THUMB_PX), b64, orientation);
    }

    /// A picture from the bridge; `orientation` is its EXIF orientation,
    /// sent separately because the bridge strips the EXIF block.
    pub fn loaded_at(&mut self, key: Key, b64: &str, orientation: Option<u8>) {
        let img = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .ok()
            .and_then(|bytes| image::load_from_memory(&bytes).ok())
            .map(|mut img| {
                if let Some(o) = orientation.and_then(image::metadata::Orientation::from_exif) {
                    img.apply_orientation(o);
                }
                img
            });
        match img {
            Some(img) => {
                self.map.insert(key, Pic::Ready { img, proto: None });
                self.touch(key);
            }
            None => self.failed_at(key),
        }
    }

    #[cfg(test)]
    pub fn failed(&mut self, id: i64) {
        self.failed_at((id, THUMB_PX));
    }

    pub fn failed_at(&mut self, key: Key) {
        self.map.insert(key, Pic::Failed);
    }

    /// The connection dropped: whatever was loading gets asked for again.
    pub fn forget_loading(&mut self) {
        self.map.retain(|_, p| !matches!(p, Pic::Loading));
    }

    /// The thumbnail encoded for the terminal, at most `max_cols` wide and
    /// `THUMB_ROWS` tall.
    pub fn protocol(&mut self, id: i64, max_cols: u16) -> Option<&Protocol> {
        self.protocol_fit((id, THUMB_PX), Size::new(max_cols, THUMB_ROWS))
    }

    /// The picture encoded to fit `area` cells, re-encoded only when that changes.
    pub fn protocol_fit(&mut self, key: Key, area: Size) -> Option<&Protocol> {
        let picker = self.picker.as_ref()?;
        let Some(Pic::Ready { img, proto }) = self.map.get_mut(&key) else {
            return None;
        };
        if proto.as_ref().is_none_or(|(s, _)| *s != area) {
            let p = picker
                .new_protocol(img.clone(), area, Resize::Fit(None))
                .ok()?;
            *proto = Some((area, p));
        }
        self.touch(key);
        match self.map.get(&key) {
            Some(Pic::Ready {
                proto: Some((_, p)),
                ..
            }) => Some(p),
            _ => None,
        }
    }

    /// The picture as a JPEG the right way up, for saving.
    pub fn jpeg(&self, key: Key) -> Option<Vec<u8>> {
        let Some(Pic::Ready { img, .. }) = self.map.get(&key) else {
            return None;
        };
        let mut out = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90);
        img.to_rgb8().write_with_encoder(encoder).ok()?;
        Some(out)
    }

    fn touch(&mut self, key: Key) {
        self.lru.retain(|&x| x != key);
        self.lru.push_back(key);
        while self.lru.len() > KEEP {
            if let Some(old) = self.lru.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    /// Whether `path` should be decoded for a preview: a picture type not
    /// yet asked for. Other files get their note right away.
    pub fn want_local(&mut self, path: &Path) -> bool {
        if !self.enabled() || self.local.contains_key(path) {
            return false;
        }
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if PREVIEWABLE.contains(&ext.as_str()) {
            self.local.insert(path.to_path_buf(), Local::Loading);
            true
        } else {
            let kind = match ext.as_str() {
                "" => "no preview".to_string(),
                "heic" | "heif" => "HEIC: no preview".to_string(),
                e => format!("{}: no preview", e.to_uppercase()),
            };
            self.local.insert(path.to_path_buf(), Local::Note(kind));
            false
        }
    }

    pub fn local_decoded(&mut self, path: PathBuf, result: Decoded) {
        let entry = match result {
            Ok((img, width, height, format)) => Local::Ready {
                img,
                width,
                height,
                format,
                proto: None,
            },
            Err(_) => Local::Note("couldn't read it as a picture".into()),
        };
        self.local.insert(path, entry);
    }

    pub fn local(&self, path: &Path) -> Option<&Local> {
        self.local.get(path)
    }

    /// The preview encoded to fit `area` cells, re-encoded only when that changes.
    pub fn local_protocol_fit(&mut self, path: &Path, area: Size) -> Option<&Protocol> {
        let picker = self.picker.as_ref()?;
        let Some(Local::Ready { img, proto, .. }) = self.local.get_mut(path) else {
            return None;
        };
        if proto.as_ref().is_none_or(|(s, _)| *s != area) {
            let p = picker
                .new_protocol(img.clone(), area, Resize::Fit(None))
                .ok()?;
            *proto = Some((area, p));
        }
        proto.as_ref().map(|(_, p)| p)
    }

    /// The attach prompt closed: its previews go.
    pub fn forget_local(&mut self) {
        self.local.clear();
    }

    /// A pane drew these pictures this frame.
    pub fn add_drawn(&mut self, drawn: Vec<(Rect, i64)>) {
        self.frame.extend(drawn);
    }

    /// The frame is drawn: compare its pictures with the last frame's.
    pub fn end_frame(&mut self) {
        let drawn = std::mem::take(&mut self.frame);
        self.finish_frame(drawn);
    }

    /// Record this frame's pictures; flag a clear if any moved or vanished.
    pub fn finish_frame(&mut self, drawn: Vec<(Rect, i64)>) {
        if !self.drawn.is_empty() && drawn != self.drawn {
            self.needs_clear = true;
        }
        self.drawn = drawn;
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A tiny PNG, base64-encoded the way the bridge sends pictures.
    pub fn png_b64(w: u32, h: u32) -> String {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([40, 120, 200]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn enabled() -> Pics {
        Pics {
            picker: Some(Picker::halfblocks()),
            ..Default::default()
        }
    }

    #[test]
    fn local_previews() {
        let dir = std::env::temp_dir().join(format!("omabubble-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("wide.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::new(3000, 1000))
            .save(&png)
            .unwrap();
        let (img, w, h, format) = decode_local(&png).unwrap();
        assert_eq!(
            (w, h, format.as_str()),
            (3000, 1000, "PNG"),
            "the original's size"
        );
        assert_eq!(img.width(), PREVIEW_PX, "shrunk for the pane");
        let fake = dir.join("fake.jpg");
        std::fs::write(&fake, b"not a picture").unwrap();
        assert!(decode_local(&fake).is_err());

        let mut p = Pics {
            picker: Some(Picker::halfblocks()),
            ..Default::default()
        };
        assert!(p.want_local(&png), "a picture is decoded");
        assert!(!p.want_local(&png), "once");
        assert!(matches!(p.local(&png), Some(Local::Loading)));
        p.local_decoded(png.clone(), decode_local(&png));
        assert!(matches!(
            p.local(&png),
            Some(Local::Ready { width: 3000, .. })
        ));
        assert!(p.local_protocol_fit(&png, Size::new(20, 8)).is_some());
        // Other files get a note, never a decode.
        let pdf = dir.join("doc.pdf");
        assert!(!p.want_local(&pdf));
        assert!(matches!(p.local(&pdf), Some(Local::Note(n)) if n == "PDF: no preview"));
        assert!(!p.want_local(&dir.join("IMG_1.HEIC")));
        p.local_decoded(fake.clone(), decode_local(&fake));
        assert!(matches!(p.local(&fake), Some(Local::Note(_))));
        p.forget_local();
        assert!(p.local(&png).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn want_once_then_load() {
        let mut p = enabled();
        p.want(1);
        p.want(1);
        assert_eq!(p.take_wanted(), vec![(1, THUMB_PX)]);
        p.want(1);
        assert!(p.take_wanted().is_empty(), "loading isn't asked for twice");
        p.want_at((1, FULL_PX));
        assert_eq!(
            p.take_wanted(),
            vec![(1, FULL_PX)],
            "each size is its own picture"
        );
        p.loaded(1, &png_b64(40, 20), None);
        assert!(matches!(p.get(1), Some(Pic::Ready { .. })));
        let proto = p.protocol(1, 30).unwrap();
        assert!(proto.size().width <= 30 && proto.size().height <= THUMB_ROWS);
    }

    #[test]
    fn orientation_is_applied() {
        let mut p = enabled();
        p.loaded(1, &png_b64(40, 20), Some(6)); // "rotate 90° clockwise"
        let Some(Pic::Ready { img, .. }) = p.get(1) else {
            panic!()
        };
        assert_eq!((img.width(), img.height()), (20, 40));
        p.loaded(2, &png_b64(40, 20), Some(1));
        let Some(Pic::Ready { img, .. }) = p.get(2) else {
            panic!()
        };
        assert_eq!((img.width(), img.height()), (40, 20));
    }

    #[test]
    fn saved_jpeg_is_upright() {
        let mut p = enabled();
        p.loaded_at((1, FULL_PX), &png_b64(40, 20), Some(6));
        let jpeg = p.jpeg((1, FULL_PX)).unwrap();
        let back = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((back.width(), back.height()), (20, 40));
        assert!(p.jpeg((1, THUMB_PX)).is_none());
    }

    #[test]
    fn bad_bytes_fail_and_disabled_wants_nothing() {
        let mut p = enabled();
        p.loaded(2, "not base64!!", None);
        assert!(matches!(p.get(2), Some(Pic::Failed)));
        let mut off = Pics::default();
        off.want(3);
        assert!(off.take_wanted().is_empty());
    }

    #[test]
    fn reconnect_retries_loading_and_cache_is_bounded() {
        let mut p = enabled();
        p.want(1);
        p.take_wanted();
        p.forget_loading();
        p.want(1);
        assert_eq!(p.take_wanted(), vec![(1, THUMB_PX)]);
        let b64 = png_b64(4, 4);
        for id in 0..(KEEP as i64 + 5) {
            p.loaded(id, &b64, None);
        }
        assert!(p.get(0).is_none(), "oldest dropped");
        assert!(p.get(KEEP as i64 + 4).is_some());
    }

    #[test]
    fn moving_pictures_ask_for_a_clear() {
        let mut p = enabled();
        p.finish_frame(vec![(Rect::new(0, 0, 4, 4), 1)]);
        assert!(!p.needs_clear, "first frame");
        p.finish_frame(vec![(Rect::new(0, 0, 4, 4), 1)]);
        assert!(!p.needs_clear, "nothing moved");
        p.finish_frame(vec![(Rect::new(0, 2, 4, 4), 1)]);
        assert!(p.needs_clear);
    }
}
