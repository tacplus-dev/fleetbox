// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

//! The supervisor against stand-in children: `sh` scripts that log what happens to them to a file in the box's
//! directory, and say they are ready by creating `<name>.ready` there.

use super::*;
use crate::health;
use std::path::Path;
use std::thread::JoinHandle;

/// A stand-in service: logs its start, gets ready after `ready_after`, logs the signal that stops it.
fn standin(ready_after: &str) -> String {
    format!(
        r#"trap 'echo "$N term" >> "$D/log"; exit 0' TERM; trap 'echo "$N int" >> "$D/log"; exit 0' INT; echo "$N start" >> "$D/log"; sleep {ready_after}; echo "$N ready" >> "$D/log"; touch "$D/$N.ready"; while :; do sleep 0.05; done"#
    )
}

/// One service of a manifest, running `script` with `D` and `N` set, ready when `<name>.ready` exists.
fn service(dir: &Path, name: &str, script: &str, extra: &str) -> String {
    let d = dir.display();
    format!(
        r#"  - name: {name}
    binary: /bin/sh
    args: ["-c", {script:?}]
    env: {{D: "{d}", N: {name}}}
    readiness: {{exec: {{command: ["/bin/sh", "-c", "test -f {d}/{name}.ready"]}}, interval_ms: 20, timeout_seconds: 5}}
{extra}"#
    )
}

struct Run {
    dir: tempfile::TempDir,
    notes: Receiver<Note>,
    handle: Handle,
    done: JoinHandle<Outcome>,
}

const RESTART: &str = "restart: {initial_backoff_ms: 100, max_backoff_ms: 400, unhealthy_after: 3, reset_after_seconds: 1}\n";

fn start(services: impl Fn(&Path) -> String, fail_fast: bool) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let text = format!("version: 1\nkernel: kernel\n{RESTART}services:\n{}", services(dir.path()));
    let m = Manifest::parse(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    let (tx, notes) = mpsc::channel();
    let opts = Options { fail_fast, run_dir: dir.path().join("run"), uni_config: None, observer: Some(tx) };
    let (sup, handle) = Supervisor::new(m, opts).unwrap();
    let done = std::thread::spawn(move || sup.run());
    Run { dir, notes, handle, done }
}

impl Run {
    /// The notes up to and including the first that `until` matches.
    fn until(&self, mut until: impl FnMut(&Note) -> bool) -> Vec<Note> {
        let mut seen = Vec::new();
        loop {
            match self.notes.recv_timeout(Duration::from_secs(15)) {
                Ok(n) => {
                    let last = until(&n);
                    seen.push(n);
                    if last {
                        return seen;
                    }
                }
                Err(_) => panic!("timed out; saw {seen:?}\nlog:\n{}", self.log()),
            }
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("log")).unwrap_or_default()
    }

    /// The box's health once `want` holds of it: the status is written after the note that changes it.
    fn health_when(&self, want: impl Fn(bool, &str) -> bool) -> (bool, String) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (healthy, status) = health::read(&self.dir.path().join("run")).unwrap_or((false, String::new()));
            if want(healthy, &status) || Instant::now() > deadline {
                return (healthy, status);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop(self, sig: i32) -> (Outcome, Vec<Note>, String) {
        self.handle.signal(sig);
        let outcome = self.done.join().unwrap();
        let notes: Vec<Note> = self.notes.try_iter().collect();
        let log = std::fs::read_to_string(self.dir.path().join("log")).unwrap_or_default();
        (outcome, notes, log)
    }
}

fn ready(name: &str) -> impl Fn(&Note) -> bool + '_ {
    move |n| *n == Note::Ready(name.into())
}

fn started_and_ready(notes: &[Note]) -> Vec<String> {
    notes
        .iter()
        .filter_map(|n| match n {
            Note::Started(s) => Some(format!("start {s}")),
            Note::Ready(s) => Some(format!("ready {s}")),
            _ => None,
        })
        .collect()
}

fn three(d: &Path) -> String {
    // Listed out of order: the order is the dependencies', the kernel always first.
    service(d, "b", &standin("0.1"), "    depends_on: [a]\n")
        + &service(d, "a", &standin("0.1"), "")
        + &service(d, "kernel", &standin("0.3"), "")
}

#[test]
fn starts_the_kernel_first_and_each_service_once_its_dependencies_are_ready() {
    let run = start(three, false);
    let notes = run.until(ready("b"));
    assert_eq!(started_and_ready(&notes), ["start kernel", "ready kernel", "start a", "ready a", "start b", "ready b"]);
    // The children agree: none started before what it depends on was ready.
    let log = run.log();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines, ["kernel start", "kernel ready", "a start", "a ready", "b start", "b ready"], "{log}");
    let (healthy, status) = run.health_when(|h, _| h);
    assert!(healthy, "{status}");
    run.stop(libc::SIGTERM);
}

#[test]
fn reports_starting_until_every_service_is_ready() {
    let run = start(three, false);
    run.until(|n| *n == Note::Started("kernel".into()));
    let (healthy, status) = run.health_when(|_, s| s.contains("kernel: starting"));
    assert!(!healthy && status.starts_with("starting\n"), "{status}");
    assert!(status.contains("kernel: starting") && status.contains("b: waiting"), "{status}");
    run.stop(libc::SIGTERM);
}

#[test]
fn forwards_sigterm_and_stops_in_reverse_order() {
    let run = start(three, false);
    run.until(ready("b"));
    let (outcome, notes, log) = run.stop(libc::SIGTERM);
    assert_eq!(outcome, Outcome::Stopped(libc::SIGTERM));
    assert_eq!(outcome.exit_code(), 0);
    let stopping: Vec<&Note> = notes.iter().filter(|n| matches!(n, Note::Stopping(..))).collect();
    assert_eq!(
        stopping,
        [&Note::Stopping("b".into(), libc::SIGTERM), &Note::Stopping("a".into(), libc::SIGTERM), &Note::Stopping("kernel".into(), libc::SIGTERM)]
    );
    // Each got the signal, in reverse order, and exited on it: none needed killing.
    let terms: Vec<&str> = log.lines().filter(|l| l.ends_with(" term")).collect();
    assert_eq!(terms, ["b term", "a term", "kernel term"], "{log}");
    assert!(!notes.iter().any(|n| matches!(n, Note::Killed(_))), "{notes:?}");
    assert!(notes.contains(&Note::Stopped("kernel".into(), Status(0))), "{notes:?}");
}

#[test]
fn forwards_sigint() {
    let run = start(three, false);
    run.until(ready("b"));
    let (outcome, _, log) = run.stop(libc::SIGINT);
    assert_eq!(outcome, Outcome::Stopped(libc::SIGINT));
    let ints: Vec<&str> = log.lines().filter(|l| l.ends_with(" int")).collect();
    assert_eq!(ints, ["b int", "a int", "kernel int"], "{log}");
}

#[test]
fn kills_a_service_that_does_not_stop_within_its_timeout() {
    // It ignores SIGTERM, and so does everything it starts.
    let deaf = r#"trap '' TERM; touch "$D/$N.ready"; while :; do sleep 0.05; done"#;
    let run = start(|d| service(d, "kernel", &standin("0"), "") + &service(d, "deaf", deaf, "    stop_timeout_seconds: 1\n"), false);
    run.until(ready("deaf"));
    let t = Instant::now();
    let (outcome, notes, log) = run.stop(libc::SIGTERM);
    assert_eq!(outcome, Outcome::Stopped(libc::SIGTERM));
    assert!(t.elapsed() >= Duration::from_secs(1));
    let i = notes.iter().position(|n| *n == Note::Killed("deaf".into())).expect("killed");
    assert!(notes[i + 1..].contains(&Note::Stopped("deaf".into(), Status(libc::SIGKILL))), "{notes:?}");
    // The kernel is still stopped after it, with its signal.
    assert!(log.ends_with("kernel term\n"), "{log}");
}

/// Exits 3 the first `n` times it runs, then runs.
fn crashes(n: u32) -> String {
    format!(
        r#"c=$(cat "$D/count" 2>/dev/null || echo 0); c=$((c+1)); echo $c > "$D/count"; echo "$N start $c" >> "$D/log"; [ $c -le {n} ] && exit 3; touch "$D/$N.ready"; while :; do sleep 0.05; done"#
    )
}

#[test]
fn restarts_a_crashed_service_with_backoff_and_reports_it_unhealthy() {
    let run = start(|d| service(d, "kernel", &standin("0"), "") + &service(d, "flaky", &crashes(100), ""), false);
    let mut starts = Vec::new();
    let mut delays = Vec::new();
    let notes = run.until(|n| {
        match n {
            Note::Started(s) if s == "flaky" => starts.push(Instant::now()),
            Note::Restarting(_, d) => delays.push(*d),
            _ => {}
        }
        delays.len() == 5
    });
    // 100ms, doubled per crash in a row, up to 400ms.
    let ms: Vec<u128> = delays.iter().map(|d| d.as_millis()).collect();
    assert_eq!(ms, [100, 200, 400, 400, 400]);
    for (i, w) in starts.windows(2).enumerate() {
        assert!(w[1] - w[0] >= delays[i], "restart {} came after {:?}, before its backoff {:?}", i + 1, w[1] - w[0], delays[i]);
    }
    assert!(notes.iter().all(|n| !matches!(n, Note::Crashed(s, _) if s == "kernel")));
    assert!(notes.contains(&Note::Crashed("flaky".into(), Status(3 << 8))), "exit 3: {notes:?}");
    // Unhealthy at the third crash in a row, once.
    let unhealthy: Vec<usize> = notes.iter().enumerate().filter(|(_, n)| **n == Note::Unhealthy("flaky".into())).map(|(i, _)| i).collect();
    assert_eq!(unhealthy.len(), 1, "{notes:?}");
    let crashes_before = notes[..unhealthy[0]].iter().filter(|n| matches!(n, Note::Crashed(..))).count();
    assert_eq!(crashes_before, 3);
    let (healthy, status) = run.health_when(|_, s| s.starts_with("unhealthy\n"));
    assert!(!healthy && status.starts_with("unhealthy\n") && status.contains("flaky: "), "{status}");
    assert!(status.contains(", unhealthy"), "{status}");
    run.stop(libc::SIGTERM);
}

#[test]
fn a_service_that_runs_again_is_healthy_again() {
    // Three crashes (unhealthy), then it runs: past reset_after_seconds it is healthy again.
    let run = start(|d| service(d, "kernel", &standin("0"), "") + &service(d, "flaky", &crashes(3), ""), false);
    let notes = run.until(|n| *n == Note::Healthy("flaky".into()));
    assert!(notes.contains(&Note::Unhealthy("flaky".into())));
    let (healthy, status) = run.health_when(|h, _| h);
    assert!(healthy, "{status}");
    assert!(status.contains("flaky: ready (restarts 3)"), "{status}");
    run.stop(libc::SIGTERM);
}

#[test]
fn a_service_never_ready_is_killed_and_restarted() {
    let never = r#"while :; do sleep 0.05; done"#;
    let run = start(
        |d| service(d, "kernel", &standin("0"), "") + &service(d, "stuck", never, "").replace("timeout_seconds: 5", "timeout_seconds: 1"),
        false,
    );
    let notes = run.until(|n| matches!(n, Note::Restarting(s, _) if s == "stuck"));
    let tail: Vec<&Note> = notes.iter().skip_while(|n| **n != Note::NotReady("stuck".into())).collect();
    assert_eq!(tail[1], &Note::Crashed("stuck".into(), Status(libc::SIGKILL)), "{notes:?}");
    run.until(|n| *n == Note::Started("stuck".into()));
    run.stop(libc::SIGTERM);
}

#[test]
fn fail_fast_exits_on_the_first_crash() {
    let run = start(
        |d| service(d, "kernel", &standin("0"), "") + &service(d, "a", &standin("0"), "") + &service(d, "flaky", &crashes(1), "    depends_on: [a]\n"),
        true,
    );
    let outcome = run.done.join().unwrap();
    let notes: Vec<Note> = run.notes.try_iter().collect();
    match &outcome {
        Outcome::Failed(why) => assert!(why.contains("flaky crashed: exited 3 (--fail-fast)"), "{why}"),
        o => panic!("{o:?}"),
    }
    assert_eq!(outcome.exit_code(), 1);
    assert!(!notes.iter().any(|n| matches!(n, Note::Restarting(..))), "{notes:?}");
    // The rest were stopped, in reverse order.
    let stopping: Vec<&Note> = notes.iter().filter(|n| matches!(n, Note::Stopping(..))).collect();
    assert_eq!(stopping, [&Note::Stopping("a".into(), libc::SIGTERM), &Note::Stopping("kernel".into(), libc::SIGTERM)]);
    let log = std::fs::read_to_string(run.dir.path().join("log")).unwrap();
    assert!(log.ends_with("a term\nkernel term\n"), "{log}");
    let (healthy, status) = health::read(&run.dir.path().join("run")).unwrap();
    assert!(!healthy && status.starts_with("failed\n"), "{status}");
}

#[test]
fn fail_fast_exits_when_a_service_is_never_ready() {
    let never = r#"while :; do sleep 0.05; done"#;
    let run = start(
        |d| service(d, "kernel", &standin("0"), "") + &service(d, "stuck", never, "").replace("timeout_seconds: 5", "timeout_seconds: 1"),
        true,
    );
    let outcome = run.done.join().unwrap();
    assert_eq!(outcome, Outcome::Failed("stuck not ready within 1s".into()));
}

#[test]
fn exits_non_zero_when_the_kernel_dies() {
    let dies = r#"touch "$D/$N.ready"; sleep 0.3; exit 7"#;
    let run = start(|d| service(d, "kernel", dies, "") + &service(d, "a", &standin("0"), ""), false);
    let outcome = run.done.join().unwrap();
    assert_eq!(outcome, Outcome::Failed("the kernel (kernel) died: exited 7".into()));
    assert_eq!(outcome.exit_code(), 1);
    let notes: Vec<Note> = run.notes.try_iter().collect();
    assert!(notes.contains(&Note::Stopping("a".into(), libc::SIGTERM)), "{notes:?}");
    assert!(!notes.iter().any(|n| matches!(n, Note::Restarting(..))), "{notes:?}");
}

#[test]
fn exits_non_zero_when_the_kernel_cannot_start() {
    let run = start(|d| service(d, "kernel", "", "").replace("binary: /bin/sh", "binary: /nonexistent/kernel"), false);
    match run.done.join().unwrap() {
        Outcome::Failed(why) => assert!(why.starts_with("the kernel (kernel) died: cannot start /nonexistent/kernel"), "{why}"),
        o => panic!("{o:?}"),
    }
}
