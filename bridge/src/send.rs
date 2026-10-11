//! Sending through Messages.app with osascript.
//!
//! Values travel as argv to `on run argv`, so message text never needs escaping.

use crate::log;
use crate::redact::redact;
use anyhow::{Result, bail};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TO_HANDLE: &str = r#"on run argv
set targetHandle to item 1 of argv
set messageText to item 2 of argv
set serviceName to item 3 of argv
tell application "Messages"
if serviceName is "SMS" then
set targetAccount to 1st account whose service type = SMS
else
set targetAccount to 1st account whose service type = iMessage
end if
send messageText to participant targetHandle of targetAccount
end tell
end run"#;

const TO_CHAT: &str = r#"on run argv
tell application "Messages" to send (item 2 of argv) to chat id (item 1 of argv)
end run"#;

/// Long enough to walk to the Mac and answer an Automation permission
/// dialog: killing osascript dismisses the dialog.
const TIMEOUT: Duration = Duration::from_secs(120);

pub enum Target<'a> {
    /// A phone number or email; tries iMessage, then SMS.
    Handle(&'a str),
    /// An existing chat's `chat.guid`; uses that chat's own service.
    Chat(&'a str),
}

pub struct Sender {
    dry_run: bool,
    /// Tests: make dry-run chat-id sends fail like Messages does for email chats.
    fail_chat_ids: bool,
    /// One osascript at a time.
    lock: Mutex<()>,
}

impl Sender {
    pub fn new(dry_run: bool) -> Sender {
        Sender {
            dry_run,
            fail_chat_ids: false,
            lock: Mutex::new(()),
        }
    }

    #[cfg(test)]
    pub fn failing_chat_ids() -> Sender {
        Sender {
            fail_chat_ids: true,
            ..Sender::new(true)
        }
    }

    /// Returns the service the message went out on.
    pub fn send(&self, target: Target, text: &str) -> Result<String> {
        if text.trim().is_empty() {
            bail!("empty message");
        }
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        match target {
            Target::Handle(handle) => {
                match self.osascript(TO_HANDLE, &[handle, text, "iMessage"]) {
                    Ok(svc) => Ok(svc.unwrap_or_else(|| "iMessage".into())),
                    Err(e) => {
                        log!(
                            "iMessage send failed ({}); retrying as SMS",
                            redact(&e.to_string())
                        );
                        let svc = self.osascript(TO_HANDLE, &[handle, text, "SMS"])?;
                        Ok(svc.unwrap_or_else(|| "SMS".into()))
                    }
                }
            }
            Target::Chat(guid) => {
                let svc = self.osascript(TO_CHAT, &[guid, text])?;
                Ok(svc.unwrap_or_else(|| guid.split(';').next().unwrap_or("iMessage").to_string()))
            }
        }
    }

    /// Runs the script; `Ok(Some("dry-run"))` in dry-run mode, `Ok(None)` after a real send.
    fn osascript(&self, script: &str, args: &[&str]) -> Result<Option<String>> {
        let mut cmd = Command::new("osascript");
        for line in script.lines() {
            cmd.arg("-e").arg(line);
        }
        cmd.args(args);

        if self.dry_run && self.fail_chat_ids && script == TO_CHAT {
            bail!(
                "osascript failed: execution error: Messages got an error: Can’t get chat id. (-1728)"
            );
        }
        if self.dry_run {
            log!(
                "dry-run send: osascript <{} lines> argv={:?}",
                script.lines().count(),
                args
            );
            return Ok(Some("dry-run".into()));
        }

        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if start.elapsed() > TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                bail!(
                    "osascript timed out after {}s (Automation permission prompt pending?)",
                    TIMEOUT.as_secs()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        if status.success() {
            return Ok(None);
        }
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            use std::io::Read;
            let _ = stderr.read_to_string(&mut err);
        }
        let err = err.trim();
        if err.contains("-1743") {
            bail!(
                "not allowed to control Messages (-1743): allow omabubble-bridge under Privacy & Security → Automation"
            );
        }
        bail!(
            "osascript failed: {}",
            if err.is_empty() { "unknown error" } else { err }
        );
    }
}
