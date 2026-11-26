// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

//! The supervisor: starts the kernel, waits for it to be ready, then starts each service once its dependencies
//! are ready; restarts a crashed service with backoff; on SIGTERM or SIGINT stops every service in the reverse
//! of the start order, each with the signal and, past its stop timeout, SIGKILL.
//!
//! One thread owns all the state and acts on events: a child's exit (from the reaper), a probe's verdict, a
//! signal. Nothing else changes a service's state, so there is no lock to get wrong.

use crate::config::{self, Rendered, UniConfig};
use crate::logs;
use crate::manifest::{Error, Manifest, Service};
use crate::reaper::{self, Status};
use crate::readiness::{self, Probe};
use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a service killed with SIGKILL may take to be reaped before the supervisor stops waiting for it.
const KILL_WAIT: Duration = Duration::from_secs(5);
/// How long a stopped box waits for its services' last lines.
const LOG_DRAIN: Duration = Duration::from_secs(2);

pub struct Options {
    /// Exit on the first crash, of any service, instead of restarting it (for tests).
    pub fail_fast: bool,
    /// Where the rendered config (`config/`) and the box's status (`status.json`) are written.
    pub run_dir: PathBuf,
    pub uni_config: Option<PathBuf>,
    /// Told what happens, in order, for tests.
    pub observer: Option<Sender<Note>>,
}

/// What happened to a service, as an observer is told it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    Started(String),
    Ready(String),
    NotReady(String),
    Crashed(String, Status),
    Restarting(String, Duration),
    Unhealthy(String),
    Healthy(String),
    Stopping(String, i32),
    Killed(String),
    Stopped(String, Status),
}

/// Why the supervisor returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Stopped by this signal: every service stopped.
    Stopped(i32),
    /// The kernel died, or with `--fail-fast` a service crashed: every service stopped.
    Failed(String),
}

impl Outcome {
    pub fn exit_code(&self) -> i32 {
        match self {
            Outcome::Stopped(_) => 0,
            Outcome::Failed(_) => 1,
        }
    }
}

enum Event {
    Exited { pid: i32, status: Status },
    Ready { name: String, gen: u64 },
    NotReady { name: String, gen: u64 },
    Signal(i32),
}

/// Delivers signals to a running supervisor.
#[derive(Clone)]
pub struct Handle(Sender<Event>);

impl Handle {
    pub fn signal(&self, sig: i32) {
        let _ = self.0.send(Event::Signal(sig));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Waiting,
    Starting,
    Ready,
    Backoff(Instant),
    Stopping,
    Stopped,
}

struct Svc {
    spec: Service,
    deps: Vec<String>,
    rendered: Rendered,
    state: State,
    pid: Option<i32>,
    /// Counts the service's starts, so a probe's verdict on an earlier start is ignored.
    gen: u64,
    probe: Option<Probe>,
    crashes: u32,
    restarts: u32,
    started_at: Option<Instant>,
    unhealthy: bool,
}

pub struct Supervisor {
    m: Manifest,
    opts: Options,
    order: Vec<String>,
    svcs: BTreeMap<String, Svc>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    all_ready_once: bool,
    last_status: String,
    /// The threads forwarding the services' output, drained before the box stops.
    forwarders: Vec<JoinHandle<()>>,
}

impl Supervisor {
    /// Renders every service's config, so a box that cannot run fails before anything starts.
    pub fn new(m: Manifest, opts: Options) -> Result<(Supervisor, Handle), Error> {
        let uni = opts.uni_config.as_deref().map(UniConfig::load).transpose()?;
        std::fs::create_dir_all(&opts.run_dir).map_err(|e| Error(format!("run dir {}: {e}", opts.run_dir.display())))?;
        let mut rendered = config::render_all(&m, uni.as_ref(), &opts.run_dir.join("config"))?;
        let order = m.start_order()?;
        let svcs = m
            .services
            .iter()
            .map(|s| {
                let svc = Svc {
                    spec: s.clone(),
                    deps: m.dependencies(s),
                    rendered: rendered.remove(&s.name).unwrap_or_default(),
                    state: State::Waiting,
                    pid: None,
                    gen: 0,
                    probe: None,
                    crashes: 0,
                    restarts: 0,
                    started_at: None,
                    unhealthy: false,
                };
                (s.name.clone(), svc)
            })
            .collect();
        let (tx, rx) = mpsc::channel();
        let handle = Handle(tx.clone());
        let sup = Supervisor { m, opts, order, svcs, tx, rx, all_ready_once: false, last_status: String::new(), forwarders: Vec::new() };
        Ok((sup, handle))
    }

    pub fn run(mut self) -> Outcome {
        logs::say(format!("starting {}, the kernel ({}) first", self.order.join(", "), self.m.kernel));
        loop {
            if let Err(why) = self.start_eligible() {
                return self.shutdown(libc::SIGTERM, Outcome::Failed(why));
            }
            self.reset_crash_counts();
            self.write_status(None);
            let event = match self.rx.recv_timeout(self.next_wakeup()) {
                Ok(e) => e,
                Err(RecvTimeoutError::Timeout) => {
                    self.restarts_due();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => unreachable!("the supervisor holds a sender"),
            };
            let r = match event {
                Event::Signal(sig) => {
                    logs::say(format!("received {}", signal_name(sig)));
                    return self.shutdown(sig, Outcome::Stopped(sig));
                }
                Event::Exited { pid, status } => self.on_exit(pid, status),
                Event::Ready { name, gen } => {
                    self.on_ready(&name, gen);
                    Ok(())
                }
                Event::NotReady { name, gen } => self.on_not_ready(&name, gen),
            };
            if let Err(why) = r {
                return self.shutdown(libc::SIGTERM, Outcome::Failed(why));
            }
            self.restarts_due();
        }
    }

    fn note(&self, n: Note) {
        if let Some(o) = &self.opts.observer {
            let _ = o.send(n);
        }
    }

    fn svc(&mut self, name: &str) -> &mut Svc {
        self.svcs.get_mut(name).expect("a service of the manifest")
    }

    /// Starts, in order, each waiting service whose dependencies are all ready.
    fn start_eligible(&mut self) -> Result<(), String> {
        for name in self.order.clone() {
            let s = &self.svcs[&name];
            if s.state == State::Waiting && s.deps.iter().all(|d| self.svcs[d].state == State::Ready) {
                self.start(&name)?;
            }
        }
        Ok(())
    }

    fn start(&mut self, name: &str) -> Result<(), String> {
        let tx = self.tx.clone();
        let s = self.svc(name);
        s.gen += 1;
        let mut cmd = Command::new(&s.spec.binary);
        cmd.args(s.spec.args.iter().map(|a| s.rendered.substitute(a)))
            .envs(s.spec.env.iter().map(|(k, v)| (k, s.rendered.substitute(v))))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Its own process group, so a signal reaches whatever the service starts as well.
            .process_group(0);
        let spawned = reaper::spawn(&mut cmd, move |pid, status| {
            let _ = tx.send(Event::Exited { pid, status });
        });
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                let why = format!("cannot start {}: {e}", s.spec.binary);
                logs::say(format!("{name}: {why}"));
                return self.crashed(name, why);
            }
        };
        let pid = child.id() as i32;
        self.forwarders.retain(|f| !f.is_finished());
        if let Some(out) = child.stdout.take() {
            self.forwarders.extend(logs::forward(name.to_string(), out, false));
        }
        if let Some(err) = child.stderr.take() {
            self.forwarders.extend(logs::forward(name.to_string(), err, true));
        }
        let s = self.svc(name);
        s.pid = Some(pid);
        s.started_at = Some(Instant::now());
        s.state = State::Starting;
        let gen = s.gen;
        let readiness = s.spec.readiness.clone();
        logs::say(format!("{name} started (pid {pid})"));
        self.note(Note::Started(name.to_string()));
        match readiness {
            None => self.on_ready(name, gen),
            Some(r) => {
                let cancel = Arc::new(AtomicBool::new(false));
                self.svc(name).probe = Some(Probe(cancel.clone()));
                let tx = self.tx.clone();
                let name = name.to_string();
                std::thread::spawn(move || {
                    let e = if readiness::wait_ready(&r, &cancel) {
                        Event::Ready { name, gen }
                    } else {
                        Event::NotReady { name, gen }
                    };
                    let _ = tx.send(e);
                });
            }
        }
        Ok(())
    }

    fn on_ready(&mut self, name: &str, gen: u64) {
        let s = self.svc(name);
        if s.gen != gen || s.state != State::Starting {
            return;
        }
        s.state = State::Ready;
        s.probe = None;
        logs::say(format!("{name} ready"));
        self.note(Note::Ready(name.to_string()));
    }

    fn on_not_ready(&mut self, name: &str, gen: u64) -> Result<(), String> {
        let fail_fast = self.opts.fail_fast;
        let kernel = name == self.m.kernel;
        let s = self.svc(name);
        if s.gen != gen || s.state != State::Starting {
            return Ok(());
        }
        let timeout = s.spec.readiness.as_ref().map_or(0, |r| r.timeout_seconds);
        let why = format!("not ready within {timeout}s");
        logs::say(format!("{name} {why}"));
        self.note(Note::NotReady(name.to_string()));
        if kernel || fail_fast {
            return Err(format!("{name} {why}"));
        }
        // Killed, and its exit is a crash like any other.
        if let Some(pid) = self.svc(name).pid {
            reaper::kill_group(pid, libc::SIGKILL);
        }
        Ok(())
    }

    fn on_exit(&mut self, pid: i32, status: Status) -> Result<(), String> {
        let Some(name) = self.svcs.iter().find(|(_, s)| s.pid == Some(pid)).map(|(n, _)| n.clone()) else {
            return Ok(());
        };
        let s = self.svc(&name);
        s.pid = None;
        if let Some(p) = s.probe.take() {
            p.cancel();
        }
        // Whatever the service left running in its group goes with it.
        reaper::kill_group(pid, libc::SIGKILL);
        logs::say(format!("{name} {status}"));
        self.note(Note::Crashed(name.clone(), status));
        self.crashed(&name, status.to_string())
    }

    /// A service that stopped on its own: the kernel's death, or any crash with `--fail-fast`, ends the box;
    /// any other is restarted after its backoff.
    fn crashed(&mut self, name: &str, why: String) -> Result<(), String> {
        if name == self.m.kernel {
            return Err(format!("the kernel ({name}) died: {why}"));
        }
        if self.opts.fail_fast {
            return Err(format!("{name} crashed: {why} (--fail-fast)"));
        }
        let policy = self.m.restart.clone();
        let s = self.svc(name);
        s.crashes += 1;
        let crashes = s.crashes;
        let delay = policy.backoff(crashes);
        s.state = State::Backoff(Instant::now() + delay);
        let newly_unhealthy = crashes >= policy.unhealthy_after && !s.unhealthy;
        s.unhealthy |= newly_unhealthy;
        logs::say(format!("{name} restarts in {}ms ({crashes} crashes in a row)", delay.as_millis()));
        self.note(Note::Restarting(name.to_string(), delay));
        if newly_unhealthy {
            logs::say(format!("{name} is unhealthy: {crashes} crashes in a row"));
            self.note(Note::Unhealthy(name.to_string()));
        }
        Ok(())
    }

    fn restarts_due(&mut self) {
        let now = Instant::now();
        for s in self.svcs.values_mut() {
            if let State::Backoff(at) = s.state {
                if at <= now {
                    s.state = State::Waiting;
                    s.restarts += 1;
                }
            }
        }
    }

    /// A service ready for `reset_after_seconds` since its start has its crash count reset, and is healthy again.
    fn reset_crash_counts(&mut self) {
        let after = Duration::from_secs(self.m.restart.reset_after_seconds);
        let mut healthy = Vec::new();
        for (name, s) in self.svcs.iter_mut() {
            if s.state == State::Ready && s.crashes > 0 && s.started_at.is_some_and(|t| t.elapsed() >= after) {
                s.crashes = 0;
                if s.unhealthy {
                    s.unhealthy = false;
                    healthy.push(name.clone());
                }
            }
        }
        for name in healthy {
            logs::say(format!("{name} is healthy again"));
            self.note(Note::Healthy(name));
        }
    }

    fn next_wakeup(&self) -> Duration {
        let now = Instant::now();
        let after = Duration::from_secs(self.m.restart.reset_after_seconds);
        let mut next = now + Duration::from_secs(1);
        for s in self.svcs.values() {
            match s.state {
                State::Backoff(at) => next = next.min(at),
                State::Ready if s.crashes > 0 => {
                    if let Some(t) = s.started_at {
                        next = next.min(t + after);
                    }
                }
                _ => {}
            }
        }
        next.saturating_duration_since(now).max(Duration::from_millis(5))
    }

    /// Stops every service in the reverse of the start order: `sig`, then SIGKILL past its stop timeout.
    fn shutdown(mut self, sig: i32, outcome: Outcome) -> Outcome {
        match &outcome {
            Outcome::Stopped(_) => logs::say(format!("stopping every service with {}", signal_name(sig))),
            Outcome::Failed(why) => logs::say(format!("{why}: stopping every service")),
        }
        for s in self.svcs.values_mut() {
            if let Some(p) = s.probe.take() {
                p.cancel();
            }
            if s.pid.is_none() {
                s.state = State::Stopped;
            }
        }
        self.write_status(Some("stopping"));
        for name in self.order.clone().into_iter().rev() {
            let Some(pid) = self.svcs[&name].pid else { continue };
            let timeout = Duration::from_secs(self.svcs[&name].spec.stop_timeout_seconds);
            self.svc(&name).state = State::Stopping;
            logs::say(format!("stopping {name} with {}", signal_name(sig)));
            self.note(Note::Stopping(name.clone(), sig));
            reaper::kill_group(pid, sig);
            if !self.wait_exit(pid, Instant::now() + timeout) {
                logs::say(format!("{name} did not stop within {}s: killing it", timeout.as_secs()));
                self.note(Note::Killed(name.clone()));
                reaper::kill_group(pid, libc::SIGKILL);
                self.wait_exit(pid, Instant::now() + KILL_WAIT);
            }
        }
        self.drain_logs(Instant::now() + LOG_DRAIN);
        self.write_status(Some(match outcome {
            Outcome::Stopped(_) => "stopped",
            Outcome::Failed(_) => "failed",
        }));
        match &outcome {
            Outcome::Stopped(_) => logs::say("stopped"),
            Outcome::Failed(why) => logs::say(format!("failed: {why}")),
        }
        outcome
    }

    /// Waits until every service's output is forwarded, or `deadline` passes: an orphan that left its group may
    /// hold a stream open.
    fn drain_logs(&mut self, deadline: Instant) {
        while self.forwarders.iter().any(|f| !f.is_finished()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Waits until `pid` is reaped or `deadline` passes, noting any other service that exits meanwhile.
    fn wait_exit(&mut self, pid: i32, deadline: Instant) -> bool {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(Event::Exited { pid: p, status }) => {
                    if let Some(name) = self.svcs.iter().find(|(_, s)| s.pid == Some(p)).map(|(n, _)| n.clone()) {
                        let s = self.svc(&name);
                        s.pid = None;
                        s.state = State::Stopped;
                        reaper::kill_group(p, libc::SIGKILL);
                        logs::say(format!("{name} stopped: {status}"));
                        self.note(Note::Stopped(name, status));
                    }
                    if p == pid {
                        return true;
                    }
                }
                Ok(Event::Signal(sig)) => logs::say(format!("received {} while stopping", signal_name(sig))),
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    }

    /// The box's state in `run_dir/status.json`, for `fleetbox health` (docs/USER.md, Health).
    fn write_status(&mut self, fixed: Option<&str>) {
        let all_ready = self.svcs.values().all(|s| s.state == State::Ready);
        self.all_ready_once |= all_ready;
        let state = fixed.unwrap_or(if self.svcs.values().any(|s| s.unhealthy) {
            "unhealthy"
        } else if all_ready {
            "healthy"
        } else if self.all_ready_once {
            "degraded"
        } else {
            "starting"
        });
        let services: serde_json::Map<String, serde_json::Value> = self
            .order
            .iter()
            .map(|n| {
                let s = &self.svcs[n];
                let st = match s.state {
                    State::Waiting => "waiting",
                    State::Starting => "starting",
                    State::Ready => "ready",
                    State::Backoff(_) => "restarting",
                    State::Stopping => "stopping",
                    State::Stopped => "stopped",
                };
                let v = serde_json::json!({
                    "state": st, "pid": s.pid, "restarts": s.restarts,
                    "crashes_in_a_row": s.crashes, "unhealthy": s.unhealthy,
                });
                (n.clone(), v)
            })
            .collect();
        let doc = serde_json::json!({"state": state, "services": services}).to_string();
        if doc == self.last_status {
            return;
        }
        let path = self.opts.run_dir.join("status.json");
        let tmp = self.opts.run_dir.join("status.json.tmp");
        if std::fs::write(&tmp, &doc).and_then(|_| std::fs::rename(&tmp, &path)).is_err() {
            logs::say(format!("cannot write {}", path.display()));
        }
        self.last_status = doc;
    }
}

pub fn signal_name(sig: i32) -> String {
    match sig {
        libc::SIGTERM => "SIGTERM".into(),
        libc::SIGINT => "SIGINT".into(),
        libc::SIGKILL => "SIGKILL".into(),
        n => format!("signal {n}"),
    }
}

#[cfg(test)]
#[path = "supervisor_tests.rs"]
mod tests;
