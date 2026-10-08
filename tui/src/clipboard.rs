//! Copy text to the system clipboard: OSC 52 (works over SSH and in tmux with
//! `set-clipboard on`), plus wl-copy / xclip when a local display is available.

use base64::Engine;
use std::io::Write;
use std::process::{Command, Stdio};

pub fn copy(text: &str) {
    let b64 = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{b64}\x07");
    let _ = out.flush();

    let tool: Option<(&str, &[&str])> = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        Some(("wl-copy", &[]))
    } else if std::env::var_os("DISPLAY").is_some() {
        Some(("xclip", &["-selection", "clipboard"]))
    } else {
        None
    };
    if let Some((cmd, args)) = tool
        && let Ok(mut child) = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        // wl-copy forks to serve the selection; reap it off the UI thread.
        std::thread::spawn(move || child.wait());
    }
}
