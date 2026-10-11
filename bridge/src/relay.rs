//! `omabubble-bridge connect`: relays stdio to the running service's socket.
//! This is what the restricted SSH key runs; it never touches chat.db.

use proto::Event;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

pub fn connect(socket: &Path) -> ! {
    let stream = match UnixStream::connect(socket) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("omabubble-bridge connect: {}: {e}", socket.display());
            let msg = "bridge service not running".to_string();
            let ev = serde_json::to_string(&Event::Error { msg }).unwrap_or_default();
            let _ = writeln!(io::stdout(), "{ev}");
            std::process::exit(1);
        }
    };

    let mut to_server = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("omabubble-bridge connect: {e}");
            std::process::exit(1);
        }
    };
    std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut to_server);
        let _ = to_server.shutdown(std::net::Shutdown::Write);
    });

    // Flush every chunk: `io::copy` into stdout can sit on buffered data,
    // and the TUI needs each line as soon as it arrives.
    let mut from_server = stream;
    let mut out = io::stdout().lock();
    let mut buf = [0u8; 16 * 1024];
    loop {
        match from_server.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if out.write_all(&buf[..n]).and_then(|_| out.flush()).is_err() {
                    break;
                }
            }
        }
    }
    std::process::exit(0);
}
