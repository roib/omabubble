//! Groups chat.db handles by person using the Mac's Contacts database.
//!
//! Messages.app merges a contact's phone and email chats into one
//! conversation via Contacts; chat.db keeps them as separate rows and leaves
//! `handle.person_centric_id` empty. This reads only each card's phone
//! numbers, email addresses and link id. Names are returned only on request.

use crate::log;
use anyhow::{Context, Result};
use proto::PersonGroup;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const DB_NAME: &str = "AddressBook-v22.abcddb";
/// Shorter digit strings are too ambiguous to match on.
const MIN_DIGITS: usize = 7;

pub fn default_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join("Library/Application Support/AddressBook"))
}

/// The local database plus one per synced source (iCloud, Google, …).
pub fn db_paths(dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let local = dir.join(DB_NAME);
    if local.is_file() {
        paths.push(local);
    }
    if let Ok(sources) = std::fs::read_dir(dir.join("Sources")) {
        let mut found: Vec<PathBuf> = sources
            .flatten()
            .map(|e| e.path().join(DB_NAME))
            .filter(|p| p.is_file())
            .collect();
        found.sort();
        paths.append(&mut found);
    }
    paths
}

#[derive(Debug, Default, Clone)]
pub struct Card {
    pub name: Option<String>,
    pub phones: Vec<String>,
    pub emails: Vec<String>,
}

/// Cards keyed by link id (cards Contacts shows as one person share it) or
/// primary key, merged across databases.
pub fn load_cards(paths: &[PathBuf]) -> Result<Vec<Card>> {
    let mut cards: HashMap<String, Card> = HashMap::new();
    for path in paths {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("opening {}", path.display()))?;
        let prefix = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned());

        let mut keys: HashMap<i64, String> = HashMap::new();
        let mut stmt = conn.prepare(
            "SELECT Z_PK, ZLINKID, ZFIRSTNAME, ZLASTNAME, ZORGANIZATION FROM ZABCDRECORD",
        )?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })? {
            let (pk, link, first, last, org) = row?;
            let key = match link.filter(|l| !l.is_empty()) {
                Some(l) => l,
                None => format!("{}:{pk}", prefix.as_deref().unwrap_or("local")),
            };
            let name = [first, last]
                .into_iter()
                .flatten()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let name = if name.is_empty() {
                org.filter(|o| !o.trim().is_empty())
            } else {
                Some(name)
            };
            let card = cards.entry(key.clone()).or_default();
            if card.name.is_none() {
                card.name = name;
            }
            keys.insert(pk, key);
        }

        let mut stmt = conn.prepare(
            "SELECT ZOWNER, ZFULLNUMBER FROM ZABCDPHONENUMBER WHERE ZFULLNUMBER IS NOT NULL",
        )?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (owner, number) = row?;
            if let Some(key) = keys.get(&owner) {
                cards.get_mut(key).unwrap().phones.push(number);
            }
        }
        let mut stmt = conn
            .prepare("SELECT ZOWNER, ZADDRESS FROM ZABCDEMAILADDRESS WHERE ZADDRESS IS NOT NULL")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (owner, address) = row?;
            if let Some(key) = keys.get(&owner) {
                cards
                    .get_mut(key)
                    .unwrap()
                    .emails
                    .push(address.trim().to_lowercase());
            }
        }
    }
    let mut out: Vec<Card> = cards.into_values().collect();
    out.retain(|c| !c.phones.is_empty() || !c.emails.is_empty());
    Ok(out)
}

fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

/// Numbers match when one's digits end with the other's: Contacts stores
/// them without a country code as often as with one.
fn same_number(a: &str, b: &str) -> bool {
    let (a, b) = (digits(a), digits(b));
    a.len() >= MIN_DIGITS && b.len() >= MIN_DIGITS && (a.ends_with(&b) || b.ends_with(&a))
}

fn card_has(card: &Card, handle: &str) -> bool {
    if handle.contains('@') {
        let h = handle.trim().to_lowercase();
        card.emails.contains(&h)
    } else {
        card.phones.iter().any(|p| same_number(p, handle))
    }
}

/// Which chat.db handles share a card. Without `names`, only groups of two
/// or more are returned; with it, every matched handle comes with its name.
pub fn groups(cards: &[Card], handles: &[String], names: bool) -> Vec<PersonGroup> {
    let mut by_card: HashMap<usize, Vec<String>> = HashMap::new();
    for h in handles {
        if let Some(i) = cards.iter().position(|c| card_has(c, h)) {
            by_card.entry(i).or_default().push(h.clone());
        }
    }
    let mut out: Vec<PersonGroup> = by_card
        .into_iter()
        .filter(|(i, hs)| hs.len() > 1 || (names && cards[*i].name.is_some()))
        .map(|(i, mut handles)| {
            handles.sort();
            PersonGroup {
                handles,
                name: if names { cards[i].name.clone() } else { None },
            }
        })
        .collect();
    out.sort_by(|a, b| a.handles.cmp(&b.handles));
    out
}

/// Groups for the given handles, or none (with one log line) when Contacts
/// can't be read.
pub fn groups_for(dir: Option<&Path>, handles: &[String], names: bool) -> Vec<PersonGroup> {
    let Some(dir) = dir else { return Vec::new() };
    let paths = db_paths(dir);
    if paths.is_empty() {
        return Vec::new();
    }
    match load_cards(&paths) {
        Ok(cards) => groups(&cards, handles, names),
        Err(e) => {
            log!("contacts unavailable, chats won't be merged by person: {e:#}");
            Vec::new()
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Build the Contacts fixture in a temp dir laid out like ~/Library/Application Support/AddressBook.
    pub fn fixture_dir() -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("imsg-contacts-{}-{n}", std::process::id()));
        let src = dir.join("Sources/ABCD-1234");
        std::fs::create_dir_all(&src).unwrap();
        let conn = Connection::open(src.join(DB_NAME)).unwrap();
        conn.execute_batch(include_str!("../tests/fixtures/make_contacts.sql"))
            .unwrap();
        dir
    }

    fn handles() -> Vec<String> {
        [
            "+15550000001",
            "+15550000002",
            "+15550000003",
            "friend@example.com",
            "+15550000004",
            "+442079460958",
            "nobody@example.com",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn number_matching() {
        assert!(same_number("(555) 000-0001", "+15550000001"));
        assert!(same_number("+1 555 000 0001", "5550000001"));
        assert!(
            !same_number("020 7946 0958", "+442079460958"),
            "trunk 0 differs; Contacts usually stores +44"
        );
        assert!(same_number("+44 20 7946 0958", "+442079460958"));
        assert!(!same_number("0001", "+15550000001"), "too short");
        assert!(!same_number("+15550000001", "+15550000002"));
    }

    #[test]
    fn groups_handles_by_card() {
        let dir = fixture_dir();
        let paths = db_paths(&dir);
        assert_eq!(paths.len(), 1);
        let cards = load_cards(&paths).unwrap();
        assert_eq!(
            cards.len(),
            5,
            "cards without any handle are dropped; linked cards count once"
        );

        let g = groups(&cards, &handles(), false);
        assert_eq!(g.len(), 2, "{g:?}");
        assert_eq!(g[0].handles, vec!["+15550000001", "friend@example.com"]);
        assert_eq!(g[0].name, None);
        assert_eq!(
            g[1].handles,
            vec!["+15550000002", "+15550000004"],
            "two linked cards"
        );

        let named = groups(&cards, &handles(), true);
        assert_eq!(named.len(), 4);
        assert_eq!(named[0].name.as_deref(), Some("Alex Example"));
        assert!(
            named
                .iter()
                .any(|p| p.handles == ["+15550000003"] && p.name.as_deref() == Some("Acme Corp"))
        );
        assert!(
            named
                .iter()
                .any(|p| p.handles == ["+442079460958"] && p.name.as_deref() == Some("Sam"))
        );
        assert!(
            !named
                .iter()
                .any(|p| p.handles.iter().any(|h| h == "nobody@example.com"))
        );
    }

    #[test]
    fn missing_contacts_dir_is_empty() {
        assert!(
            groups_for(
                Some(Path::new("/nonexistent/AddressBook")),
                &handles(),
                false
            )
            .is_empty()
        );
        assert!(groups_for(None, &handles(), false).is_empty());
    }
}
