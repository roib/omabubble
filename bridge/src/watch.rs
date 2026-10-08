//! Watches chat.db for changes and broadcasts new and updated messages.

use crate::db::{Db, now_unix};
use crate::log;
use crate::server::Hub;
use anyhow::{Context, Result};
use notify::{RecursiveMode, Watcher};
use proto::{Event, Message};
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

const FALLBACK_POLL: Duration = Duration::from_secs(5);
const DEBOUNCE: Duration = Duration::from_millis(100);
/// Edits are allowed for 15 minutes and unsends for 2; look a little further back.
const CHANGE_WINDOW_SECS: i64 = 20 * 60;
/// How far below `last_seen` (in ROWIDs) the change scan reaches.
const CHANGE_ROWID_SPAN: i64 = 5_000;

#[derive(PartialEq)]
struct Snapshot {
    text: String,
    edited: bool,
    unsent: bool,
    is_error: bool,
}

impl Snapshot {
    fn of(m: &Message) -> Snapshot {
        Snapshot {
            text: m.text.clone(),
            edited: m.edited,
            unsent: m.unsent,
            is_error: m.is_error,
        }
    }
}

pub struct Watch {
    db: Db,
    last_seen: i64,
    /// rowid → (state last pushed, message ts) for recent rows.
    pushed: HashMap<i64, (Snapshot, i64)>,
}

impl Watch {
    pub fn new(db: Db) -> Result<Watch> {
        let last_seen = db.max_rowid()?;
        Ok(Watch {
            db,
            last_seen,
            pushed: HashMap::new(),
        })
    }

    /// One pass: new rows first, then changes to rows already seen.
    pub fn tick(&mut self) -> Result<Vec<Event>> {
        let mut events = Vec::new();

        // Read MAX(ROWID) before scanning so rows that are invisible to `since`
        // still advance `last_seen`, without skipping anything inserted meanwhile.
        let max_before = self.db.max_rowid()?;
        for m in self.db.since(self.last_seen, 1000)? {
            self.last_seen = self.last_seen.max(m.rowid);
            self.pushed.insert(m.rowid, (Snapshot::of(&m), m.ts));
            events.push(Event::NewMessage { message: m });
        }
        self.last_seen = self.last_seen.max(max_before);

        let now = now_unix();
        let floor = (self.last_seen - CHANGE_ROWID_SPAN).max(0);
        for m in self
            .db
            .recent_changes(floor, self.last_seen, now - CHANGE_WINDOW_SECS)?
        {
            let snap = Snapshot::of(&m);
            if self.pushed.get(&m.rowid).is_some_and(|(s, _)| *s == snap) {
                continue;
            }
            self.pushed.insert(m.rowid, (snap, m.ts));
            events.push(Event::MessageUpdated { message: m });
        }

        self.pushed
            .retain(|_, (_, ts)| *ts > now - CHANGE_WINDOW_SECS - 300);
        Ok(events)
    }
}

/// Runs forever: FSEvents (or inotify) wakeups plus a fallback poll.
pub fn run(db_path: &Path, hub: &Hub) -> Result<()> {
    let mut watch = Watch::new(Db::open(db_path)?)?;
    let (tx, rx) = channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.send(());
        }
    })?;
    let dir = db_path
        .parent()
        .context("db path has no parent directory")?;
    if let Err(e) = watcher.watch(dir, RecursiveMode::NonRecursive) {
        log!(
            "file watcher unavailable ({e}); polling every {}s only",
            FALLBACK_POLL.as_secs()
        );
    }

    loop {
        match rx.recv_timeout(FALLBACK_POLL) {
            Ok(()) => {
                std::thread::sleep(DEBOUNCE);
                while rx.try_recv().is_ok() {}
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(FALLBACK_POLL),
        }
        match watch.tick() {
            Ok(events) => {
                for ev in events {
                    hub.broadcast(&serde_json::to_string(&ev)?);
                }
            }
            Err(e) => log!("watch tick failed: {e:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::fixture_db;
    use rusqlite::Connection;

    fn ns_ago(secs: i64) -> i64 {
        crate::db::unix_to_apple_ns(now_unix() - secs)
    }

    #[test]
    fn pushes_new_then_updates_once() {
        let path = fixture_db();
        let mut w = Watch::new(Db::open(&path).unwrap()).unwrap();
        assert!(
            w.tick().unwrap().is_empty(),
            "fixture changes are older than the window"
        );

        let rw = Connection::open(&path).unwrap();
        rw.execute_batch(&format!(
            "INSERT INTO message (ROWID, guid, text, handle_id, service, date, is_from_me) \
               VALUES (20, 'G20', 'fresh', 1, 'iMessage', {d}, 0);
             INSERT INTO chat_message_join VALUES (1, 20, {d});
             INSERT INTO message (ROWID, guid, handle_id, date, item_type) VALUES (21, 'G21', 1, {d}, 1);
             INSERT INTO chat_message_join VALUES (1, 21, {d});",
            d = ns_ago(10)
        ))
        .unwrap();
        let ev = w.tick().unwrap();
        assert_eq!(ev.len(), 1);
        assert!(
            matches!(&ev[0], Event::NewMessage { message } if message.rowid == 20 && message.text == "fresh")
        );
        assert_eq!(
            w.last_seen, 21,
            "invisible system row still advances last_seen"
        );
        assert!(w.tick().unwrap().is_empty());

        rw.execute(
            &format!(
                "UPDATE message SET text = 'fresh!', date_edited = {} WHERE ROWID = 20",
                ns_ago(1)
            ),
            [],
        )
        .unwrap();
        let ev = w.tick().unwrap();
        assert!(
            matches!(&ev[..], [Event::MessageUpdated { message }] if message.edited && message.text == "fresh!")
        );
        assert!(
            w.tick().unwrap().is_empty(),
            "same state is not pushed twice"
        );

        rw.execute(
            &format!(
                "UPDATE message SET text = NULL, date_retracted = {} WHERE ROWID = 20",
                ns_ago(0)
            ),
            [],
        )
        .unwrap();
        let ev = w.tick().unwrap();
        assert!(
            matches!(&ev[..], [Event::MessageUpdated { message }] if message.unsent && message.text.is_empty())
        );
    }
}
