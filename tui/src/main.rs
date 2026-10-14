mod app;
mod attach;
mod clipboard;
mod config;
mod conn;
mod emoji;
mod links;
mod notify;
mod phone;
mod pics;
mod ui;

use anyhow::{Result, bail};
use app::{App, Effect};
use clap::Parser;
use conn::{Conn, ConnEvent};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(
    name = "omabubble",
    version = env!("OMABUBBLE_VERSION"),
    about = "iMessage in the terminal, via omabubble-bridge on a Mac"
)]
struct Cli {
    /// SSH host to connect to (overrides `host` in the config).
    host: Option<String>,
    /// Config file [default: ~/.config/omabubble/config.toml]
    #[arg(long)]
    config: Option<PathBuf>,
}

enum AppEvent {
    Term(Event),
    Conn(conn::Tagged),
    /// A desktop notification for this chat was clicked.
    NotifyClicked(i64),
    /// A file for the attach prompt's preview, decoded.
    Preview(PathBuf, pics::Decoded),
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
    // Focus reports tell a chat open on another workspace from one being read.
    execute!(std::io::stdout(), EnableBracketedPaste, EnableFocusChange)?;
    if cfg.mouse {
        execute!(std::io::stdout(), EnableMouseCapture)?;
    }
    let result = run(&mut terminal, &cfg);
    if cfg.mouse {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    let _ = execute!(std::io::stdout(), DisableFocusChange, DisableBracketedPaste);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, cfg: &config::Config) -> Result<()> {
    // Ask the terminal which graphics it speaks. This reads its reply from
    // stdin, so it must happen before the key reader thread starts.
    let picker = cfg.images.then(detect_graphics);
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
    app.pics.picker = picker;
    app.copy_links_too = !has_display();
    app.recents = load_recents();
    app.hidden = load_hidden();
    app.drafts = load_drafts();
    let mut conn = Conn::default();
    let start = |conn: &mut Conn, app: &mut App| {
        app.on_connecting();
        if let Err(e) = conn.start(cfg, conn_tx.clone()) {
            app.on_closed(format!("{e:#}"), Instant::now());
        }
    };
    start(&mut conn, &mut app);
    // Our terminal window, so a clicked notification can bring it forward.
    // Just launched, it is almost certainly the focused one; focus reports refresh it.
    let mut window = notify::active_window();

    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;
        if std::mem::take(&mut app.pics.needs_clear) {
            // Pictures moved: wipe the screen and repaint everything so none
            // leave pixels behind. Not `terminal.clear()`: that asks the
            // terminal for the cursor position, and the key reader thread
            // takes the reply, so the request times out and the app exits.
            use ratatui::backend::{Backend, ClearType};
            terminal.backend_mut().clear_region(ClearType::All)?;
            // Both buffers blank now, matching the screen: the next draw
            // writes every cell.
            terminal.swap_buffers();
            terminal.draw(|f| ui::draw(f, &mut app))?;
            app.pics.needs_clear = false;
        }

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
                AppEvent::Term(Event::FocusGained) => {
                    window = notify::active_window().or(window.take());
                    app.on_focus(true)
                }
                AppEvent::Term(Event::FocusLost) => app.on_focus(false),
                AppEvent::Term(_) => {}
                AppEvent::Conn((generation, _)) if generation != conn.generation => {}
                AppEvent::Conn((_, ConnEvent::Line(line))) => app.on_line(&line, now),
                AppEvent::Conn((_, ConnEvent::Closed(reason))) => app.on_closed(reason, now),
                AppEvent::Preview(path, decoded) => app.pics.local_decoded(path, decoded),
                AppEvent::NotifyClicked(chat_id) => {
                    if let Some(w) = &window {
                        notify::focus_window(w);
                    }
                    app.on_notify_click(chat_id, now);
                }
            }
        }
        app.tick(Instant::now());

        for effect in std::mem::take(&mut app.effects) {
            match effect {
                Effect::Request(req) => {
                    // A failed write shows up as a Closed event from the reader.
                    conn.send(&serde_json::to_string(&req)?);
                }
                Effect::Notify {
                    title,
                    body,
                    chat_id,
                } => {
                    let tx = tx.clone();
                    notify::desktop(&title, &body, cfg.notification_seconds, move || {
                        let _ = tx.send(AppEvent::NotifyClicked(chat_id));
                    });
                }
                Effect::Bell => notify::bell(),
                Effect::Copy(text) => clipboard::copy(&text),
                Effect::Reconnect => start(&mut conn, &mut app),
                Effect::Disconnect => conn.stop(),
                Effect::SaveRecents(recents) => save_recents(&recents),
                Effect::SaveDrafts(drafts) => save_drafts(&drafts),
                Effect::SavePicture { name, jpeg } => {
                    let msg = match save_picture(&name, &jpeg) {
                        Ok(path) => {
                            open(path.as_os_str());
                            format!("saved {}", path.display())
                        }
                        Err(e) => format!("couldn't save: {e}"),
                    };
                    app.notice(msg, Instant::now());
                }
                Effect::SaveHidden(hidden) => save_hidden(&hidden),
                Effect::LoadPreview(path) => {
                    let tx = tx.clone();
                    std::thread::spawn(move || {
                        let decoded = pics::decode_local(&path);
                        let _ = tx.send(AppEvent::Preview(path, decoded));
                    });
                }
                Effect::OpenUrl(url) => {
                    if !open(url.as_ref()) {
                        app.notice(
                            "couldn't open the link: is xdg-open installed?",
                            Instant::now(),
                        );
                    }
                }
            }
        }
        if app.quit {
            break;
        }
    }
    // Whatever is in the input becomes the open chat's draft.
    app.flush_draft();
    save_drafts(&app.drafts);
    conn.stop();
    Ok(())
}

fn drafts_path() -> Option<PathBuf> {
    dirs::state_dir().map(|d| d.join("omabubble/drafts.json"))
}

fn load_drafts() -> HashMap<String, String> {
    drafts_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Unsent message text: readable only by you, written whole via a temp file.
fn save_drafts(drafts: &HashMap<String, String>) {
    use std::os::unix::fs::OpenOptionsExt;
    let Some(path) = drafts_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(json) = serde_json::to_vec(drafts) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| std::io::Write::write_all(&mut f, &json));
    if written.is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn recents_path() -> Option<PathBuf> {
    dirs::state_dir().map(|d| d.join("omabubble/recent_emoji"))
}

/// One `chat_id rowid` pair per line.
fn hidden_path() -> Option<PathBuf> {
    dirs::state_dir().map(|d| d.join("omabubble/hidden_chats"))
}

fn load_hidden() -> HashMap<i64, i64> {
    hidden_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| {
            s.lines()
                .filter_map(|l| {
                    let (id, rowid) = l.split_once(' ')?;
                    Some((id.parse().ok()?, rowid.parse().ok()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn save_hidden(hidden: &HashMap<i64, i64>) {
    if let Some(path) = hidden_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut lines: Vec<String> = hidden.iter().map(|(id, r)| format!("{id} {r}")).collect();
        lines.sort();
        let _ = std::fs::write(path, lines.join("\n"));
    }
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

/// How this terminal can draw pictures.
///
/// Asking the terminal is the only reliable way, but a terminal that never
/// answers leaves the query's reader blocked on stdin after it gives up, and
/// it then swallows the first key press. Multiplexers are the usual
/// non-answerers (and pass graphics through only when configured to), so
/// inside one use half-blocks without asking.
fn detect_graphics() -> ratatui_image::picker::Picker {
    use ratatui_image::picker::{Picker, cap_parser::QueryStdioOptions};
    if ["TMUX", "STY", "ZELLIJ"]
        .iter()
        .any(|v| std::env::var_os(v).is_some())
    {
        return Picker::halfblocks();
    }
    // Terminals that answer do so within milliseconds; the library waits 2s.
    Picker::from_query_stdio_with_options(QueryStdioOptions {
        timeout: Duration::from_millis(500),
        ..Default::default()
    })
    .unwrap_or_else(|_| Picker::halfblocks())
}

/// Writes a picture to the downloads folder as `<name>.jpg`, never
/// overwriting: `<name>-1.jpg`, `<name>-2.jpg`, … when taken.
fn save_picture(name: &str, jpeg: &[u8]) -> std::io::Result<PathBuf> {
    let dir = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .ok_or_else(|| std::io::Error::other("no downloads folder"))?;
    std::fs::create_dir_all(&dir)?;
    let stem = picture_stem(name);
    for n in 0.. {
        let file = if n == 0 {
            format!("{stem}.jpg")
        } else {
            format!("{stem}-{n}.jpg")
        };
        let path = dir.join(file);
        // create_new: fails rather than replacing a file that appeared meanwhile.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                std::io::Write::write_all(&mut f, jpeg)?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// A safe file stem from an attachment name: no extension, no path parts.
fn picture_stem(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let stem = base.rsplit_once('.').map_or(base, |(s, _)| s);
    let clean: String = stem
        .chars()
        .map(|c| if c.is_control() { '_' } else { c })
        .collect::<String>()
        .trim_start_matches('.')
        .to_string();
    if clean.is_empty() {
        "picture".into()
    } else {
        clean
    }
}

/// Whether this session has a display. Over SSH it usually doesn't; links
/// may still open (through the desktop's launcher), but can't be relied on.
fn has_display() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}

/// Opens a file or web link with the desktop's default app; false if
/// xdg-open couldn't be started.
fn open(target: &std::ffi::OsStr) -> bool {
    match std::process::Command::new("xdg-open")
        .arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn picture_stems() {
        use super::picture_stem;
        assert_eq!(picture_stem("IMG_2041.heic"), "IMG_2041");
        assert_eq!(
            picture_stem("../../.bashrc"),
            "picture",
            "hidden files and paths are dropped"
        );
        assert_eq!(picture_stem("a/b/c.jpg"), "c");
        assert_eq!(picture_stem(""), "picture");
        assert_eq!(picture_stem("..."), "picture");
        assert_eq!(picture_stem("no ext"), "no ext");
    }
}
