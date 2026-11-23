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
}

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
    for f in [ADMIN, UNI_CONFIG, COMPOSE, MANIFEST, ".gitignore"] {
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
