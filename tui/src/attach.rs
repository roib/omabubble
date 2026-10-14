//! Files attached to the next message: recognising pasted or dropped paths,
//! completing typed ones, and reading them for upload.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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

/// One name matching what's typed in the attach prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// The typed path's folder part ("~/Pictures/") and the name begun after it.
fn split_typed(typed: &str) -> (&str, &str) {
    match typed.rfind('/') {
        Some(i) => (&typed[..=i], &typed[i + 1..]),
        None => ("", typed),
    }
}

/// What `typed` would become with `name` chosen from its folder.
pub fn with_name(typed: &str, name: &str) -> String {
    format!("{}{name}", split_typed(typed).0)
}

/// The names in the typed folder that start with what's typed after it,
/// newest first: a screenshot just taken or a file just downloaded comes
/// first. Hidden files only when the name typed starts with a dot.
pub fn candidates(typed: &str) -> Vec<Candidate> {
    let (dir_part, prefix) = split_typed(typed);
    let dir = if dir_part.is_empty() {
        PathBuf::from(".")
    } else {
        expand_home(Path::new(dir_part))
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<Candidate> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with(prefix) || (name.starts_with('.') && !prefix.starts_with('.')) {
                return None;
            }
            // Follows links, so a link to a folder counts as a folder.
            let meta = std::fs::metadata(e.path()).ok()?;
            Some(Candidate {
                name,
                is_dir: meta.is_dir(),
                size: meta.len(),
                modified: meta.modified().ok(),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// "now", "2m ago", "15h ago", "3d ago".
pub fn age_text(t: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(t)
        .map_or(0, |d| d.as_secs());
    match secs {
        0..60 => "now".into(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
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
    fn candidates_newest_first() {
        let d = tmp();
        let set_age = |name: &str, secs_ago: u64| {
            let f = std::fs::File::options()
                .write(true)
                .open(d.join(name))
                .unwrap();
            f.set_modified(SystemTime::now() - std::time::Duration::from_secs(secs_ago))
                .unwrap();
        };
        for n in [
            "shot-old.png",
            "shot-new.png",
            "shot-mid.png",
            ".shot-hidden",
            "other.txt",
        ] {
            std::fs::write(d.join(n), b"xy").unwrap();
        }
        std::fs::create_dir(d.join("shots")).unwrap();
        set_age("shot-old.png", 3 * 86400);
        set_age("shot-mid.png", 3600);
        set_age("shot-new.png", 10);
        set_age("other.txt", 5);
        let base = format!("{}/", d.display());
        let names: Vec<String> = candidates(&format!("{base}shot"))
            .into_iter()
            .map(|c| c.name)
            .collect();
        // The folder was just made, so it's newest.
        assert_eq!(
            names,
            ["shots", "shot-new.png", "shot-mid.png", "shot-old.png"]
        );
        let c = &candidates(&format!("{base}shot-new"))[0];
        assert_eq!((c.is_dir, c.size), (false, 2));
        assert_eq!(
            candidates(&format!("{base}.shot")).len(),
            1,
            "hidden when asked"
        );
        assert!(candidates(&format!("{base}nothing")).is_empty());
        assert!(candidates("/no/such/folder/x").is_empty());
        assert_eq!(
            with_name(&format!("{base}sh"), "shots"),
            format!("{base}shots")
        );
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn ages() {
        let ago = |s| SystemTime::now() - std::time::Duration::from_secs(s);
        assert_eq!(age_text(ago(5)), "now");
        assert_eq!(age_text(ago(120)), "2m ago");
        assert_eq!(age_text(ago(15 * 3600)), "15h ago");
        assert_eq!(age_text(ago(3 * 86400)), "3d ago");
    }

    #[test]
    fn sizes() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(120 * 1024), "120 KB");
        assert_eq!(size_text(3_355_443), "3.2 MB");
    }
}
