//! `omabubble-bridge install` / `uninstall`: manage the LaunchAgent.
//!
//! The binary and the plist are root-owned (`/usr/local/bin`,
//! `/Library/LaunchAgents`) so that code running as the user can't replace
//! the program that holds Full Disk Access. The agent itself still runs in
//! the user's login session, which AppleScript needs.

use anyhow::{Context, Result, bail};
use std::ffi::CStr;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const LABEL: &str = "com.omabubble.bridge";
const SYSTEM_PLIST: &str = "/Library/LaunchAgents/com.omabubble.bridge.plist";

/// The user the agent runs for: the account behind `sudo`.
struct Target {
    uid: u32,
    home: PathBuf,
}

fn target_user() -> Result<Target> {
    // SAFETY: getuid has no preconditions and cannot fail.
    if unsafe { libc::getuid() } != 0 {
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "omabubble-bridge".into());
        bail!(
            "needs root so the binary and plist can't be replaced by user-level code: run `sudo {exe} install`"
        );
    }
    let uid: u32 = std::env::var("SUDO_UID")
        .ok()
        .and_then(|u| u.parse().ok())
        .filter(|u| *u != 0)
        .context("run through `sudo` from your own account, not as root directly")?;
    // SAFETY: getpwuid returns a pointer to static storage or null; it's read
    // immediately and not retained.
    let pw = unsafe { libc::getpwuid(uid) };
    if pw.is_null() {
        bail!("no account with uid {uid}");
    }
    let home = unsafe { CStr::from_ptr((*pw).pw_dir) }
        .to_string_lossy()
        .into_owned();
    Ok(Target {
        uid,
        home: PathBuf::from(home),
    })
}

fn domain(uid: u32) -> String {
    format!("gui/{uid}")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn plist(exe: &str, log: &str) -> String {
    let (exe, log) = (xml_escape(exe), xml_escape(log));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>serve</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#
    )
}

fn bootout(uid: u32, label: &str) {
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{}/{label}", domain(uid))])
        .stderr(std::process::Stdio::null())
        .status();
}

pub fn install() -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("install only works on macOS");
    }
    let t = target_user()?;
    // launchd does not expand `~`, so write absolute paths.
    let exe = std::env::current_exe()?.canonicalize()?;
    if std::fs::metadata(&exe)?.uid() != 0 {
        bail!(
            "{} is not owned by root; install it with `sudo install -o root -g wheel -m 0755 … /usr/local/bin/omabubble-bridge` first",
            exe.display()
        );
    }
    let log = t.home.join("Library/Logs/omabubble-bridge.log");

    let path = Path::new(SYSTEM_PLIST);
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, plist(&exe.to_string_lossy(), &log.to_string_lossy()))?;
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o644))?;

    // Reinstall cleanly if it was already loaded (fails harmlessly on first install).
    bootout(t.uid, LABEL);
    let status = Command::new("launchctl")
        .args(["bootstrap", &domain(t.uid)])
        .arg(path)
        .status()?;
    if !status.success() {
        bail!("launchctl bootstrap failed ({status})");
    }
    println!(
        "Installed {} → {} (runs as uid {})",
        path.display(),
        exe.display(),
        t.uid
    );
    println!("Logs: {}", log.display());
    println!(
        "If this is a new path, grant Full Disk Access to {} in System Settings → Privacy & Security.",
        exe.display()
    );
    Ok(())
}

pub fn uninstall() -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("uninstall only works on macOS");
    }
    let t = target_user()?;
    bootout(t.uid, LABEL);
    let path = Path::new(SYSTEM_PLIST);
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    println!("Removed {LABEL}");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn plist_has_absolute_args() {
        let p = super::plist(
            "/usr/local/bin/omabubble-bridge",
            "/Users/you/Library/Logs/omabubble-bridge.log",
        );
        assert!(p.contains(
            "<string>/usr/local/bin/omabubble-bridge</string>\n        <string>serve</string>"
        ));
        assert!(!p.contains('~'));
    }
}
