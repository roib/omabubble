//! The ssh child process that carries the JSON-lines protocol.

use crate::config::Config;
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

pub enum ConnEvent {
    Line(String),
    Closed(String),
}

/// Events from a connection carry its generation so stale ones can be ignored after a reconnect.
pub type Tagged = (u64, ConnEvent);

#[derive(Default)]
pub struct Conn {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    pub generation: u64,
}

pub fn command(cfg: &Config) -> Command {
    if let Some(argv) = cfg.command.as_ref().filter(|a| !a.is_empty()) {
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        return cmd;
    }
    let mut cmd = Command::new("ssh");
    // BatchMode: never prompt for a password on top of the TUI.
    cmd.args([
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=2",
    ]);
    cmd.args(&cfg.ssh_args);
    cmd.arg(&cfg.host);
    if let Some(remote) = cfg.remote_command.as_ref().filter(|r| !r.is_empty()) {
        cmd.arg(remote);
    }
    cmd
}

impl Conn {
    pub fn start(&mut self, cfg: &Config, tx: Sender<Tagged>) -> Result<()> {
        self.stop();
        self.generation += 1;
        let generation = self.generation;

        let mut child = command(cfg)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("starting ssh")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let mut stderr = child.stderr.take().context("no stderr")?;
        self.stdin = child.stdin.take();
        self.child = Some(child);

        let last_err = Arc::new(Mutex::new(String::new()));
        let stderr_thread = {
            let last_err = last_err.clone();
            std::thread::spawn(move || {
                let mut buf = String::new();
                let _ = stderr.read_to_string(&mut buf);
                if let Some(line) = buf.lines().rev().find(|l| !l.trim().is_empty()) {
                    *last_err.lock().unwrap() = line.trim().to_string();
                }
            })
        };
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send((generation, ConnEvent::Line(line))).is_err() {
                    return;
                }
            }
            let _ = stderr_thread.join();
            let reason = last_err.lock().unwrap().clone();
            let reason = if reason.is_empty() {
                "connection closed".to_string()
            } else {
                reason
            };
            let _ = tx.send((generation, ConnEvent::Closed(reason)));
        });
        Ok(())
    }

    pub fn send(&mut self, line: &str) -> bool {
        match self.stdin.as_mut() {
            Some(w) => w
                .write_all(line.as_bytes())
                .and_then(|_| w.write_all(b"\n"))
                .and_then(|_| w.flush())
                .is_ok(),
            None => false,
        }
    }

    pub fn stop(&mut self) {
        self.stdin = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.stop();
    }
}
