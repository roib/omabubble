mod app;
mod clipboard;
mod config;
mod conn;
mod emoji;
mod notify;
mod phone;
mod ui;

use anyhow::{Result, bail};
use app::{App, Effect};
use clap::Parser;
use conn::{Conn, ConnEvent};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(
    name = "imsg",
    version,
    about = "iMessage in the terminal, via imsg-bridge on a Mac"
)]
struct Cli {
    /// SSH host to connect to (overrides `host` in the config).
    host: Option<String>,
    /// Config file [default: ~/.config/imsg/config.toml]
    #[arg(long)]
    config: Option<PathBuf>,
}

enum AppEvent {
    Term(Event),
    Conn(conn::Tagged),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let path = match cli.config {
        Some(p) => p,
        None => config::default_path()?,
    };
    let Some(mut cfg) = config::load(&path)? else {
        eprintln!(
            "No config at {}. Create it, for example:\n\n{}",
            path.display(),
            config::SAMPLE
        );
        std::process::exit(1);
    };
    if let Some(host) = cli.host {
        cfg.host = host;
    }
    if cfg.host.is_empty() && cfg.command.is_none() {
        bail!(
            "no host: set `host` in {} or pass one on the command line",
            path.display()
        );
    }

    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableBracketedPaste)?;
    if cfg.mouse {
        execute!(std::io::stdout(), EnableMouseCapture)?;
    }
    let result = run(&mut terminal, &cfg);
    if cfg.mouse {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, cfg: &config::Config) -> Result<()> {
    let (tx, rx) = mpsc::channel::<AppEvent>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = event::read() {
                if tx.send(AppEvent::Term(ev)).is_err() {
                    break;
                }
            }
        });
    }
    // Connection events arrive on their own channel and are forwarded.
    let (conn_tx, conn_rx) = mpsc::channel::<conn::Tagged>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for ev in conn_rx {
                if tx.send(AppEvent::Conn(ev)).is_err() {
                    break;
                }
            }
        });
    }

    let mut app = App::new(cfg);
    app.recents = load_recents();
    let mut conn = Conn::default();
    let start = |conn: &mut Conn, app: &mut App| {
        app.on_connecting();
        if let Err(e) = conn.start(cfg, conn_tx.clone()) {
            app.on_closed(format!("{e:#}"), Instant::now());
        }
    };
    start(&mut conn, &mut app);

    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;

        let first = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(ev) => Some(ev),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        // Handle everything queued before redrawing.
        for ev in first
            .into_iter()
            .chain(std::iter::from_fn(|| rx.try_recv().ok()))
        {
            let now = Instant::now();
            match ev {
                AppEvent::Term(Event::Key(k)) if k.kind != KeyEventKind::Release => {
                    app.on_key(k, now)
                }
                AppEvent::Term(Event::Paste(s)) => app.on_paste(&s),
                AppEvent::Term(Event::Mouse(m)) => app.on_mouse(m, now),
                AppEvent::Term(_) => {}
                AppEvent::Conn((generation, _)) if generation != conn.generation => {}
                AppEvent::Conn((_, ConnEvent::Line(line))) => app.on_line(&line, now),
                AppEvent::Conn((_, ConnEvent::Closed(reason))) => app.on_closed(reason, now),
            }
        }
        app.tick(Instant::now());

        for effect in std::mem::take(&mut app.effects) {
            match effect {
                Effect::Request(req) => {
                    // A failed write shows up as a Closed event from the reader.
                    conn.send(&serde_json::to_string(&req)?);
                }
                Effect::Notify { title, body } => notify::desktop(&title, &body),
                Effect::Bell => notify::bell(),
                Effect::Copy(text) => clipboard::copy(&text),
                Effect::Reconnect => start(&mut conn, &mut app),
                Effect::Disconnect => conn.stop(),
                Effect::SaveRecents(recents) => save_recents(&recents),
            }
        }
        if app.quit {
            break;
        }
    }
    conn.stop();
    Ok(())
}

fn recents_path() -> Option<PathBuf> {
    dirs::state_dir().map(|d| d.join("imsg/recent_emoji"))
}

fn load_recents() -> Vec<String> {
    recents_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| {
            s.lines()
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn save_recents(recents: &[String]) {
    if let Some(path) = recents_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, recents.join("\n"));
    }
}
