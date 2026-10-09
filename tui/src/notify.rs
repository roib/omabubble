use std::io::{Read, Write};
use std::process::{Command, Stdio};

/// Shows a desktop notification for `seconds` (0: until dismissed);
/// `on_click` runs if the user clicks it.
///
/// notify-send waits until the notification closes and prints the action
/// that closed it; clicking the banner invokes the `default` action.
pub fn desktop(title: &str, body: &str, seconds: u32, on_click: impl FnOnce() + Send + 'static) {
    let mut cmd = Command::new("notify-send");
    cmd.args(["-a", "imsg", "-A", "default=Open"]);
    if seconds == 0 {
        // Only critical notifications stay up indefinitely on most desktops.
        cmd.args(["-u", "critical"]);
    } else {
        cmd.args(["-t", &seconds.saturating_mul(1000).to_string()]);
    }
    if let Ok(mut child) = cmd
        // `--`: a message starting with "-" is text, not an option.
        .args(["--", title, body])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || {
            let mut out = String::new();
            if let Some(mut stdout) = child.stdout.take() {
                let _ = stdout.read_to_string(&mut out);
            }
            let _ = child.wait();
            if out.trim() == "default" {
                on_click();
            }
        });
    }
}

pub fn bell() {
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}

/// Hyprland's address for the focused window, e.g. `0x55d1c2a0`; `None`
/// outside Hyprland.
pub fn active_window() -> Option<String> {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let out = Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let addr = v.get("address")?.as_str()?;
    addr.starts_with("0x").then(|| addr.to_string())
}

/// Focuses a window by address; Hyprland switches to its workspace.
pub fn focus_window(address: &str) {
    // Hyprland 0.56+ takes Lua dispatchers; older ones the classic form.
    let lua = format!("hl.dsp.focus({{ window = \"address:{address}\" }})");
    if !dispatch(&[&lua]) {
        dispatch(&["focuswindow", &format!("address:{address}")]);
    }
}

/// hyprctl exits 0 even when it rejects a dispatch; success prints "ok".
fn dispatch(args: &[&str]) -> bool {
    Command::new("hyprctl")
        .arg("dispatch")
        .args(args)
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "ok")
}
