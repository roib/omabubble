//! How far each chat has been read in a TUI, kept in a JSON file beside the socket.
//!
//! Messages never learns about these: they only lower the unread counts the
//! bridge reports, so they survive restarts and agree across clients.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct ReadMarks {
    path: PathBuf,
    /// chat ROWID → highest message ROWID read.
    upto: HashMap<i64, i64>,
}

impl ReadMarks {
    /// Loads `path`; a missing or unreadable file starts empty.
    pub fn load(path: PathBuf) -> ReadMarks {
        let upto = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<i64, i64>>(&s).ok())
            .unwrap_or_default();
        ReadMarks { path, upto }
    }

    #[cfg(test)]
    pub fn get(&self, chat_id: i64) -> i64 {
        self.upto.get(&chat_id).copied().unwrap_or(0)
    }

    pub fn all(&self) -> &HashMap<i64, i64> {
        &self.upto
    }

    /// Raises the marks for `chat_ids` to `rowid` and saves if any moved.
    /// Returns whether anything changed.
    pub fn mark(&mut self, chat_ids: &[i64], rowid: i64) -> Result<bool> {
        let mut changed = false;
        for &id in chat_ids {
            let e = self.upto.entry(id).or_insert(0);
            if rowid > *e {
                *e = rowid;
                changed = true;
            }
        }
        if changed {
            save(&self.path, &self.upto)?;
        }
        Ok(changed)
    }
}

/// Write to a temp file and rename, so a crash never leaves half a file.
fn save(path: &Path, upto: &HashMap<i64, i64>) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(upto)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_only_rise_and_persist() {
        let dir = std::env::temp_dir().join(format!("imsg-marks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("read.json");
        let _ = std::fs::remove_file(&path);

        let mut m = ReadMarks::load(path.clone());
        assert_eq!(m.get(1), 0);
        assert!(m.mark(&[1, 2], 10).unwrap());
        assert!(!m.mark(&[1], 5).unwrap(), "never moves back");
        assert!(!m.mark(&[1, 2], 10).unwrap());

        let m = ReadMarks::load(path.clone());
        assert_eq!((m.get(1), m.get(2), m.get(3)), (10, 10, 0));

        std::fs::write(&path, "garbage").unwrap();
        assert_eq!(ReadMarks::load(path).get(1), 0);
    }
}
