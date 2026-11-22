// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

//! The `fleetbox` binary as a container runs it: signalled by the process, not a handle; its services' lines
//! prefixed on its stdout and stderr; orphans reaped; config rendered from a uni-config file.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_fleetbox");

fn manifest(dir: &Path, services: &str) -> std::path::PathBuf {
    let p = dir.join("manifest.yaml");
    std::fs::write(&p, format!("version: 1\nkernel: kernel\nservices:\n{services}")).unwrap();
    p
}

struct Running {
    child: Child,
    out: JoinHandle<String>,
    err: JoinHandle<String>,
}

fn collect(mut r: impl Read + Send + 'static) -> JoinHandle<String> {
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = r.read_to_string(&mut s);
        s
    })
}

fn spawn(args: &[&str]) -> Running {
    let mut child = Command::new(BIN).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let out = collect(child.stdout.take().unwrap());
    let err = collect(child.stderr.take().unwrap());
    Running { child, out, err }
}

impl Running {
    fn signal(&self, sig: i32) {
        // SAFETY: kill(2) with plain integers.
        unsafe {
            libc::kill(self.child.id() as i32, sig);
        }
    }

    fn finish(mut self) -> (i32, String, String) {
        let deadline = Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                break s;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("fleetbox did not exit");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        (status.code().unwrap_or(-1), self.out.join().unwrap(), self.err.join().unwrap())
    }
}

fn health(run_dir: &Path) -> Output {
    Command::new(BIN).args(["health", "--run-dir", run_dir.to_str().unwrap()]).output().unwrap()
}

fn wait_healthy(run_dir: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !health(run_dir).status.success() {
        assert!(Instant::now() < deadline, "never healthy: {}", String::from_utf8_lossy(&health(run_dir).stdout));
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The children of `pid` that are zombies, from /proc.
fn zombies_of(pid: u32) -> Vec<String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { continue };
        // pid (comm) state ppid ...: comm may hold spaces, so split after its closing parenthesis.
        let Some(rest) = stat.rfind(')').map(|i| &stat[i + 2..]) else { continue };
        let f: Vec<&str> = rest.split_whitespace().collect();
        if f.len() > 1 && f[0] == "Z" && f[1] == pid.to_string() {
            out.push(stat);
        }
    }
    out
}

#[test]
fn runs_a_box_and_stops_it_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let run_dir = d.join("run");
    // The kernel writes to both streams and leaves orphans behind, which exit at once.
    let kernel = r#"echo hello out; echo hello err >&2; for i in 1 2 3; do (sleep 0.1 &); done; trap 'echo bye; exit 0' TERM; while :; do sleep 0.05; done"#;
    let app = r#"cat "$1"; cat "$2"; trap 'exit 0' TERM; while :; do sleep 0.05; done"#;
    let m = manifest(
        d,
        &format!(
            r#"  - name: kernel
    binary: /bin/sh
    args: ["-c", {kernel:?}]
  - name: app
    binary: /bin/sh
    args: ["-c", {app:?}, "app", "{{config}}", "{{secrets}}"]
    config: {{secrets: app}}
"#
        ),
    );
    let uni = d.join("uni-config.yaml");
    std::fs::write(&uni, "services:\n  app: \"greeting: hi\\n\"\nsecrets:\n  app: {token: s3cret}\n").unwrap();

    let box_ = spawn(&["--manifest", m.to_str().unwrap(), "--uni-config", uni.to_str().unwrap(), "--run-dir", run_dir.to_str().unwrap()]);
    wait_healthy(&run_dir);
    std::thread::sleep(Duration::from_millis(500));
    let pid = box_.child.id();
    assert_eq!(zombies_of(pid), Vec::<String>::new(), "orphans are reaped");

    box_.signal(libc::SIGTERM);
    let (code, out, err) = box_.finish();
    assert_eq!(code, 0, "stdout:\n{out}\nstderr:\n{err}");
    for want in ["kernel | hello out\n", "kernel | bye\n", "app | greeting: hi\n", "app | token: s3cret\n"] {
        assert!(out.contains(want), "{want:?} on stdout:\n{out}");
    }
    assert!(err.contains("kernel | hello err\n"), "stderr:\n{err}");
    assert!(err.contains("fleetbox | received SIGTERM\n") && err.ends_with("fleetbox | stopped\n"), "stderr:\n{err}");
    // The service's own lines go to its own stream only.
    assert!(!out.contains("hello err") && !err.contains("hello out"));
    let status = String::from_utf8(health(&run_dir).stdout).unwrap();
    assert!(status.starts_with("stopped\n"), "{status}");
}

#[test]
fn sigint_stops_it_too() {
    let dir = tempfile::tempdir().unwrap();
    let kernel = r#"trap 'echo got INT; exit 0' INT; while :; do sleep 0.05; done"#;
    let m = manifest(dir.path(), &format!("  - name: kernel\n    binary: /bin/sh\n    args: [\"-c\", {kernel:?}]\n"));
    let run_dir = dir.path().join("run");
    let box_ = spawn(&["--manifest", m.to_str().unwrap(), "--run-dir", run_dir.to_str().unwrap()]);
    wait_healthy(&run_dir);
    box_.signal(libc::SIGINT);
    let (code, out, err) = box_.finish();
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("kernel | got INT\n"), "{out}");
}

#[test]
fn fail_fast_exits_1_on_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let m = manifest(
        dir.path(),
        "  - name: kernel\n    binary: /bin/sh\n    args: [\"-c\", \"while :; do sleep 0.05; done\"]\n  - name: app\n    binary: /bin/sh\n    args: [\"-c\", \"sleep 0.2; exit 4\"]\n",
    );
    let run_dir = dir.path().join("run");
    let (code, _, err) = spawn(&["--manifest", m.to_str().unwrap(), "--run-dir", run_dir.to_str().unwrap(), "--fail-fast"]).finish();
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("fleetbox | failed: app crashed: exited 4 (--fail-fast)\n"), "{err}");
}

#[test]
fn a_bad_manifest_exits_2_before_anything_starts() {
    let dir = tempfile::tempdir().unwrap();
    let m = manifest(dir.path(), "  - name: kernel\n    binary: /bin/sh\n    depends_on: [kernel]\n");
    let (code, _, err) = spawn(&["--manifest", m.to_str().unwrap(), "--run-dir", dir.path().join("run").to_str().unwrap()]).finish();
    assert_eq!(code, 2);
    assert!(err.contains("depends on itself"), "{err}");

    let out = Command::new(BIN).args(["check", "--manifest", m.to_str().unwrap()]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let good = manifest(dir.path(), "  - name: kernel\n    binary: /bin/sh\n  - name: a\n    binary: /bin/sh\n");
    let out = Command::new(BIN).args(["check", "--manifest", good.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("version 1, 2 services, start order kernel, a"));
}

#[test]
fn image_writes_the_boxs_build_context_from_its_lock_and_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("box");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/image");
    let (lock, m) = (fixtures.join("box.lock"), fixtures.join("manifest.yaml"));
    let run = |lock: &Path| {
        Command::new(BIN)
            .args(["image", "--lock", lock.to_str().unwrap(), "--manifest", m.to_str().unwrap(), "--out", out.to_str().unwrap()])
            .output()
            .unwrap()
    };
    let o = run(&lock);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(std::fs::read(out.join("manifest.yaml")).unwrap(), std::fs::read(&m).unwrap());
    let d = std::fs::read_to_string(out.join("Dockerfile")).unwrap();
    assert!(d.contains("\nFROM localhost:5000/fleetbox@sha256:"), "{d}");
    assert!(d.contains("/bin/busybox /opt/gateway/busybox\n"), "{d}");
    assert!(d.contains("\nEXPOSE 8443\n"), "{d}");

    let bad = dir.path().join("bad.lock");
    std::fs::write(&bad, std::fs::read_to_string(&lock).unwrap().replace("name: eventbus", "name: oauth")).unwrap();
    let o = run(&bad);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("service oauth: in the box lock and not in the manifest"));

    let o = Command::new(BIN).args(["image", "--manifest", m.to_str().unwrap()]).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
}
