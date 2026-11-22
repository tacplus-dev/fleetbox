// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

//! Every child of the process, reaped in one place.
//!
//! As PID 1 the supervisor inherits every orphan in the container, so it reaps with `waitpid(-1)`: whatever
//! exits, a service, a readiness probe or a grandchild some service left behind. A process started here is
//! registered with a callback for its exit; an exit nobody registered is an orphan's, reaped and dropped. The
//! table is locked across spawn and registration and across each `waitpid`, so no child exits unregistered.

use signal_hook::consts::SIGCHLD;
use signal_hook::iterator::Signals;
use std::collections::HashMap;
use std::io;
use std::process::{Child, Command};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

type OnExit = Box<dyn FnOnce(i32, Status) + Send>;

static TABLE: OnceLock<Mutex<HashMap<i32, OnExit>>> = OnceLock::new();

/// A child's wait status, as `waitpid` gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status(pub i32);

impl Status {
    /// The exit code, when the child exited.
    pub fn code(self) -> Option<i32> {
        libc::WIFEXITED(self.0).then(|| libc::WEXITSTATUS(self.0))
    }

    /// The signal that killed the child, when one did.
    pub fn signal(self) -> Option<i32> {
        libc::WIFSIGNALED(self.0).then(|| libc::WTERMSIG(self.0))
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.code(), self.signal()) {
            (Some(c), _) => write!(f, "exited {c}"),
            (_, Some(s)) => write!(f, "killed by signal {s}"),
            _ => write!(f, "wait status {}", self.0),
        }
    }
}

fn table() -> &'static Mutex<HashMap<i32, OnExit>> {
    TABLE.get_or_init(|| {
        // SIGCHLD is registered before the first child exists, so none is missed; a signal that arrives while
        // the reaper is busy is coalesced, and each pass reaps until nothing is left.
        let mut signals = Signals::new([SIGCHLD]).expect("register SIGCHLD");
        std::thread::Builder::new()
            .name("reaper".into())
            .spawn(move || loop {
                for _ in signals.wait() {}
                reap();
            })
            .expect("start the reaper");
        // A slow sweep too: a child whose SIGCHLD went to a thread that blocks it is still reaped.
        std::thread::Builder::new()
            .name("reaper-sweep".into())
            .spawn(|| loop {
                std::thread::sleep(Duration::from_millis(500));
                reap();
            })
            .expect("start the reaper sweep");
        Mutex::new(HashMap::new())
    })
}

/// Spawns `cmd` and calls `on_exit` with its pid and status once it is reaped. Never wait on the returned
/// `Child`: the reaper owns its exit.
pub fn spawn(cmd: &mut Command, on_exit: impl FnOnce(i32, Status) + Send + 'static) -> io::Result<Child> {
    let mut t = table().lock().unwrap_or_else(|e| e.into_inner());
    let child = cmd.spawn()?;
    t.insert(child.id() as i32, Box::new(on_exit));
    Ok(child)
}

/// Reaps every child that has exited, calling the callbacks outside the lock.
pub fn reap() {
    let mut done = Vec::new();
    {
        let mut t = table().lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let mut status = 0;
            // SAFETY: waitpid with a valid pointer to a local int.
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid <= 0 {
                break;
            }
            if let Some(f) = t.remove(&pid) {
                done.push((pid, Status(status), f));
            }
        }
    }
    for (pid, status, f) in done {
        f(pid, status);
    }
}

/// Sends `sig` to the process group `pgid` (each child leads its own); a group already gone is no error.
pub fn kill_group(pgid: i32, sig: i32) {
    // SAFETY: kill(2) with plain integers.
    unsafe {
        libc::kill(-pgid, sig);
    }
}
