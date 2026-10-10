//! Picture thumbnails: what's loaded, what's wanted, and the terminal
//! graphics for each. Pictures live in memory only.

use base64::Engine;
use image::DynamicImage;
use ratatui::layout::{Rect, Size};
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use std::collections::{HashMap, VecDeque};

/// Rows a thumbnail occupies in the conversation, loaded or not.
pub const THUMB_ROWS: u16 = 8;
/// Long side requested from the bridge for thumbnails.
pub const THUMB_PX: u32 = 800;
/// Widest a thumbnail gets, in columns.
pub const THUMB_MAX_COLS: u16 = 48;
/// Decoded pictures kept; the least recently drawn go first.
const KEEP: usize = 40;

pub enum Pic {
    Loading,
    Ready {
        img: DynamicImage,
        /// Encoded for the terminal at this many columns at most.
        proto: Option<(u16, Protocol)>,
    },
    Failed,
}

#[derive(Default)]
pub struct Pics {
    /// `None`: pictures are off (config, or tests).
    pub picker: Option<Picker>,
    map: HashMap<i64, Pic>,
    /// Ready pictures, least recently used first.
    lru: VecDeque<i64>,
    wanted: Vec<i64>,
    /// Where pictures were drawn last frame.
    pub drawn: Vec<(Rect, i64)>,
    /// Pictures moved or vanished: clear the screen so no pixels linger.
    pub needs_clear: bool,
}

impl Pics {
    pub fn enabled(&self) -> bool {
        self.picker.is_some()
    }

    pub fn get(&self, id: i64) -> Option<&Pic> {
        self.map.get(&id)
    }

    /// Ask for a picture that isn't loaded, loading or failed yet.
    pub fn want(&mut self, id: i64) {
        if self.enabled() && !self.map.contains_key(&id) && !self.wanted.contains(&id) {
            self.wanted.push(id);
        }
    }

    /// Wanted pictures to request now; they count as loading from here on.
    pub fn take_wanted(&mut self) -> Vec<i64> {
        let ids = std::mem::take(&mut self.wanted);
        for &id in &ids {
            self.map.insert(id, Pic::Loading);
        }
        ids
    }

    /// A picture from the bridge; `orientation` is its EXIF orientation,
    /// sent separately because the bridge strips the EXIF block.
    pub fn loaded(&mut self, id: i64, b64: &str, orientation: Option<u8>) {
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
                self.map.insert(id, Pic::Ready { img, proto: None });
                self.touch(id);
            }
            None => self.failed(id),
        }
    }

    pub fn failed(&mut self, id: i64) {
        self.map.insert(id, Pic::Failed);
    }

    /// The connection dropped: whatever was loading gets asked for again.
    pub fn forget_loading(&mut self) {
        self.map.retain(|_, p| !matches!(p, Pic::Loading));
    }

    /// The picture encoded for the terminal, at most `max_cols` wide and
    /// `THUMB_ROWS` tall, re-encoded only when the width changes.
    pub fn protocol(&mut self, id: i64, max_cols: u16) -> Option<&Protocol> {
        let picker = self.picker.as_ref()?;
        let Some(Pic::Ready { img, proto }) = self.map.get_mut(&id) else {
            return None;
        };
        if proto.as_ref().is_none_or(|(w, _)| *w != max_cols) {
            let p = picker
                .new_protocol(
                    img.clone(),
                    Size::new(max_cols, THUMB_ROWS),
                    Resize::Fit(None),
                )
                .ok()?;
            *proto = Some((max_cols, p));
        }
        self.touch(id);
        match self.map.get(&id) {
            Some(Pic::Ready {
                proto: Some((_, p)),
                ..
            }) => Some(p),
            _ => None,
        }
    }

    fn touch(&mut self, id: i64) {
        self.lru.retain(|&x| x != id);
        self.lru.push_back(id);
        while self.lru.len() > KEEP {
            if let Some(old) = self.lru.pop_front() {
                self.map.remove(&old);
            }
        }
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
    fn want_once_then_load() {
        let mut p = enabled();
        p.want(1);
        p.want(1);
        assert_eq!(p.take_wanted(), vec![1]);
        p.want(1);
        assert!(p.take_wanted().is_empty(), "loading isn't asked for twice");
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
        assert_eq!(p.take_wanted(), vec![1]);
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
