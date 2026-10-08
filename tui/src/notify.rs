use std::io::Write;
use std::process::{Command, Stdio};

pub fn desktop(title: &str, body: &str) {
    if let Ok(mut child) = Command::new("notify-send")
        .args(["-a", "imsg", title, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || child.wait());
    }
}

pub fn bell() {
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}
