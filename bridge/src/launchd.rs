//! `imsg-bridge install` / `uninstall`: manage the per-user LaunchAgent.

use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::process::Command;

const LABEL: &str = "com.imsg.bridge";

fn plist_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("no home directory")?
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

fn domain() -> String {
    // SAFETY: getuid has no preconditions and cannot fail.
    format!("gui/{}", unsafe { libc::getuid() })
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

pub fn install() -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("install only works on macOS");
    }
    // launchd does not expand `~`, so write absolute paths.
    let exe = std::env::current_exe()?.canonicalize()?;
    let home = dirs::home_dir().context("no home directory")?;
    let log = home.join("Library/Logs/imsg-bridge.log");
    let path = plist_path()?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, plist(&exe.to_string_lossy(), &log.to_string_lossy()))?;

    // Reinstall cleanly if it was already loaded (fails harmlessly on first install).
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{}/{LABEL}", domain())])
        .stderr(std::process::Stdio::null())
        .status();
    let status = Command::new("launchctl")
        .args(["bootstrap", &domain()])
        .arg(&path)
        .status()?;
    if !status.success() {
        bail!("launchctl bootstrap failed ({status})");
    }
    println!("Installed {} → {}", path.display(), exe.display());
    println!("Logs: {}", log.display());
    println!(
        "Next: grant Full Disk Access to {} in System Settings → Privacy & Security.",
        exe.display()
    );
    Ok(())
}

pub fn uninstall() -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("uninstall only works on macOS");
    }
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{}/{LABEL}", domain())])
        .status();
    let path = plist_path()?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    println!("Removed {LABEL}");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn plist_has_absolute_args() {
        let p = super::plist(
            "/Users/you/.local/bin/imsg-bridge",
            "/Users/you/Library/Logs/imsg-bridge.log",
        );
        assert!(p.contains(
            "<string>/Users/you/.local/bin/imsg-bridge</string>\n        <string>serve</string>"
        ));
        assert!(!p.contains('~'));
    }
}
