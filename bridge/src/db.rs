//! Read-only access to the Messages database (chat.db).

use anyhow::{Context, Result, bail};
use proto::{Attachment, Chat, Kind, Message, Tapback};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

/// 2001-01-01T00:00:00Z as a Unix timestamp.
pub const APPLE_EPOCH: i64 = 978_307_200;

/// `message.date` is nanoseconds since 2001 on modern macOS, seconds on very old databases.
pub fn apple_to_unix(d: i64) -> i64 {
    if d.abs() > 1_000_000_000_000 {
        d / 1_000_000_000 + APPLE_EPOCH
    } else {
        d + APPLE_EPOCH
    }
}

pub fn unix_to_apple_ns(unix: i64) -> i64 {
    (unix - APPLE_EPOCH) * 1_000_000_000
}

/// Rows the bridge shows: normal messages and tapbacks. Skips system rows
/// (participant changes, renames) and tapback removals / stickers.
const VISIBLE: &str = "m.item_type = 0 AND (m.associated_message_type = 0 OR m.associated_message_type BETWEEN 2000 AND 2999)";

/// Columns the bridge cannot work without, by table.
const REQUIRED: &[(&str, &[&str])] = &[
    (
        "message",
        &[
            "ROWID",
            "guid",
            "text",
            "handle_id",
            "service",
            "error",
            "date",
            "is_from_me",
            "is_read",
            "cache_has_attachments",
            "item_type",
            "associated_message_guid",
            "associated_message_type",
            "attributedBody",
        ],
    ),
    (
        "chat",
        &[
            "ROWID",
            "guid",
            "style",
            "chat_identifier",
            "service_name",
            "display_name",
        ],
    ),
    ("handle", &["ROWID", "id"]),
    ("chat_handle_join", &["chat_id", "handle_id"]),
    (
        "chat_message_join",
        &["chat_id", "message_id", "message_date"],
    ),
    (
        "attachment",
        &["ROWID", "filename", "mime_type", "transfer_name"],
    ),
    ("message_attachment_join", &["message_id", "attachment_id"]),
];

pub struct Db {
    conn: Connection,
    msg_select: String,
    edited_col: &'static str,
    retracted_col: &'static str,
}

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("opening {}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(2))?;

        let mut missing = Vec::new();
        let mut message_cols = HashSet::new();
        for (table, cols) in REQUIRED {
            let have = table_columns(&conn, table).with_context(|| {
                format!(
                    "reading schema of {} (Full Disk Access granted?)",
                    path.display()
                )
            })?;
            for c in *cols {
                if !have.contains(&c.to_ascii_lowercase()) {
                    missing.push(format!("{table}.{c}"));
                }
            }
            if *table == "message" {
                message_cols = have;
            }
        }
        if !missing.is_empty() {
            bail!(
                "chat.db is missing columns this bridge needs: {}",
                missing.join(", ")
            );
        }

        // Newer columns: fall back to constants when absent.
        let edited_col = if message_cols.contains("date_edited") {
            "m.date_edited"
        } else {
            "0"
        };
        let retracted_col = if message_cols.contains("date_retracted") {
            "m.date_retracted"
        } else {
            "0"
        };
        let emoji_col = if message_cols.contains("associated_message_emoji") {
            "m.associated_message_emoji"
        } else {
            "NULL"
        };
        let msg_select = format!(
            "SELECT m.ROWID, cmj.chat_id, h.id, m.is_from_me, m.text, m.attributedBody, m.date, m.service, \
                    m.error, {edited_col}, {retracted_col}, m.cache_has_attachments, \
                    m.associated_message_type, m.associated_message_guid, {emoji_col} \
             FROM message m \
             JOIN chat_message_join cmj ON cmj.message_id = m.ROWID \
             LEFT JOIN handle h ON h.ROWID = m.handle_id"
        );
        Ok(Db {
            conn,
            msg_select,
            edited_col,
            retracted_col,
        })
    }

    /// The other person's handle when `guid` is a one-on-one chat.
    pub fn direct_handle(&self, guid: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT h.id FROM chat c \
             JOIN chat_handle_join chj ON chj.chat_id = c.ROWID \
             JOIN handle h ON h.ROWID = chj.handle_id WHERE c.guid = ?1",
        )?;
        let mut handles: Vec<String> = stmt
            .query_map([guid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(if handles.len() == 1 {
            handles.pop()
        } else {
            None
        })
    }

    /// Every handle id (phone number or email) chat.db knows.
    pub fn handle_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT DISTINCT id FROM handle ORDER BY id")?;
        let ids = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    pub fn max_rowid(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT IFNULL(MAX(ROWID), 0) FROM message", [], |r| {
                r.get(0)
            })?)
    }

    pub fn list_chats(&self, limit: u32) -> Result<Vec<Chat>> {
        self.list_chats_at(limit, now_unix())
    }

    fn list_chats_at(&self, limit: u32, now: i64) -> Result<Vec<Chat>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT c.ROWID, c.guid, c.style, c.service_name, c.display_name, c.chat_identifier, \
                    (SELECT MAX(message_date) FROM chat_message_join WHERE chat_id = c.ROWID) AS last_date \
             FROM chat c WHERE last_date IS NOT NULL ORDER BY last_date DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Unread counts only look at the last 30 days to keep this cheap on big databases.
        let unread_floor = unix_to_apple_ns(now - 30 * 86_400);
        let mut chats = Vec::with_capacity(rows.len());
        for (chat_id, guid, style, service, display_name, identifier) in rows {
            let handles = self.chat_handles(chat_id)?;
            let display = match display_name.filter(|d| !d.is_empty()) {
                Some(d) => d,
                None if !handles.is_empty() => handles.join(", "),
                None => identifier.unwrap_or_else(|| guid.clone()),
            };
            let unread: u32 = self.conn.prepare_cached(
                "SELECT COUNT(*) FROM chat_message_join cmj JOIN message m ON m.ROWID = cmj.message_id \
                 WHERE cmj.chat_id = ?1 AND cmj.message_date > ?2 AND m.is_from_me = 0 AND m.is_read = 0 \
                   AND m.item_type = 0 AND m.associated_message_type = 0",
            )?
            .query_row(params![chat_id, unread_floor], |r| r.get(0))?;
            let last = self.query_messages(
                &format!("WHERE cmj.chat_id = ?1 AND {VISIBLE} ORDER BY cmj.message_date DESC, m.ROWID DESC LIMIT 1"),
                params![chat_id],
            )?
            .pop();
            chats.push(Chat {
                chat_id,
                guid,
                is_group: style == Some(43) || handles.len() > 1,
                display,
                service: service.unwrap_or_else(|| "iMessage".into()),
                handles,
                unread,
                last,
                chat_ids: Vec::new(),
            });
        }
        Ok(chats)
    }

    /// Up to `limit` messages of a chat older than `before_rowid`, oldest first.
    pub fn history(
        &self,
        chat_ids: &[i64],
        before_rowid: Option<i64>,
        limit: u32,
    ) -> Result<Vec<Message>> {
        if chat_ids.is_empty() {
            return Ok(Vec::new());
        }
        // Integers only, so inlining the list is safe.
        let ids = chat_ids
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let mut msgs = self.query_messages(
            &format!(
                "WHERE cmj.chat_id IN ({ids}) AND (?1 IS NULL OR m.ROWID < ?1) AND {VISIBLE} \
                 ORDER BY m.ROWID DESC LIMIT ?2"
            ),
            params![before_rowid, limit],
        )?;
        msgs.reverse();
        Ok(msgs)
    }

    /// Messages with ROWID greater than `rowid`, oldest first.
    pub fn since(&self, rowid: i64, limit: u32) -> Result<Vec<Message>> {
        self.query_messages(
            &format!("WHERE m.ROWID > ?1 AND {VISIBLE} ORDER BY m.ROWID ASC LIMIT ?2"),
            params![rowid, limit],
        )
    }

    /// Already-seen rows from the recent past that were edited, unsent, or failed.
    /// Bounded by ROWID range and date so it stays a small scan.
    pub fn recent_changes(
        &self,
        floor_rowid: i64,
        last_seen: i64,
        since_unix: i64,
    ) -> Result<Vec<Message>> {
        let (e, r) = (self.edited_col, self.retracted_col);
        self.query_messages(
            &format!(
                "WHERE m.ROWID > ?1 AND m.ROWID <= ?2 AND m.date > ?3 AND {VISIBLE} \
                   AND ({e} > 0 OR {r} > 0 OR (m.is_from_me = 1 AND m.error != 0)) \
                 ORDER BY m.ROWID ASC"
            ),
            params![floor_rowid, last_seen, unix_to_apple_ns(since_unix)],
        )
    }

    fn chat_handles(&self, chat_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT h.id FROM chat_handle_join chj JOIN handle h ON h.ROWID = chj.handle_id \
             WHERE chj.chat_id = ?1 ORDER BY h.ROWID",
        )?;
        let handles = stmt
            .query_map([chat_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(handles)
    }

    fn query_messages(&self, tail: &str, params: impl rusqlite::Params) -> Result<Vec<Message>> {
        let sql = format!("{} {tail}", self.msg_select);
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let raws = stmt
            .query_map(params, RawMsg::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raws.into_iter().map(|raw| self.finish(raw)).collect()
    }

    fn finish(&self, raw: RawMsg) -> Result<Message> {
        let edited = raw.date_edited != 0;
        let unsent = raw.date_retracted != 0;
        let decoded = raw
            .attributed_body
            .as_deref()
            .and_then(decode_attributed_body);
        let plain = raw.text.filter(|t| !t.is_empty());
        // After an edit attributedBody holds the newest text; prefer it then.
        let mut text = if edited {
            decoded.or(plain)
        } else {
            plain.or(decoded)
        }
        .unwrap_or_default();
        text = text.replace('\u{FFFC}', "");
        if raw.has_attachments {
            text = text.trim().to_string();
        }
        if unsent {
            text.clear();
        }

        let attachments = if raw.has_attachments && !unsent {
            self.attachments(raw.rowid)?
        } else {
            Vec::new()
        };

        let tapback = if (2000..3000).contains(&raw.assoc_type) {
            let target_text = match raw.assoc_guid.as_deref() {
                Some(g) => self.target_text(strip_assoc_guid(g))?,
                None => String::new(),
            };
            Some(Tapback {
                emoji: tapback_emoji(raw.assoc_type, raw.assoc_emoji.as_deref()),
                target_text,
            })
        } else {
            None
        };

        let kind = if tapback.is_some() {
            Kind::Tapback
        } else if text.is_empty() && !attachments.is_empty() {
            Kind::Attachment
        } else {
            Kind::Text
        };

        Ok(Message {
            rowid: raw.rowid,
            chat_id: raw.chat_id,
            sender: if raw.from_me { None } else { raw.sender },
            from_me: raw.from_me,
            text,
            ts: apple_to_unix(raw.date),
            service: raw.service.unwrap_or_else(|| "iMessage".into()),
            is_error: raw.error != 0,
            edited,
            unsent,
            kind,
            attachments,
            tapback,
        })
    }

    fn attachments(&self, rowid: i64) -> Result<Vec<Attachment>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT a.transfer_name, a.filename, a.mime_type FROM message_attachment_join maj \
             JOIN attachment a ON a.ROWID = maj.attachment_id WHERE maj.message_id = ?1 ORDER BY a.ROWID",
        )?;
        let atts = stmt
            .query_map([rowid], |r| {
                let transfer: Option<String> = r.get(0)?;
                let filename: Option<String> = r.get(1)?;
                let name = transfer
                    .filter(|n| !n.is_empty())
                    .or_else(|| {
                        filename
                            .as_deref()
                            .and_then(|f| f.rsplit('/').next())
                            .map(String::from)
                    })
                    .unwrap_or_else(|| "attachment".into());
                Ok(Attachment {
                    name,
                    mime: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(atts)
    }

    fn target_text(&self, guid: &str) -> Result<String> {
        let row: Option<(Option<String>, Option<Vec<u8>>)> = self
            .conn
            .prepare_cached("SELECT text, attributedBody FROM message WHERE guid = ?1")?
            .query_row([guid], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let text = row
            .and_then(|(t, ab)| {
                t.filter(|t| !t.is_empty())
                    .or_else(|| ab.as_deref().and_then(decode_attributed_body))
            })
            .unwrap_or_default()
            .replace('\u{FFFC}', "");
        Ok(truncate_chars(text.trim(), 60))
    }
}

struct RawMsg {
    rowid: i64,
    chat_id: i64,
    sender: Option<String>,
    from_me: bool,
    text: Option<String>,
    attributed_body: Option<Vec<u8>>,
    date: i64,
    service: Option<String>,
    error: i64,
    date_edited: i64,
    date_retracted: i64,
    has_attachments: bool,
    assoc_type: i64,
    assoc_guid: Option<String>,
    assoc_emoji: Option<String>,
}

impl RawMsg {
    fn from_row(r: &Row) -> rusqlite::Result<RawMsg> {
        Ok(RawMsg {
            rowid: r.get(0)?,
            chat_id: r.get(1)?,
            sender: r.get(2)?,
            from_me: r.get::<_, i64>(3)? != 0,
            text: r.get(4)?,
            attributed_body: r.get(5)?,
            date: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
            service: r.get(7)?,
            error: r.get::<_, Option<i64>>(8)?.unwrap_or(0),
            date_edited: r.get::<_, Option<i64>>(9)?.unwrap_or(0),
            date_retracted: r.get::<_, Option<i64>>(10)?.unwrap_or(0),
            has_attachments: r.get::<_, Option<i64>>(11)?.unwrap_or(0) != 0,
            assoc_type: r.get::<_, Option<i64>>(12)?.unwrap_or(0),
            assoc_guid: r.get(13)?,
            assoc_emoji: r.get(14)?,
        })
    }
}

fn table_columns(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let cols = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .map(|c| c.map(|c| c.to_ascii_lowercase()))
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    // PRAGMA table_info omits the implicit rowid alias.
    let mut cols = cols;
    cols.insert("rowid".into());
    Ok(cols)
}

/// `associated_message_guid` looks like `p:0/GUID` or `bp:GUID`.
fn strip_assoc_guid(g: &str) -> &str {
    match g.rsplit_once('/') {
        Some((_, rest)) => rest,
        None => g.strip_prefix("bp:").unwrap_or(g),
    }
}

fn tapback_emoji(kind: i64, custom: Option<&str>) -> String {
    match kind {
        2000 => "❤️",
        2001 => "👍",
        2002 => "👎",
        2003 => "😂",
        2004 => "‼️",
        2005 => "❓",
        2006 => return custom.filter(|e| !e.is_empty()).unwrap_or("🙂").to_string(),
        2007 => "[sticker]",
        _ => "•",
    }
    .to_string()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

/// Pull the plain text out of an `attributedBody` typedstream blob.
///
/// The string follows the `NSString` class name: a few type bytes ending in `+`,
/// then a length (one byte, or `0x81` + u16 LE, or `0x82` + u32 LE), then UTF-8.
pub fn decode_attributed_body(blob: &[u8]) -> Option<String> {
    const MARKER: &[u8] = b"NSString";
    let start = blob.windows(MARKER.len()).position(|w| w == MARKER)? + MARKER.len();
    let rest = &blob[start..];
    let plus = rest.iter().take(16).position(|&b| b == b'+')?;
    let mut i = plus + 1;
    let len = match *rest.get(i)? {
        0x81 => {
            let len = u16::from_le_bytes([*rest.get(i + 1)?, *rest.get(i + 2)?]) as usize;
            i += 3;
            len
        }
        0x82 => {
            let b = rest.get(i + 1..i + 5)?;
            i += 5;
            u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize
        }
        n => {
            i += 1;
            n as usize
        }
    };
    let bytes = rest.get(i..i + len)?;
    Some(String::from_utf8_lossy(bytes).into_owned())
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Build the fixture database in a fresh temp file and return its path.
    pub fn fixture_db() -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("imsg-fixture-{}-{n}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(include_str!("../tests/fixtures/make_fixture.sql"))
            .unwrap();
        path
    }

    fn blob(text: &str) -> Vec<u8> {
        let mut b = b"\x04\x0bstreamtyped\x81\xe8\x03\x84\x01@\x84\x84\x84\x12NSAttributedString\x00\x84\x84\x08NSObject\x00\x85\x92\x84\x84\x84\x08NSString\x01\x94\x84\x01+".to_vec();
        let t = text.as_bytes();
        match t.len() {
            n if n < 0x80 => b.push(n as u8),
            n if n <= u16::MAX as usize => {
                b.push(0x81);
                b.extend_from_slice(&(n as u16).to_le_bytes());
            }
            n => {
                b.push(0x82);
                b.extend_from_slice(&(n as u32).to_le_bytes());
            }
        }
        b.extend_from_slice(t);
        b.extend_from_slice(b"\x86\x84\x02iI\x01 \x92\x84\x84\x84");
        b
    }

    #[test]
    fn timestamps() {
        assert_eq!(apple_to_unix(0), APPLE_EPOCH);
        assert_eq!(
            apple_to_unix(781_436_060_000_000_000),
            781_436_060 + APPLE_EPOCH
        );
        assert_eq!(apple_to_unix(781_436_060), 781_436_060 + APPLE_EPOCH); // legacy seconds
        assert_eq!(
            apple_to_unix(unix_to_apple_ns(1_791_369_060)),
            1_791_369_060
        );
    }

    #[test]
    fn decodes_attributed_body() {
        assert_eq!(
            decode_attributed_body(&blob("hi 👋")).as_deref(),
            Some("hi 👋")
        );
        let long = "x".repeat(300);
        assert_eq!(
            decode_attributed_body(&blob(&long)).as_deref(),
            Some(long.as_str())
        );
        let huge = "y".repeat(70_000);
        assert_eq!(
            decode_attributed_body(&blob(&huge)).as_deref(),
            Some(huge.as_str())
        );
        assert_eq!(decode_attributed_body(b"garbage"), None);
        let cut = blob("this text gets cut off");
        assert_eq!(
            decode_attributed_body(&cut[..cut.len() - 15]),
            None,
            "length runs past the end"
        );
    }

    #[test]
    fn lists_chats() {
        let db = Db::open(&fixture_db()).unwrap();
        let chats = db.list_chats_at(100, 1_791_460_800).unwrap();
        let ids: Vec<i64> = chats.iter().map(|c| c.chat_id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5], "sorted by newest message");

        let c1 = &chats[0];
        assert_eq!(c1.display, "+15550000001");
        assert!(!c1.is_group);
        assert_eq!(c1.handles, vec!["+15550000001"]);
        assert_eq!(c1.last.as_ref().unwrap().rowid, 9);

        let sms = &chats[1];
        assert_eq!(sms.service, "SMS");
        assert_eq!(sms.unread, 1);
        assert_eq!(c1.unread, 2);
        assert_eq!(
            db.list_chats_at(100, 1_791_460_800 + 60 * 86_400).unwrap()[0].unread,
            0,
            "outside 30-day window"
        );

        let group = &chats[2];
        assert!(group.is_group);
        assert_eq!(group.unread, 1);
        assert_eq!(group.display, "+15550000002, +15550000004");
        assert_eq!(
            group.last.as_ref().unwrap().rowid,
            15,
            "system row and tapback removal skipped"
        );
        assert_eq!(chats[4].display, "Weekend Crew");
    }

    #[test]
    fn reads_history() {
        let db = Db::open(&fixture_db()).unwrap();
        let h = db.history(&[1], None, 50).unwrap();
        assert_eq!(
            h.iter().map(|m| m.rowid).collect::<Vec<_>>(),
            (1..=9).collect::<Vec<_>>()
        );

        let tap = &h[4];
        assert_eq!(tap.kind, Kind::Tapback);
        assert!(tap.from_me && tap.sender.is_none());
        assert_eq!(tap.tapback.as_ref().unwrap().emoji, "❤️");
        assert_eq!(
            tap.tapback.as_ref().unwrap().target_text,
            "what time should we leave"
        );

        assert!(h[5].edited);
        assert_eq!(h[7].text, "this one only has attributedBody 📎");
        assert!(h[8].unsent && h[8].text.is_empty());
        assert_eq!(h[0].sender.as_deref(), Some("+15550000001"));
        assert_eq!(h[0].ts, 1_791_369_060);

        let older = db.history(&[1], Some(4), 2).unwrap();
        assert_eq!(
            older.iter().map(|m| m.rowid).collect::<Vec<_>>(),
            vec![2, 3]
        );

        let failed = db.history(&[2], None, 50).unwrap();
        assert!(failed[1].is_error && failed[1].from_me);

        let att = &db.history(&[4], None, 50).unwrap()[0];
        assert_eq!(att.kind, Kind::Attachment);
        assert_eq!(att.text, "");
        assert_eq!(att.attachments[0].name, "IMG_1234.heic");
        assert_eq!(att.attachments[0].mime.as_deref(), Some("image/heic"));

        let group = db.history(&[3], None, 50).unwrap();
        assert_eq!(
            group.iter().map(|m| m.rowid).collect::<Vec<_>>(),
            vec![12, 13, 14, 15]
        );

        // Several chat rows (one person with two handles) read as one timeline.
        let merged = db.history(&[1, 2], Some(12), 4).unwrap();
        assert_eq!(
            merged.iter().map(|m| m.rowid).collect::<Vec<_>>(),
            vec![8, 9, 10, 11]
        );
        assert!(db.history(&[], None, 10).unwrap().is_empty());
    }

    #[test]
    fn direct_handle_only_for_one_on_one() {
        let db = Db::open(&fixture_db()).unwrap();
        assert_eq!(
            db.direct_handle("iMessage;-;friend@example.com")
                .unwrap()
                .as_deref(),
            Some("friend@example.com")
        );
        assert_eq!(
            db.direct_handle("iMessage;+;chat000000000000000001")
                .unwrap(),
            None
        );
        assert_eq!(db.direct_handle("nope").unwrap(), None);
    }

    #[test]
    fn since_and_changes() {
        let db = Db::open(&fixture_db()).unwrap();
        assert_eq!(db.max_rowid().unwrap(), 19);
        assert_eq!(db.handle_ids().unwrap().len(), 5);
        let s = db.since(14, 100).unwrap();
        assert_eq!(
            s.iter().map(|m| m.rowid).collect::<Vec<_>>(),
            vec![15, 18, 19]
        );
        // Everything in the fixture is in the past relative to this window start.
        let changed = db.recent_changes(0, 19, 1_791_300_000).unwrap();
        assert_eq!(
            changed.iter().map(|m| m.rowid).collect::<Vec<_>>(),
            vec![6, 9, 11]
        );
    }

    #[test]
    fn reports_missing_columns() {
        let path = std::env::temp_dir().join(format!("imsg-bad-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE message (ROWID INTEGER PRIMARY KEY, guid TEXT);")
            .unwrap();
        let err = Db::open(&path).err().unwrap().to_string();
        assert!(err.contains("message.text"), "{err}");
    }
}
