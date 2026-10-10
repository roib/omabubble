//! Picture attachments as small JPEGs for the TUI.
//!
//! A request names an attachment by id only. The file must be on an allowlist
//! of picture types, live inside the Messages attachments folder once `~` and
//! symlinks are resolved, and be under a size cap. `sips` converts it (HEIC
//! included) and resizes it; `jpeg::sanitize` then strips its metadata
//! (`sips` keeps EXIF, location included). Nothing is cached on disk.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

const ALLOWED_MIME: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/heic",
    "image/heif",
    "image/webp",
    "image/tiff",
];
const ALLOWED_EXT: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "heic", "heif", "webp", "tif", "tiff",
];
const MAX_SOURCE: u64 = 50 * 1024 * 1024;
const MAX_OUTPUT: u64 = 2 * 1024 * 1024;
const MIN_PX: u32 = 200;
const MAX_PX: u32 = 2000;
const MAX_PARALLEL: usize = 2;
const CONVERT_TIMEOUT: Duration = Duration::from_secs(20);

/// Whether an attachment can be served as a picture, by MIME type or, when
/// chat.db has none, by file extension.
pub fn is_picture(mime: Option<&str>, name: &str) -> bool {
    match mime {
        Some(m) => ALLOWED_MIME.contains(&m.to_ascii_lowercase().as_str()),
        None => name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| ALLOWED_EXT.contains(&ext.to_ascii_lowercase().as_str())),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Convert {
    /// `sips` on macOS: any allowed type → resized JPEG.
    Sips,
    /// Return the file's bytes as-is (Linux dev/demo runs and tests).
    Passthrough,
}

pub struct Images {
    /// Canonical attachments folder; `None` when it doesn't exist.
    root: Option<PathBuf>,
    home: PathBuf,
    convert: Convert,
    running: Mutex<usize>,
    free: Condvar,
}

impl Images {
    pub fn new(root: &Path, home: PathBuf, convert: Convert) -> Images {
        Images {
            root: root.canonicalize().ok(),
            home,
            convert,
            running: Mutex::new(0),
            free: Condvar::new(),
        }
    }

    /// The file behind a chat.db `attachment.filename`, if it's allowed.
    pub fn resolve(&self, filename: &str) -> Result<PathBuf> {
        let root = self.root.as_ref().context("no attachments folder")?;
        let path = match filename.strip_prefix("~/") {
            Some(rest) => self.home.join(rest),
            None => PathBuf::from(filename),
        };
        // Resolves `..` and symlinks, so the check below can't be dodged.
        let real = path.canonicalize().context("attachment file not found")?;
        if !real.starts_with(root) {
            bail!("attachment is outside the Messages attachments folder");
        }
        let meta = std::fs::metadata(&real)?;
        if !meta.is_file() {
            bail!("attachment is not a file");
        }
        if meta.len() > MAX_SOURCE {
            bail!("attachment is too large to preview");
        }
        Ok(real)
    }

    /// The picture as image bytes no larger than `max_px` on its long side,
    /// without metadata, plus the EXIF orientation to display it with.
    pub fn render(&self, src: &Path, max_px: u32) -> Result<(Vec<u8>, Option<u8>)> {
        let _slot = self.slot();
        let max_px = max_px.clamp(MIN_PX, MAX_PX);
        let (bytes, orientation) = match self.convert {
            Convert::Sips => crate::jpeg::sanitize(&sips(src, max_px)?)?,
            Convert::Passthrough => {
                let bytes = std::fs::read(src)?;
                if bytes.starts_with(&[0xFF, 0xD8]) {
                    crate::jpeg::sanitize(&bytes)?
                } else {
                    (bytes, None)
                }
            }
        };
        if bytes.len() as u64 > MAX_OUTPUT {
            bail!("preview is too large");
        }
        Ok((bytes, orientation))
    }

    /// Waits for one of the few conversion slots.
    fn slot(&self) -> Slot<'_> {
        let mut running = self.running.lock().unwrap();
        while *running >= MAX_PARALLEL {
            running = self.free.wait(running).unwrap();
        }
        *running += 1;
        Slot(self)
    }
}

struct Slot<'a>(&'a Images);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        *self.0.running.lock().unwrap() -= 1;
        self.0.free.notify_one();
    }
}

/// `sips -s format jpeg -Z max src --out tmp`, argv only, into a private
/// temp directory that's removed afterwards.
fn sips(src: &Path, max_px: u32) -> Result<Vec<u8>> {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "imsg-img-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&dir)?;
    let _cleanup = RemoveDir(dir.clone());
    std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    let out = dir.join("preview.jpg");

    let mut child = Command::new("sips")
        .args(["-s", "format", "jpeg", "-s", "formatOptions", "80", "-Z"])
        .arg(max_px.to_string())
        .arg(src)
        .arg("--out")
        .arg(&out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("running sips")?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > CONVERT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("sips timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        bail!("sips could not convert this picture");
    }
    Ok(std::fs::read(&out)?)
}

struct RemoveDir(PathBuf);

impl Drop for RemoveDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A temp "home" with `Library/Messages/Attachments/aa/pic.png` and a
    /// file outside it, plus a symlink pointing out.
    pub fn fixture() -> (PathBuf, Images) {
        static N: AtomicU64 = AtomicU64::new(0);
        let home = std::env::temp_dir().join(format!(
            "imsg-images-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let att = home.join("Library/Messages/Attachments/aa");
        std::fs::create_dir_all(&att).unwrap();
        std::fs::write(att.join("pic.png"), b"\x89PNG fake picture bytes").unwrap();
        std::fs::write(att.join("big.png"), vec![0u8; (MAX_OUTPUT + 1) as usize]).unwrap();
        std::fs::write(home.join("secret.txt"), b"not an attachment").unwrap();
        std::os::unix::fs::symlink(home.join("secret.txt"), att.join("link.png")).unwrap();
        let images = Images::new(
            &home.join("Library/Messages/Attachments"),
            home.clone(),
            Convert::Passthrough,
        );
        (home, images)
    }

    #[test]
    fn picture_types() {
        assert!(is_picture(Some("image/heic"), "x"));
        assert!(is_picture(Some("IMAGE/JPEG"), "x"));
        assert!(!is_picture(Some("video/quicktime"), "x.jpg"), "mime wins");
        assert!(!is_picture(Some("image/svg+xml"), "x.svg"));
        assert!(is_picture(None, "IMG_1.HEIC"));
        assert!(!is_picture(None, "notes.pdf"));
    }

    #[test]
    fn resolves_only_inside_the_attachments_folder() {
        let (home, images) = fixture();
        let ok = images
            .resolve("~/Library/Messages/Attachments/aa/pic.png")
            .unwrap();
        assert!(ok.ends_with("aa/pic.png"));
        assert_eq!(
            images.render(&ok, 800).unwrap(),
            (b"\x89PNG fake picture bytes".to_vec(), None)
        );

        // JPEGs lose their metadata and report their orientation.
        let photo = home.join("Library/Messages/Attachments/aa/photo.jpg");
        std::fs::write(&photo, crate::jpeg::tests::jpeg_with_metadata(6, true)).unwrap();
        let (bytes, o) = images
            .render(&images.resolve(photo.to_str().unwrap()).unwrap(), 800)
            .unwrap();
        assert_eq!(o, Some(6));
        assert!(!String::from_utf8_lossy(&bytes).contains("GPS"));

        let escape = images.resolve("~/Library/Messages/Attachments/aa/../../../../secret.txt");
        assert!(escape.unwrap_err().to_string().contains("outside"));
        let link = images.resolve("~/Library/Messages/Attachments/aa/link.png");
        assert!(
            link.unwrap_err().to_string().contains("outside"),
            "symlinks are followed"
        );
        let abs = images.resolve(home.join("secret.txt").to_str().unwrap());
        assert!(abs.unwrap_err().to_string().contains("outside"));
        assert!(
            images
                .resolve("~/Library/Messages/Attachments/aa/missing.png")
                .is_err()
        );

        let big = images
            .resolve("~/Library/Messages/Attachments/aa/big.png")
            .unwrap();
        assert!(
            images
                .render(&big, 800)
                .unwrap_err()
                .to_string()
                .contains("too large")
        );
    }

    #[test]
    fn no_attachments_folder() {
        let images = Images::new(
            Path::new("/nonexistent/Attachments"),
            "/tmp".into(),
            Convert::Passthrough,
        );
        assert!(
            images
                .resolve("/tmp/x.png")
                .unwrap_err()
                .to_string()
                .contains("no attachments folder")
        );
    }
}
