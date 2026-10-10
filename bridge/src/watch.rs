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
    delivered: Option<i64>,
    read: Option<i64>,
}

impl Snapshot {
    fn of(m: &Message) -> Snapshot {
        Snapshot {
            text: m.text.clone(),
            edited: m.edited,
            unsent: m.unsent,
            is_error: m.is_error,
            delivered: m.delivered,
            read: m.read,
        }
    }
}

pub struct Watch {
    db: Db,
    last_seen: i64,
    /// rowid → (state last pushed, message ts) for recent rows.
    pushed: HashMap<i64, (Snapshot, i64)>,
    /// Apple-epoch ns when the previous pass started: reads after this are new.
    last_pass: i64,
    /// chat ROWID → newest message announced as read elsewhere.
    read_announced: HashMap<i64, i64>,
}

/// Overlap between passes when looking for reads, so one that lands while a
/// pass runs is still caught; repeats are filtered by `read_announced`.
const READ_SLACK_NS: i64 = 2_000_000_000;

impl Watch {
    pub fn new(db: Db) -> Result<Watch> {
        let last_seen = db.max_rowid()?;
        Ok(Watch {
            db,
            last_seen,
            pushed: HashMap::new(),
            // Reads from before startup are already in the chat list's counts.
            last_pass: crate::db::unix_to_apple_ns(now_unix()),
            read_announced: HashMap::new(),
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

        // Chats read on the phone (or anywhere Messages syncs): tell every
        // client, the same way a read in one imsg is shared with the others.
        let pass_start = crate::db::unix_to_apple_ns(now);
        for (chat_id, rowid) in self.db.read_since(floor, self.last_pass - READ_SLACK_NS)? {
            if self
                .read_announced
                .get(&chat_id)
                .is_some_and(|&r| r >= rowid)
            {
                continue;
            }
            self.read_announced.insert(chat_id, rowid);
            events.push(Event::Read {
                chat_ids: vec![chat_id],
                rowid,
            });
        }
        self.last_pass = pass_start;

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
               VALUES (40, 'G20', 'fresh', 1, 'iMessage', {d}, 0);
             INSERT INTO chat_message_join VALUES (1, 40, {d});
             INSERT INTO message (ROWID, guid, handle_id, date, item_type) VALUES (41, 'G21', 1, {d}, 1);
             INSERT INTO chat_message_join VALUES (1, 41, {d});
             INSERT INTO message (ROWID, guid, text, handle_id, service, date, is_from_me) \
               VALUES (42, 'G22', 'mine', 1, 'iMessage', {d}, 1);
             INSERT INTO chat_message_join VALUES (1, 42, {d});",
            d = ns_ago(10)
        ))
        .unwrap();
        let ev = w.tick().unwrap();
        assert_eq!(ev.len(), 2);
        assert!(
            matches!(&ev[0], Event::NewMessage { message } if message.rowid == 40 && message.text == "fresh")
        );
        assert!(
            matches!(&ev[1], Event::NewMessage { message } if message.rowid == 42 && message.delivered.is_none())
        );
        assert_eq!(
            w.last_seen, 42,
            "invisible system row still advances last_seen"
        );
        assert!(w.tick().unwrap().is_empty());

        rw.execute(
            &format!(
                "UPDATE message SET text = 'fresh!', date_edited = {} WHERE ROWID = 40",
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
                "UPDATE message SET text = NULL, date_retracted = {} WHERE ROWID = 40",
                ns_ago(0)
            ),
            [],
        )
        .unwrap();
        let ev = w.tick().unwrap();
        assert!(
            matches!(&ev[..], [Event::MessageUpdated { message }] if message.unsent && message.text.is_empty())
        );

        // Receipts arrive as updates too, each state once.
        rw.execute(
            &format!(
                "UPDATE message SET date_delivered = {} WHERE ROWID = 42",
                ns_ago(0)
            ),
            [],
        )
        .unwrap();
        let ev = w.tick().unwrap();
        assert!(
            matches!(&ev[..], [Event::MessageUpdated { message }] if message.rowid == 42 && message.delivered.is_some() && message.read.is_none()),
            "{ev:?}"
        );
        assert!(w.tick().unwrap().is_empty());
        rw.execute(
            &format!(
                "UPDATE message SET date_read = {} WHERE ROWID = 42",
                ns_ago(0)
            ),
            [],
        )
        .unwrap();
        let ev = w.tick().unwrap();
        assert!(matches!(&ev[..], [Event::MessageUpdated { message }] if message.read.is_some()));
    }

    #[test]
    fn reads_elsewhere_are_announced_once() {
        let path = fixture_db();
        let mut w = Watch::new(Db::open(&path).unwrap()).unwrap();
        assert!(
            w.tick().unwrap().is_empty(),
            "earlier reads aren't replayed"
        );

        let rw = Connection::open(&path).unwrap();
        rw.execute_batch(&format!(
            "INSERT INTO message (ROWID, guid, text, handle_id, service, date, is_from_me, is_read) \
               VALUES (50, 'G50', 'one', 1, 'iMessage', {d}, 0, 0), \
                      (51, 'G51', 'two', 1, 'iMessage', {d}, 0, 0), \
                      (52, 'G52', 'group', 2, 'iMessage', {d}, 0, 0);
             INSERT INTO chat_message_join VALUES (1, 50, {d}), (1, 51, {d}), (3, 52, {d});",
            d = ns_ago(30)
        ))
        .unwrap();
        let ev = w.tick().unwrap();
        assert_eq!(ev.len(), 3, "the three new messages");
        assert!(ev.iter().all(|e| matches!(e, Event::NewMessage { .. })));

        // Read on the phone: Messages sets is_read and date_read.
        rw.execute(
            &format!(
                "UPDATE message SET is_read = 1, date_read = {} WHERE ROWID IN (50, 51, 52)",
                ns_ago(0)
            ),
            [],
        )
        .unwrap();
        let mut ev = w.tick().unwrap();
        ev.sort_by_key(|e| match e {
            Event::Read { chat_ids, .. } => chat_ids[0],
            _ => 0,
        });
        assert!(
            matches!(&ev[..], [Event::Read { chat_ids: a, rowid: 51 }, Event::Read { chat_ids: b, rowid: 52 }]
                if a == &[1] && b == &[3]),
            "{ev:?}"
        );
        assert!(w.tick().unwrap().is_empty(), "announced once");

        // Reading an older message in a chat already announced further: nothing new.
        rw.execute(
            &format!(
                "UPDATE message SET is_read = 1, date_read = {} WHERE ROWID = 7",
                ns_ago(0)
            ),
            [],
        )
        .unwrap();
        assert!(w.tick().unwrap().is_empty());
    }
}
