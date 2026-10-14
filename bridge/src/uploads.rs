//! Files uploaded by a TUI to send through Messages.
//!
//! Each upload gets its own folder under the staging directory, inside
//! `~/Library/Messages/Attachments`: Messages is sandboxed and sends files it
//! can read from there. The bridge picks the folder and a safe file name; the
//! TUI never names a path. Unfinished uploads are removed when their
//! connection ends, and sent files after a day (Messages may still be
//! transferring them for a while after `send` returns).

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// iMessage's own limit is about 100 MB.
pub const MAX_SIZE: u64 = 100 * 1024 * 1024;
/// Uploads one connection may have going at once.
const MAX_ACTIVE: usize = 4;
/// How long sent files stay staged.
const KEEP_SENT: Duration = Duration::from_secs(24 * 60 * 60);

struct Upload {
    dir: PathBuf,
    path: PathBuf,
    file: File,
    size: u64,
    received: u64,
}

/// One connection's uploads.
pub struct Uploads {
    staging: PathBuf,
    active: HashMap<String, Upload>,
}

impl Uploads {
    pub fn new(staging: PathBuf) -> Uploads {
        Uploads {
            staging,
            active: HashMap::new(),
        }
    }

    /// Starts an upload of `size` bytes; returns its id.
    pub fn start(&mut self, name: &str, size: u64) -> Result<String> {
        if size == 0 {
            bail!("empty file");
        }
        if size > MAX_SIZE {
            bail!("file too large (over {} MB)", MAX_SIZE / 1024 / 1024);
        }
        if self.active.len() >= MAX_ACTIVE {
            bail!("too many uploads at once");
        }
        clean_old(&self.staging, KEEP_SENT);
        let token = token()?;
        let dir = self.staging.join(&token);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .with_context(|| "creating the upload folder")?;
        let path = dir.join(safe_name(name));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| "creating the upload file")?;
        self.active.insert(
            token.clone(),
            Upload {
                dir,
                path,
                file,
                size,
                received: 0,
            },
        );
        Ok(token)
    }

    /// Appends the next piece; `offset` must be the bytes received so far.
    pub fn chunk(&mut self, token: &str, offset: u64, data: &[u8]) -> Result<()> {
        let Some(u) = self.active.get_mut(token) else {
            bail!("no such upload");
        };
        if offset != u.received {
            bail!("upload out of order (at {}, got {offset})", u.received);
        }
        if u.received + data.len() as u64 > u.size {
            bail!("upload larger than announced");
        }
        u.file.write_all(data)?;
        u.received += data.len() as u64;
        Ok(())
    }

    /// The finished file, ready to send. It stays on disk for Messages; it's
    /// no longer this connection's to clean up.
    pub fn finish(&mut self, token: &str) -> Result<PathBuf> {
        let Some(u) = self.active.get(token) else {
            bail!("no such upload");
        };
        if u.received != u.size {
            bail!("upload incomplete ({} of {} bytes)", u.received, u.size);
        }
        let mut u = self.active.remove(token).unwrap();
        u.file.flush()?;
        u.file.sync_all()?;
        Ok(u.path)
    }
}

impl Drop for Uploads {
    /// The connection ended: drop what never finished.
    fn drop(&mut self) {
        for (_, u) in self.active.drain() {
            let _ = std::fs::remove_dir_all(&u.dir);
        }
    }
}

/// Removes a sent file's folder, e.g. after Messages refused it.
pub fn discard(path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Removes upload folders older than `age`.
pub fn clean_old(staging: &Path, age: Duration) {
    let Ok(entries) = std::fs::read_dir(staging) else {
        return;
    };
    let now = SystemTime::now();
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).is_ok_and(|d| d > age));
        if old && e.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// A random id, also the folder name.
fn token() -> Result<String> {
    let mut b = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// The file name to keep: no folders, no hidden files, no control
/// characters, a sane length, and the extension kept so Messages knows the
/// type.
pub fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, ':' | '/' | '\\'))
        .collect();
    let clean = clean.trim().trim_start_matches('.').trim();
    if clean.is_empty() {
        return "attachment".into();
    }
    if clean.chars().count() <= 100 {
        return clean.to_string();
    }
    let (stem, ext) = match clean.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.chars().count() <= 10 => (s, Some(e)),
        _ => (clean, None),
    };
    let stem: String = stem.chars().take(80).collect();
    match ext {
        Some(e) => format!("{stem}.{e}"),
        None => stem,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staging() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "omabubble-uploads-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn upload_in_pieces() {
        let dir = staging();
        let mut ups = Uploads::new(dir.clone());
        let t = ups.start("photo.jpg", 6).unwrap();
        ups.chunk(&t, 0, b"abc").unwrap();
        assert!(ups.finish(&t).is_err(), "not complete yet");
        assert!(ups.chunk(&t, 0, b"def").is_err(), "wrong offset");
        ups.chunk(&t, 3, b"def").unwrap();
        assert!(ups.chunk(&t, 6, b"g").is_err(), "more than announced");
        let path = ups.finish(&t).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"abcdef");
        assert_eq!(path.file_name().unwrap(), "photo.jpg");
        assert_eq!(path.parent().unwrap().parent().unwrap(), dir);
        drop(ups);
        assert!(path.exists(), "a finished file stays for Messages");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unfinished_uploads_are_removed_with_the_connection() {
        let dir = staging();
        let mut ups = Uploads::new(dir.clone());
        let t = ups.start("a.pdf", 10).unwrap();
        ups.chunk(&t, 0, b"12345").unwrap();
        drop(ups);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn limits() {
        let dir = staging();
        let mut ups = Uploads::new(dir.clone());
        assert!(ups.start("x", 0).is_err());
        assert!(ups.start("x", MAX_SIZE + 1).is_err());
        for _ in 0..MAX_ACTIVE {
            ups.start("x", 1).unwrap();
        }
        assert!(ups.start("x", 1).is_err(), "too many at once");
        assert!(ups.chunk("nope", 0, b"x").is_err());
        drop(ups);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn safe_names() {
        assert_eq!(safe_name("IMG_2041.HEIC"), "IMG_2041.HEIC");
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("C:\\Users\\x\\doc.pdf"), "doc.pdf");
        assert_eq!(safe_name(".bashrc"), "bashrc");
        assert_eq!(safe_name("a\nb\u{7}.txt"), "ab.txt");
        assert_eq!(safe_name(""), "attachment");
        assert_eq!(safe_name("..."), "attachment");
        let long = format!("{}.jpeg", "x".repeat(200));
        assert_eq!(safe_name(&long), format!("{}.jpeg", "x".repeat(80)));
    }

    #[test]
    fn old_folders_are_cleaned() {
        let dir = staging();
        std::fs::create_dir_all(dir.join("old")).unwrap();
        clean_old(&dir, Duration::from_secs(3600));
        assert!(dir.join("old").exists(), "recent");
        clean_old(&dir, Duration::ZERO);
        std::thread::sleep(Duration::from_millis(5));
        clean_old(&dir, Duration::ZERO);
        assert!(!dir.join("old").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
