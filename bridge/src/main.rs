mod contacts;
mod db;
mod images;
mod jpeg;
mod launchd;
mod marks;
mod redact;
mod relay;
mod send;
mod server;
mod watch;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Timestamped line on stderr (the launchd log for `serve`).
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        eprintln!("{} {}", $crate::db::now_unix(), format_args!($($arg)*))
    };
}

#[derive(Parser)]
#[command(
    name = "imsg-bridge",
    version,
    about = "iMessage bridge for the imsg TUI"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the bridge service (normally started by launchd).
    Serve {
        /// Messages database [default: ~/Library/Messages/chat.db]
        #[arg(long)]
        db: Option<PathBuf>,
        /// Unix socket path [default: ~/Library/Application Support/imsg/bridge.sock]
        #[arg(long)]
        socket: Option<PathBuf>,
        /// Log sends instead of running osascript.
        #[arg(long)]
        dry_run_send: bool,
        /// Contacts directory [default: ~/Library/Application Support/AddressBook]
        #[arg(long)]
        contacts: Option<PathBuf>,
        /// Don't read Contacts; chats of one person stay separate.
        #[arg(long)]
        no_contacts: bool,
        /// Serve picture files as-is instead of converting with sips (demo data).
        #[arg(long, hide = true)]
        images_passthrough: bool,
    },
    /// Relay stdin/stdout to the running service (the SSH forced command).
    Connect {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Install and load the LaunchAgent.
    Install,
    /// Unload and remove the LaunchAgent.
    Uninstall,
    /// Debug: print query results as JSON.
    Query {
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        contacts: Option<PathBuf>,
        #[command(subcommand)]
        what: Query,
    },
}

#[derive(Subcommand)]
enum Query {
    Chats {
        #[arg(default_value_t = 20)]
        limit: u32,
    },
    History {
        chat_id: i64,
        #[arg(default_value_t = 20)]
        limit: u32,
    },
    Since {
        rowid: i64,
    },
    /// Counts only: how many people group several handles. Never prints handles.
    People,
}

fn default_db() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("no home directory")?
        .join("Library/Messages/chat.db"))
}

fn default_socket() -> Result<PathBuf> {
    Ok(dirs::data_dir()
        .context("no data directory")?
        .join("imsg/bridge.sock"))
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Serve {
            db,
            socket,
            dry_run_send,
            contacts,
            no_contacts,
            images_passthrough,
        } => {
            let db = db.map_or_else(default_db, Ok)?;
            let home = dirs::home_dir().context("no home directory")?;
            server::serve(server::ServeOpts {
                // chat.db sits in ~/Library/Messages, beside Attachments.
                attachments: db.with_file_name("Attachments"),
                home,
                convert: if cfg!(target_os = "macos") && !images_passthrough {
                    images::Convert::Sips
                } else {
                    images::Convert::Passthrough
                },
                db,
                socket: socket.map_or_else(default_socket, Ok)?,
                dry_run_send,
                contacts: if no_contacts {
                    None
                } else {
                    contacts.or_else(contacts::default_dir)
                },
            })
        }
        Cmd::Connect { socket } => relay::connect(&socket.map_or_else(default_socket, Ok)?),
        Cmd::Install => launchd::install(),
        Cmd::Uninstall => launchd::uninstall(),
        Cmd::Query { db, contacts, what } => {
            let db = db::Db::open(&db.map_or_else(default_db, Ok)?)?;
            let out = match what {
                Query::People => {
                    let dir = contacts.or_else(contacts::default_dir);
                    let groups = contacts::groups_for(dir.as_deref(), &db.handle_ids()?, false);
                    let handles: usize = groups.iter().map(|g| g.handles.len()).sum();
                    format!(
                        "{{\"people_with_several_handles\": {}, \"handles_covered\": {handles}}}",
                        groups.len()
                    )
                }
                Query::Chats { limit } => {
                    serde_json::to_string_pretty(&db.list_chats(limit, &Default::default())?)?
                }
                Query::History { chat_id, limit } => {
                    serde_json::to_string_pretty(&db.history(&[chat_id], None, limit)?)?
                }
                Query::Since { rowid } => serde_json::to_string_pretty(&db.since(rowid, 1000)?)?,
            };
            println!("{out}");
            Ok(())
        }
    }
}
