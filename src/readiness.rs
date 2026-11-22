// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

//! Readiness checks: a TCP port that accepts, an HTTP path that answers 2xx, or a command that exits 0.

use crate::manifest::Readiness;
use crate::reaper;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const ATTEMPT_LIMIT: Duration = Duration::from_secs(5);

/// Checks `r` every `interval_ms` until it passes (true), `timeout_seconds` pass (false) or `cancel` is set
/// (false).
pub fn wait_ready(r: &Readiness, cancel: &AtomicBool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(r.timeout_seconds);
    let interval = Duration::from_millis(r.interval_ms);
    loop {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if check(r, left.min(ATTEMPT_LIMIT)) {
            return !cancel.load(Ordering::SeqCst);
        }
        if Instant::now() + interval >= deadline {
            return false;
        }
        std::thread::sleep(interval);
    }
}

/// One attempt of `r`, given at most `limit`.
pub fn check(r: &Readiness, limit: Duration) -> bool {
    let limit = limit.max(Duration::from_millis(100));
    if let Some(t) = &r.tcp {
        return TcpStream::connect_timeout(&local(t.port), limit).is_ok();
    }
    if let Some(h) = &r.http {
        return http_ok(h.port, &h.path, limit);
    }
    if let Some(e) = &r.exec {
        return exec_ok(&e.command, limit);
    }
    true
}

fn local(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

fn http_ok(port: u16, path: &str, limit: Duration) -> bool {
    let Ok(mut s) = TcpStream::connect_timeout(&local(port), limit) else { return false };
    let _ = s.set_read_timeout(Some(limit));
    let _ = s.set_write_timeout(Some(limit));
    let req = format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut head = [0u8; 12];
    if s.read_exact(&mut head).is_err() {
        return false;
    }
    // "HTTP/1.x 2xx"
    head.starts_with(b"HTTP/1.") && head[8] == b' ' && head[9] == b'2'
}

fn exec_ok(command: &[String], limit: Duration) -> bool {
    let (tx, rx) = mpsc::channel();
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
    let child = match reaper::spawn(&mut cmd, move |_, status| {
        let _ = tx.send(status);
    }) {
        Ok(c) => c,
        Err(_) => return false,
    };
    match rx.recv_timeout(limit) {
        Ok(status) => status.code() == Some(0),
        Err(_) => {
            reaper::kill_group(child.id() as i32, libc::SIGKILL);
            false
        }
    }
}

/// A probe running for one start of a service; setting its flag stops it.
pub struct Probe(pub Arc<AtomicBool>);

impl Probe {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
#[path = "readiness_tests.rs"]
mod tests;
