mod db;
mod launchd;
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
        } => server::serve(server::ServeOpts {
            db: db.map_or_else(default_db, Ok)?,
            socket: socket.map_or_else(default_socket, Ok)?,
            dry_run_send,
        }),
        Cmd::Connect { socket } => relay::connect(&socket.map_or_else(default_socket, Ok)?),
        Cmd::Install => launchd::install(),
        Cmd::Uninstall => launchd::uninstall(),
        Cmd::Query { db, what } => {
            let db = db::Db::open(&db.map_or_else(default_db, Ok)?)?;
            let out = match what {
                Query::Chats { limit } => serde_json::to_string_pretty(&db.list_chats(limit)?)?,
                Query::History { chat_id, limit } => {
                    serde_json::to_string_pretty(&db.history(chat_id, None, limit)?)?
                }
                Query::Since { rowid } => serde_json::to_string_pretty(&db.since(rowid, 1000)?)?,
            };
            println!("{out}");
            Ok(())
        }
    }
}
