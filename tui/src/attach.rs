//! Files attached to the next message: recognising pasted or dropped paths,
//! completing typed ones, and reading them for upload.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// The bridge's limit, about iMessage's own.
pub const MAX_SIZE: u64 = 100 * 1024 * 1024;
/// Upload piece size: base64 of this stays well under the bridge's 1 MiB
/// line limit.
pub const CHUNK: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Attached {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
}

impl Attached {
    /// The file at `path`, if it's one that can be sent.
    pub fn new(path: &Path) -> Result<Attached, String> {
        let path = expand_home(path);
        let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !meta.is_file() {
            return Err(format!("{} isn't a file", path.display()));
        }
        if meta.len() == 0 {
            return Err(format!("{} is empty", path.display()));
        }
        if meta.len() > MAX_SIZE {
            return Err(format!(
                "{} is {}; the limit is {}",
                path.display(),
                size_text(meta.len()),
                size_text(MAX_SIZE)
            ));
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".into());
        Ok(Attached {
            path,
            name,
            size: meta.len(),
        })
    }

    /// Reads the piece starting at `offset`.
    pub fn read_chunk(&self, offset: u64) -> std::io::Result<Vec<u8>> {
        let mut f = std::fs::File::open(&self.path)?;
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = Vec::with_capacity(CHUNK);
        f.take(CHUNK as u64).read_to_end(&mut buf)?;
        Ok(buf)
    }
}

/// "3.2 MB", "120 KB", "512 B".
pub fn size_text(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f >= K * K {
        format!("{:.1} MB", f / K / K)
    } else if f >= K {
        format!("{:.0} KB", f / K)
    } else {
        format!("{n} B")
    }
}

/// Pasted text that's only paths of existing files, as terminals paste a
/// dropped file: plain, quoted, shell-escaped or as `file://` URIs, one or
/// several. Anything else is ordinary text to paste.
pub fn pasted_paths(text: &str) -> Option<Vec<PathBuf>> {
    let words = shell_words(text.trim())?;
    if words.is_empty() {
        return None;
    }
    let paths: Vec<PathBuf> = words.iter().map(|w| to_path(w)).collect::<Option<_>>()?;
    paths.iter().all(|p| p.is_file()).then_some(paths)
}

fn to_path(word: &str) -> Option<PathBuf> {
    let path = match word.strip_prefix("file://") {
        // file://host/path: only this machine's files.
        Some(rest) => {
            let rest = rest.strip_prefix("localhost").unwrap_or(rest);
            PathBuf::from(percent_decode(rest)?)
        }
        None => expand_home(Path::new(word)),
    };
    path.is_absolute().then_some(path)
}

/// Splits like a shell: whitespace separates words, quotes and backslashes
/// keep it. `None` for unbalanced quotes.
fn shell_words(s: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                cur.push(chars.next()?);
                in_word = true;
            }
            '\'' | '"' => {
                let q = c;
                loop {
                    match chars.next()? {
                        x if x == q => break,
                        x => cur.push(x),
                    }
                }
                in_word = true;
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Some(words)
}

fn percent_decode(s: &str) -> Option<String> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn expand_home(path: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => dirs::home_dir().map_or_else(|| path.to_path_buf(), |h| h.join(rest)),
        Err(_) => path.to_path_buf(),
    }
}

/// Tab completion for a typed path: extends it as far as the matching
/// names agree, adding `/` after a folder. Hidden files only when asked for.
pub fn complete(typed: &str) -> String {
    let (dir_part, prefix) = match typed.rfind('/') {
        Some(i) => (&typed[..=i], &typed[i + 1..]),
        None => ("", typed),
    };
    let dir = if dir_part.is_empty() {
        PathBuf::from(".")
    } else {
        expand_home(Path::new(dir_part))
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return typed.to_string();
    };
    let mut names: Vec<(String, bool)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (name.starts_with(prefix) && (prefix.starts_with('.') || !name.starts_with('.')))
                .then(|| (name, e.path().is_dir()))
        })
        .collect();
    names.sort();
    let Some((first, _)) = names.first() else {
        return typed.to_string();
    };
    let common = names.iter().fold(first.clone(), |acc, (n, _)| {
        acc.chars()
            .zip(n.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a)
            .collect()
    });
    let mut out = format!("{dir_part}{common}");
    if names.len() == 1 && names[0].1 {
        out.push('/');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "omabubble-attach-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn pasted_paths_in_every_form() {
        let d = tmp();
        let a = d.join("a photo.jpg");
        let b = d.join("b.pdf");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"y").unwrap();
        let (sa, sb) = (a.to_str().unwrap(), b.to_str().unwrap());

        assert_eq!(pasted_paths(&format!("'{sa}'")), Some(vec![a.clone()]));
        assert_eq!(
            pasted_paths(&sa.replace(' ', "\\ ")),
            Some(vec![a.clone()]),
            "shell-escaped"
        );
        assert_eq!(
            pasted_paths(&format!("file://{}", sa.replace(' ', "%20"))),
            Some(vec![a.clone()])
        );
        assert_eq!(
            pasted_paths(&format!("\"{sa}\"\n{sb}\n")),
            Some(vec![a.clone(), b.clone()]),
            "several, one per line"
        );
        assert_eq!(pasted_paths(sb), Some(vec![b.clone()]));

        // Ordinary text stays text.
        assert_eq!(pasted_paths("hello there"), None);
        assert_eq!(pasted_paths(&format!("see {sb}")), None);
        assert_eq!(pasted_paths(&d.join("missing.jpg").to_string_lossy()), None);
        assert_eq!(pasted_paths(d.to_str().unwrap()), None, "a folder");
        assert_eq!(pasted_paths("'unbalanced"), None);
        assert_eq!(pasted_paths("b.pdf"), None, "relative");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn attaching_checks_the_file() {
        let d = tmp();
        let f = d.join("doc.txt");
        std::fs::write(&f, b"hello").unwrap();
        let a = Attached::new(&f).unwrap();
        assert_eq!((a.name.as_str(), a.size), ("doc.txt", 5));
        assert_eq!(a.read_chunk(0).unwrap(), b"hello");
        assert_eq!(a.read_chunk(3).unwrap(), b"lo");
        std::fs::write(d.join("empty"), b"").unwrap();
        assert!(
            Attached::new(&d.join("empty"))
                .unwrap_err()
                .contains("empty")
        );
        assert!(Attached::new(&d).unwrap_err().contains("isn't a file"));
        assert!(Attached::new(&d.join("nope")).is_err());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn tab_completion() {
        let d = tmp();
        std::fs::create_dir(d.join("Pictures")).unwrap();
        std::fs::write(d.join("photo-1.jpg"), b"x").unwrap();
        std::fs::write(d.join("photo-2.jpg"), b"x").unwrap();
        std::fs::write(d.join(".hidden"), b"x").unwrap();
        let base = format!("{}/", d.display());
        assert_eq!(complete(&format!("{base}Pic")), format!("{base}Pictures/"));
        assert_eq!(complete(&format!("{base}pho")), format!("{base}photo-"));
        assert_eq!(
            complete(&format!("{base}x")),
            format!("{base}x"),
            "no match"
        );
        assert_eq!(complete(&format!("{base}.h")), format!("{base}.hidden"));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn sizes() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(120 * 1024), "120 KB");
        assert_eq!(size_text(3_355_443), "3.2 MB");
    }
}
