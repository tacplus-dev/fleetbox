// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

use super::*;
use serde_json::Value;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

/// The test fleet: the image smoke's (tests/image/), whose gateway listens on 8443.
fn fleet() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/image/manifest.yaml")
}

fn options() -> Options {
    Options {
        image: "localhost:5000/fleetbox-box@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        ..Options::default()
    }
}

fn yaml(text: &str) -> Value {
    serde_saphyr::from_str(text).unwrap()
}

/// A config directory, as `mkdir` makes it. Under root (a CI job, say) it is given to `nobody`: init refuses a
/// directory of root's, and as root it writes as the directory's owner.
fn config_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: geteuid(2) takes nothing and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        std::os::unix::fs::chown(dir.path(), Some(65534), Some(65534)).unwrap();
    }
    dir
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn defaults_are_the_gateway_on_8443_on_localhost() {
    let o = Options::default();
    assert_eq!((o.port, o.domain.as_str(), o.gateway.as_str()), (8443, "localhost", "gateway"));
}

#[test]
fn the_uni_config_carries_the_domain_and_the_gateways_port() {
    let o = Options { domain: "box.example.com".into(), port: 9443, ..options() };
    let u = yaml(&uni_config(&o));
    assert_eq!(u["operator"]["fqdn"], "box.example.com");
    assert_eq!(u["operator"]["port"], 9443);
    // The shape the supervisor reads its services' config and secrets from (src/config.rs).
    assert!(u["services"].as_object().unwrap().is_empty());
    assert!(u["secrets"].as_object().unwrap().is_empty());
    crate::config::UniConfig::parse(&uni_config(&o)).unwrap();
}

#[test]
fn compose_runs_the_box_from_the_config_directory_as_its_owner() {
    let m = Manifest::load(&fleet()).unwrap();
    let c = yaml(&compose(&m, &options(), (1000, 1001)).unwrap());
    let b = &c["services"]["box"];
    assert_eq!(b["image"], options().image.as_str());
    assert_eq!(b["user"], "1000:1001");
    assert_eq!(b["volumes"], serde_json::json!(["./:/config:ro"]));
    assert_eq!(
        b["command"],
        serde_json::json!(["run", "--manifest", "/config/manifest.yaml", "--uni-config", "/config/uni-config.yaml"])
    );
    // The run directory is the box user's alone, as in the image.
    assert_eq!(b["tmpfs"], serde_json::json!(["/run/fleetbox:uid=1000,gid=1001,mode=0700"]));
    assert_eq!(b["cap_drop"], serde_json::json!(["ALL"]));
    assert_eq!(b["security_opt"], serde_json::json!(["no-new-privileges:true"]));
    // Every service stopped in turn, each within its stop timeout (the test fleet's three take the default 10s).
    assert_eq!(b["stop_grace_period"], "40s");
    // Rootless Podman maps the directory's owner to itself only with keep-id, which Docker has not: the Compose
    // specification's key, from the environment (FLEETBOX_USERNS=keep-id podman compose up).
    assert!(compose(&m, &options(), (1000, 1001)).unwrap().contains("    userns_mode: \"${FLEETBOX_USERNS:-}\"\n"));
    assert_eq!(b.as_object().unwrap().keys().filter(|k| !COMPOSE_SPEC_KEYS.contains(&k.as_str())).count(), 0, "{b}");
}

/// The service keys compose.yaml may use: the Compose specification's, which every implementation reads.
const COMPOSE_SPEC_KEYS: &[&str] = &[
    "image", "user", "command", "volumes", "tmpfs", "ports", "cap_drop", "security_opt", "stop_grace_period", "restart",
    "userns_mode",
];

#[test]
fn compose_publishes_the_configs_port_on_the_gateways() {
    let m = Manifest::load(&fleet()).unwrap();
    let c = yaml(&compose(&m, &options(), (1000, 1000)).unwrap());
    assert_eq!(c["services"]["box"]["ports"], serde_json::json!(["8443:8443"]));
    let c = yaml(&compose(&m, &Options { port: 443, ..options() }, (1000, 1000)).unwrap());
    assert_eq!(c["services"]["box"]["ports"], serde_json::json!(["443:8443"]));
}

#[test]
fn compose_refuses_a_gateway_the_manifest_lacks_or_with_no_port() {
    let m = Manifest::load(&fleet()).unwrap();
    let e = compose(&m, &Options { gateway: "edge".into(), ..options() }, (1, 1)).unwrap_err().to_string();
    assert!(e.contains("edge"), "{e}");
    let no_port = Manifest::parse("version: 1\nkernel: kernel\nservices:\n  - {name: kernel, binary: /bin/k}\n  - {name: gateway, binary: /bin/g}\n").unwrap();
    let e = compose(&no_port, &options(), (1, 1)).unwrap_err().to_string();
    assert!(e.contains("gateway") && e.contains("port"), "{e}");
}

#[test]
fn refuses_options_a_file_would_misread() {
    for bad in [
        Options { domain: "".into(), ..options() },
        Options { domain: "a b".into(), ..options() },
        Options { domain: "box..example".into(), ..options() },
        Options { domain: "-box.example".into(), ..options() },
        Options { port: 0, ..options() },
        Options { image: "".into(), ..options() },
        Options { image: "box\nservices: {}".into(), ..options() },
        Options { image: "box latest".into(), ..options() },
    ] {
        assert!(bad.validate().is_err(), "{:?} {} {:?}", bad.domain, bad.port, bad.image);
    }
    options().validate().unwrap();
    Options { domain: "Box-1.example.com".into(), ..options() }.validate().unwrap();
}

#[test]
fn a_password_is_random_and_long() {
    let (a, b) = (password().unwrap(), password().unwrap());
    assert_ne!(a, b);
    assert_eq!(a.len(), 32);
    assert!(a.chars().all(|c| c.is_ascii_alphanumeric()), "{a}");
    let doc = yaml(&admin(&a));
    assert_eq!(doc["username"], "admin");
    assert_eq!(doc["password"], a.as_str());
}

#[test]
fn files_go_to_the_directorys_owner() {
    // Root writes as the directory's owner; its owner writes as itself; anyone else is refused, and so is a
    // directory of root's, whose box would run as root.
    assert_eq!(chown_to((1000, 1000), 0).unwrap(), Some((1000, 1000)));
    assert_eq!(chown_to((1000, 1000), 1000).unwrap(), None);
    let e = chown_to((1000, 1000), 65532).unwrap_err().to_string();
    assert!(e.contains("1000") && e.contains("--user"), "{e}");
    let e = chown_to((0, 0), 0).unwrap_err().to_string();
    assert!(e.contains("root") && e.contains("mkdir"), "{e}");
    // Rootless Podman shows the host user's directory as root's without keep-id.
    let e = chown_to((0, 0), 1000).unwrap_err().to_string();
    assert!(e.contains("--userns=keep-id"), "{e}");
}

#[test]
fn writes_the_config_directory_from_a_test_fleet() {
    let dir = config_dir();
    let d = dir.path();
    init(d, &fleet(), &options()).unwrap();

    assert_eq!(std::fs::read(d.join(MANIFEST)).unwrap(), std::fs::read(fleet()).unwrap());
    assert_eq!(yaml(&std::fs::read_to_string(d.join(UNI_CONFIG)).unwrap())["operator"]["port"], 8443);
    let me = std::fs::metadata(d).unwrap();
    let c = yaml(&std::fs::read_to_string(d.join(COMPOSE)).unwrap());
    assert_eq!(c["services"]["box"]["user"], format!("{}:{}", me.uid(), me.gid()));
    let admin = yaml(&std::fs::read_to_string(d.join(ADMIN)).unwrap());
    assert_eq!(admin["password"].as_str().unwrap().len(), 32);
    assert_eq!(std::fs::read_to_string(d.join(".gitignore")).unwrap().lines().last(), Some("admin.yaml"));

    // The secrets are their owner's alone.
    assert_eq!(mode(&d.join(ADMIN)), 0o600);
    assert_eq!(mode(&d.join(UNI_CONFIG)), 0o600);
    assert_eq!(mode(&d.join(COMPOSE)), 0o644);
    assert_eq!(mode(&d.join(START)), 0o755);
    assert_eq!(mode(&d.join(STOP)), 0o755);
    for f in [ADMIN, UNI_CONFIG, COMPOSE, START, STOP, MANIFEST, ".gitignore"] {
        let m = std::fs::metadata(d.join(f)).unwrap();
        assert_eq!((m.uid(), m.gid()), (me.uid(), me.gid()), "{f}");
    }
}

#[test]
fn rerunning_regenerates_everything_but_the_admin_account() {
    let dir = config_dir();
    let d = dir.path();
    init(d, &fleet(), &options()).unwrap();
    let admin = std::fs::read(d.join(ADMIN)).unwrap();
    std::fs::write(d.join(COMPOSE), "edited").unwrap();

    init(d, &fleet(), &Options { port: 9443, domain: "box.example.com".into(), ..options() }).unwrap();
    assert_eq!(std::fs::read(d.join(ADMIN)).unwrap(), admin, "the admin account is never overwritten");
    let c = yaml(&std::fs::read_to_string(d.join(COMPOSE)).unwrap());
    assert_eq!(c["services"]["box"]["ports"], serde_json::json!(["9443:8443"]));
    let u = yaml(&std::fs::read_to_string(d.join(UNI_CONFIG)).unwrap());
    assert_eq!(u["operator"]["fqdn"], "box.example.com");
}

#[test]
fn writes_nothing_for_a_bad_manifest_or_options() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.yaml");
    std::fs::write(&bad, "version: 2\n").unwrap();
    let config = config_dir();
    let out = config.path();
    assert!(init(out, &bad, &options()).unwrap_err().to_string().contains("version 2"));
    assert!(init(out, &fleet(), &Options { gateway: "edge".into(), ..options() }).is_err());
    assert!(init(out, &fleet(), &Options { port: 0, ..options() }).is_err());
    assert_eq!(std::fs::read_dir(out).unwrap().count(), 0);
}

#[test]
fn refuses_a_directory_that_is_not_there() {
    let dir = tempfile::tempdir().unwrap();
    let e = init(&dir.path().join("missing"), &fleet(), &options()).unwrap_err().to_string();
    assert!(e.contains("missing"), "{e}");
}

#[test]
fn replaces_a_symlink_rather_than_writing_through_it() {
    let (dir, config) = (tempfile::tempdir().unwrap(), config_dir());
    let d = config.path();
    let elsewhere = dir.path().join("elsewhere");
    std::fs::write(&elsewhere, "untouched").unwrap();
    std::os::unix::fs::symlink(&elsewhere, d.join(UNI_CONFIG)).unwrap();
    init(d, &fleet(), &options()).unwrap();
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "untouched");
    assert!(!std::fs::symlink_metadata(d.join(UNI_CONFIG)).unwrap().file_type().is_symlink());
}

/// Runs a script `init` wrote with only the runtimes `found` on the PATH, each a stand-in that logs its arguments
/// (podman also answers `info` as rootless or not). Returns its status, its stderr and the runtimes' log.
fn script(d: &Path, name: &str, found: &[&str], env: &[(&str, &str)]) -> (i32, String, Vec<String>) {
    let bin = tempfile::tempdir().unwrap();
    let log = bin.path().join("log");
    for r in found {
        let p = bin.path().join(r);
        std::fs::write(&p, "#!/bin/sh\necho \"${0##*/} $*\" >> \"$LOG\"\n[ \"$1\" != info ] || echo \"$ROOTLESS\"\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut cmd = std::process::Command::new(d.join(name));
    cmd.env_clear()
        .env("PATH", bin.path())
        .env("LOG", &log)
        .env("ROOTLESS", "false")
        .envs(env.iter().copied())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    // The supervisor's tests share this process and its reaper, which waits on every child: the script's exit
    // is the reaper's to collect, or `wait` races it and fails with ECHILD.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut pipe = crate::reaper::spawn(&mut cmd, move |_, status| tx.send(status).unwrap()).unwrap().stderr.unwrap();
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut pipe, &mut stderr).unwrap();
    let status = rx.recv().unwrap();
    let log = std::fs::read_to_string(&log).unwrap_or_default();
    (status.code().unwrap_or(-1), stderr, log.lines().map(String::from).collect())
}

fn initialized() -> (tempfile::TempDir, String) {
    let dir = config_dir();
    init(dir.path(), &fleet(), &Options { port: 9443, ..options() }).unwrap();
    let me = std::fs::metadata(dir.path()).unwrap();
    let box_args = format!(
        "--name fleetbox --user {u}:{g} --volume {d}:/config:ro --publish 9443:8443",
        u = me.uid(),
        g = me.gid(),
        d = dir.path().display()
    );
    (dir, box_args)
}

const RUN: &str = "run --manifest /config/manifest.yaml --uni-config /config/uni-config.yaml";

#[test]
fn start_sh_runs_the_box_on_the_first_runtime_found_as_compose_yaml_does() {
    let (dir, box_args) = initialized();
    let me = std::fs::metadata(dir.path()).unwrap();
    let hardened = format!(
        "--tmpfs /run/fleetbox:uid={},gid={},mode=0700 --cap-drop ALL --security-opt no-new-privileges --stop-timeout 40 \
         --restart unless-stopped",
        me.uid(),
        me.gid()
    );
    let (code, err, log) = script(dir.path(), START, &["podman", "docker", "nerdctl", "container"], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("docker"), "it says which: {err}");
    // A box stopped before is removed first, so its name is free.
    assert_eq!(log, [
        "docker rm fleetbox".to_string(),
        format!("docker run --detach {box_args} {hardened} {} {RUN}", options().image),
    ]);

    let (code, err, log) = script(dir.path(), START, &["nerdctl", "container"], &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(log.last().unwrap(), &format!("nerdctl run --detach {box_args} {hardened} {} {RUN}", options().image));
}

#[test]
fn start_sh_maps_the_owner_to_itself_under_rootless_podman() {
    let (dir, box_args) = initialized();
    let (code, err, log) = script(dir.path(), START, &["podman"], &[("ROOTLESS", "true")]);
    assert_eq!(code, 0, "{err}");
    assert!(log.last().unwrap().starts_with(&format!("podman run --detach {box_args} ")), "{log:?}");
    assert!(log.last().unwrap().contains(" --userns keep-id "), "{log:?}");
    let (_, _, log) = script(dir.path(), START, &["podman"], &[]);
    assert!(!log.last().unwrap().contains("--userns"), "rootful Podman has no keep-id: {log:?}");
}

#[test]
fn start_sh_runs_apples_container_without_the_keys_it_lacks() {
    let (dir, box_args) = initialized();
    let (code, err, log) = script(dir.path(), START, &["container"], &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(log.last().unwrap(), &format!("container run --detach {box_args} --tmpfs /run/fleetbox {} {RUN}", options().image));
}

#[test]
fn the_scripts_take_the_runtime_and_the_name_from_the_environment() {
    let (dir, _) = initialized();
    let env = [("FLEETBOX_RUNTIME", "nerdctl"), ("FLEETBOX_NAME", "box-2")];
    let (code, err, log) = script(dir.path(), START, &["docker", "nerdctl"], &env);
    assert_eq!(code, 0, "{err}");
    assert!(log.iter().all(|l| l.starts_with("nerdctl ")) && log.last().unwrap().contains("--name box-2 "), "{log:?}");
    let (code, err, log) = script(dir.path(), STOP, &["docker", "nerdctl"], &env);
    assert_eq!(code, 0, "{err}");
    assert_eq!(log, ["nerdctl stop -t 40 box-2", "nerdctl rm box-2"]);
}

#[test]
fn stop_sh_stops_the_box_within_its_grace_period_and_removes_it() {
    let (dir, _) = initialized();
    let (code, err, log) = script(dir.path(), STOP, &["podman"], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("podman"), "{err}");
    assert_eq!(log, ["podman stop -t 40 fleetbox", "podman rm fleetbox"]);
}

#[test]
fn the_scripts_say_when_there_is_no_runtime() {
    let (dir, _) = initialized();
    for name in [START, STOP] {
        let (code, err, log) = script(dir.path(), name, &[], &[]);
        assert_eq!(code, 1, "{name}");
        assert!(err.contains("FLEETBOX_RUNTIME") && err.contains("podman"), "{err}");
        assert!(log.is_empty());
    }
}
