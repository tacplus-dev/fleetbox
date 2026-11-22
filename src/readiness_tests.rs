// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::manifest::{ExecCheck, HttpCheck, TcpCheck};
use std::net::TcpListener;

fn readiness() -> Readiness {
    Readiness { tcp: None, http: None, exec: None, timeout_seconds: 2, interval_ms: 20 }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[test]
fn tcp_is_ready_once_the_port_accepts() {
    let port = free_port();
    let r = Readiness { tcp: Some(TcpCheck { port }), ..readiness() };
    assert!(!check(&r, Duration::from_millis(200)));
    let _l = TcpListener::bind(("127.0.0.1", port)).unwrap();
    assert!(check(&r, Duration::from_millis(200)));
}

fn serve(status: &'static str) -> (u16, std::thread::JoinHandle<String>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let t = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut buf = [0u8; 1024];
        let n = s.read(&mut buf).unwrap();
        s.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n").as_bytes()).unwrap();
        String::from_utf8_lossy(&buf[..n]).into_owned()
    });
    (port, t)
}

#[test]
fn http_is_ready_on_2xx_only() {
    let (port, t) = serve("200 OK");
    let r = Readiness { http: Some(HttpCheck { port, path: "/healthz".into() }), ..readiness() };
    assert!(check(&r, Duration::from_secs(1)));
    assert!(t.join().unwrap().starts_with("GET /healthz HTTP/1.0\r\n"));

    let (port, t) = serve("503 Service Unavailable");
    let r = Readiness { http: Some(HttpCheck { port, path: "/".into() }), ..readiness() };
    assert!(!check(&r, Duration::from_secs(1)));
    t.join().unwrap();
}

#[test]
fn exec_is_ready_on_exit_0() {
    let ok = Readiness { exec: Some(ExecCheck { command: vec!["/bin/sh".into(), "-c".into(), "exit 0".into()] }), ..readiness() };
    let no = Readiness { exec: Some(ExecCheck { command: vec!["/bin/sh".into(), "-c".into(), "exit 1".into()] }), ..readiness() };
    let missing = Readiness { exec: Some(ExecCheck { command: vec!["/nonexistent/check".into()] }), ..readiness() };
    assert!(check(&ok, Duration::from_secs(2)));
    assert!(!check(&no, Duration::from_secs(2)));
    assert!(!check(&missing, Duration::from_secs(2)));
}

#[test]
fn exec_that_hangs_is_killed_and_not_ready() {
    let r = Readiness { exec: Some(ExecCheck { command: vec!["/bin/sh".into(), "-c".into(), "sleep 30".into()] }), ..readiness() };
    let t = Instant::now();
    assert!(!check(&r, Duration::from_millis(200)));
    assert!(t.elapsed() < Duration::from_secs(5));
}

#[test]
fn wait_ready_waits_until_it_passes_or_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let flag = dir.path().join("ready");
    let r = Readiness {
        exec: Some(ExecCheck { command: vec!["/bin/sh".into(), "-c".into(), format!("test -f {}", flag.display())] }),
        timeout_seconds: 1,
        ..readiness()
    };
    let never = AtomicBool::new(false);
    let t = Instant::now();
    assert!(!wait_ready(&r, &never), "times out");
    assert!(t.elapsed() >= Duration::from_millis(900));

    let f = flag.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(f, "").unwrap();
    });
    assert!(wait_ready(&r, &never), "passes once the flag is there");

    let cancelled = AtomicBool::new(true);
    assert!(!wait_ready(&r, &cancelled), "a cancelled probe is never ready");
}
