// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

//! Each line a service writes, prefixed with its name: its stdout on the box's stdout, its stderr on its stderr.

use std::io::{BufRead, BufReader, Read, Write};
use std::thread::JoinHandle;

/// The box's own lines carry this name.
pub const SELF: &str = "fleetbox";

/// Copies `from` to `to` a line at a time, each line as `<name> | <line>`. A line is written in one call on the
/// locked stream, so lines of different services never interleave. A last line with no newline gets one.
pub fn prefix_lines(name: &str, from: impl Read, mut to: impl Write) {
    let mut r = BufReader::new(from);
    let mut line = Vec::new();
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if line.last() != Some(&b'\n') {
            line.push(b'\n');
        }
        let mut out = Vec::with_capacity(name.len() + 3 + line.len());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(b" | ");
        out.extend_from_slice(&line);
        if to.write_all(&out).and_then(|_| to.flush()).is_err() {
            return;
        }
    }
}

/// Forwards a child's stream on a thread of its own, which ends at the stream's end: join it before the process
/// exits, or the child's last lines may be lost.
pub fn forward(name: String, from: impl Read + Send + 'static, stderr: bool) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(format!("log-{name}"))
        .spawn(move || {
            if stderr {
                prefix_lines(&name, from, LockedStderr);
            } else {
                prefix_lines(&name, from, LockedStdout);
            }
        })
        .ok()
}

struct LockedStdout;
struct LockedStderr;

impl Write for LockedStdout {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        std::io::stdout().lock().write_all(buf).map(|_| buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stdout().lock().flush()
    }
}

impl Write for LockedStderr {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        std::io::stderr().lock().write_all(buf).map(|_| buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stderr().lock().flush()
    }
}

/// The supervisor's own line, on stderr.
pub fn say(msg: impl AsRef<str>) {
    let line = format!("{SELF} | {}\n", msg.as_ref());
    let _ = std::io::stderr().lock().write_all(line.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_each_line_with_the_service_name() {
        let mut out = Vec::new();
        prefix_lines("oauth", &b"one\ntwo\n\nlast without newline"[..], &mut out);
        assert_eq!(String::from_utf8(out).unwrap(), "oauth | one\noauth | two\noauth | \noauth | last without newline\n");
    }

    #[test]
    fn keeps_bytes_that_are_not_utf8() {
        let mut out = Vec::new();
        prefix_lines("k", &b"\xff\xfe\n"[..], &mut out);
        assert_eq!(out, b"k | \xff\xfe\n");
    }
}
