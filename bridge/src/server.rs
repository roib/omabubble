//! `imsg-bridge serve`: Unix socket server with one thread per client.

use crate::contacts;
use crate::db::Db;
use crate::log;
use crate::send::{Sender, Target};
use crate::watch;
use anyhow::{Context, Result, bail};
use proto::{Command, Event, PROTOCOL_VERSION, Request, Response};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender as Tx, channel};
use std::sync::{Arc, Mutex};

const MAX_LIMIT: u32 = 500;
const SINCE_LIMIT: u32 = 1000;

/// Connected clients, each fed through its own writer thread.
#[derive(Default)]
pub struct Hub {
    clients: Mutex<Vec<(u64, Tx<String>)>>,
    next_id: Mutex<u64>,
}

impl Hub {
    fn add(&self, tx: Tx<String>) -> u64 {
        let mut next = self.next_id.lock().unwrap();
        *next += 1;
        self.clients.lock().unwrap().push((*next, tx));
        *next
    }

    fn remove(&self, id: u64) {
        self.clients.lock().unwrap().retain(|(cid, _)| *cid != id);
    }

    pub fn broadcast(&self, line: &str) {
        self.clients
            .lock()
            .unwrap()
            .retain(|(_, tx)| tx.send(line.to_string()).is_ok());
    }
}

pub struct ServeOpts {
    pub db: PathBuf,
    pub socket: PathBuf,
    pub dry_run_send: bool,
    /// Contacts directory; `None` disables merging by person.
    pub contacts: Option<PathBuf>,
}

/// Per-process settings shared by client threads.
struct Shared {
    db: PathBuf,
    contacts: Option<PathBuf>,
    sender: Sender,
}

pub fn serve(opts: ServeOpts) -> Result<()> {
    // Fail fast (and loudly, for the launchd log) if the DB can't be read.
    Db::open(&opts.db).with_context(|| {
        format!(
            "cannot read {}; grant Full Disk Access to {}",
            opts.db.display(),
            std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "imsg-bridge".into())
        )
    })?;

    let listener = bind(&opts.socket)?;
    log!(
        "serving {} on {}{}",
        opts.db.display(),
        opts.socket.display(),
        if opts.dry_run_send {
            " (dry-run sends)"
        } else {
            ""
        }
    );

    let hub = Arc::new(Hub::default());
    {
        let (hub, db) = (hub.clone(), opts.db.clone());
        std::thread::spawn(move || {
            if let Err(e) = watch::run(&db, &hub) {
                log!("watcher stopped: {e:#}");
                std::process::exit(1);
            }
        });
    }

    let shared = Arc::new(Shared {
        db: opts.db.clone(),
        contacts: opts.contacts.clone(),
        sender: Sender::new(opts.dry_run_send),
    });
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                log!("accept failed: {e}");
                continue;
            }
        };
        let (hub, shared) = (hub.clone(), shared.clone());
        std::thread::spawn(move || {
            if let Err(e) = handle_client(stream, &shared, &hub) {
                log!("client error: {e:#}");
            }
        });
    }
    Ok(())
}

fn bind(socket: &Path) -> Result<UnixListener> {
    let dir = socket.parent().context("socket path has no parent")?;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    if socket.exists() {
        if UnixStream::connect(socket).is_ok() {
            bail!(
                "another imsg-bridge is already serving {}",
                socket.display()
            );
        }
        std::fs::remove_file(socket)?;
    }
    let listener =
        UnixListener::bind(socket).with_context(|| format!("binding {}", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

fn handle_client(stream: UnixStream, shared: &Shared, hub: &Hub) -> Result<()> {
    let (tx, rx) = channel::<String>();
    let mut writer = stream.try_clone()?;
    let writer_thread = std::thread::spawn(move || {
        for line in rx {
            if writer
                .write_all(line.as_bytes())
                .and_then(|_| writer.write_all(b"\n"))
                .is_err()
            {
                break;
            }
        }
        let _ = writer.shutdown(std::net::Shutdown::Both);
    });

    let hello = Event::Hello {
        version: PROTOCOL_VERSION,
        bridge: env!("CARGO_PKG_VERSION").into(),
    };
    tx.send(serde_json::to_string(&hello)?)?;

    let db = match Db::open(&shared.db) {
        Ok(db) => db,
        Err(e) => {
            tx.send(serde_json::to_string(&Event::Error {
                msg: format!("{e:#}"),
            })?)?;
            return Ok(());
        }
    };
    let client_id = hub.add(tx.clone());

    for line in BufReader::new(&stream).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => dispatch(&db, shared, req),
            Err(e) => Response::error(0, format!("bad request: {e}")),
        };
        if tx.send(serde_json::to_string(&resp)?).is_err() {
            break;
        }
    }

    // Let the writer drain queued responses before closing the socket.
    hub.remove(client_id);
    drop(tx);
    let _ = writer_thread.join();
    let _ = stream.shutdown(std::net::Shutdown::Both);
    Ok(())
}

fn dispatch(db: &Db, shared: &Shared, req: Request) -> Response {
    let id = req.id;
    let sender = &shared.sender;
    let result = match req.cmd {
        Command::People { names } => db.handle_ids().map(|handles| Response {
            id,
            people: Some(contacts::groups_for(
                shared.contacts.as_deref(),
                &handles,
                names,
            )),
            ..Default::default()
        }),
        Command::ListChats { limit } => db.list_chats(limit.min(MAX_LIMIT)).map(|chats| Response {
            id,
            chats: Some(chats),
            ..Default::default()
        }),
        Command::History {
            chat_id,
            chat_ids,
            before_rowid,
            limit,
        } => db
            .history(
                &std::iter::once(chat_id).chain(chat_ids).collect::<Vec<_>>(),
                before_rowid,
                limit.min(MAX_LIMIT),
            )
            .map(|messages| Response {
                id,
                messages: Some(messages),
                ..Default::default()
            }),
        Command::Since { rowid } => db.since(rowid, SINCE_LIMIT).map(|messages| Response {
            id,
            messages: Some(messages),
            ..Default::default()
        }),
        Command::Send {
            to,
            chat_guid,
            text,
        } => send(db, sender, to.as_deref(), chat_guid.as_deref(), &text)
            // Log the error only: never message text or recipients.
            .inspect_err(|e| log!("send failed: {e:#}"))
            .map(|service| Response {
                id,
                ok: Some(true),
                service: Some(service),
                ..Default::default()
            }),
    };
    result.unwrap_or_else(|e| Response::error(id, format!("{e:#}")))
}

fn send(
    db: &Db,
    sender: &Sender,
    to: Option<&str>,
    chat_guid: Option<&str>,
    text: &str,
) -> Result<String> {
    match (to, chat_guid) {
        (Some(to), None) => sender.send(Target::Handle(to), text),
        (None, Some(guid)) => match sender.send(Target::Chat(guid), text) {
            // Messages rejects some chat ids it lists in chat.db (notably email
            // chats) with -1728 before sending anything. One-on-one chats can go
            // by address instead. Other errors aren't retried, to avoid double sends.
            Err(e) if format!("{e:#}").contains("(-1728)") => match db.direct_handle(guid)? {
                Some(handle) => {
                    log!("chat id not accepted by Messages; sending by address");
                    sender.send(Target::Handle(&handle), text)
                }
                None => Err(e),
            },
            other => other,
        },
        _ => bail!("send needs exactly one of `to` or `chat_guid`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::fixture_db;
    use proto::{ServerMsg, parse_server_line};
    use std::io::BufRead;

    #[test]
    fn one_on_one_falls_back_to_address() {
        let db = Db::open(&fixture_db()).unwrap();
        let sender = Sender::failing_chat_ids();
        let ok = send(
            &db,
            &sender,
            None,
            Some("iMessage;-;friend@example.com"),
            "hi",
        );
        assert_eq!(ok.unwrap(), "dry-run");
        let group = send(
            &db,
            &sender,
            None,
            Some("iMessage;+;chat000000000000000001"),
            "hi",
        );
        assert!(
            group.unwrap_err().to_string().contains("-1728"),
            "groups have no address to fall back to"
        );
    }

    #[test]
    fn end_to_end_over_socket() {
        let db = fixture_db();
        let socket = std::env::temp_dir()
            .join(format!("imsg-test-{}", std::process::id()))
            .join("bridge.sock");
        let opts = ServeOpts {
            db,
            socket: socket.clone(),
            dry_run_send: true,
            contacts: Some(crate::contacts::tests::fixture_dir()),
        };
        std::thread::spawn(move || serve(opts).unwrap());
        let start = std::time::Instant::now();
        let stream = loop {
            if let Ok(s) = UnixStream::connect(&socket) {
                break s;
            }
            assert!(start.elapsed().as_secs() < 5, "server did not start");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let mut w = stream.try_clone().unwrap();
        let mut lines = BufReader::new(stream).lines();
        let mut next = || parse_server_line(&lines.next().unwrap().unwrap()).unwrap();

        assert!(matches!(
            next(),
            ServerMsg::Event(Event::Hello {
                version: PROTOCOL_VERSION,
                ..
            })
        ));

        writeln!(w, r#"{{"id":1,"cmd":"list_chats","limit":10}}"#).unwrap();
        let ServerMsg::Response(r) = next() else {
            panic!()
        };
        assert_eq!((r.id, r.chats.unwrap().len()), (1, 5));

        writeln!(
            w,
            r#"{{"id":2,"cmd":"send","chat_guid":"iMessage;-;+15550000001","text":"hi"}}"#
        )
        .unwrap();
        let ServerMsg::Response(r) = next() else {
            panic!()
        };
        assert_eq!(
            (r.id, r.ok, r.service.as_deref()),
            (2, Some(true), Some("dry-run"))
        );

        writeln!(w, r#"{{"id":3,"cmd":"send","text":"hi"}}"#).unwrap();
        let ServerMsg::Response(r) = next() else {
            panic!()
        };
        assert!(r.error.unwrap().contains("exactly one"));

        writeln!(w, r#"{{"id":4,"cmd":"people"}}"#).unwrap();
        let ServerMsg::Response(r) = next() else {
            panic!()
        };
        let people = r.people.unwrap();
        assert_eq!(people.len(), 2, "{people:?}");
        assert_eq!(
            people[0].handles,
            vec!["+15550000001", "friend@example.com"]
        );
        assert_eq!(people[1].handles, vec!["+15550000002", "+15550000004"]);
        assert!(people.iter().all(|p| p.name.is_none()));

        writeln!(w, "not json").unwrap();
        let ServerMsg::Response(r) = next() else {
            panic!()
        };
        assert!(r.error.unwrap().starts_with("bad request"));
    }
}
