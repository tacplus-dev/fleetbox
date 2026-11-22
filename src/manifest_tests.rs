// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

use super::*;

const FLEET: &str = r#"
version: 1
kernel: kernel
services:
  - name: gateway
    binary: /opt/gateway/gateway
    ports: [8443]
    depends_on: [oauth, eventbus]
    readiness: {tcp: {port: 8443}}
  - name: oauth
    binary: /opt/oauth/oauth
    args: ["--listen", ":9001"]
    env: {RUST_LOG: info}
    ports: [9001]
    readiness: {http: {port: 9001, path: /healthz}, timeout_seconds: 30, interval_ms: 100}
    config: {secrets: oauth}
  - name: eventbus
    binary: /opt/java/bin/java
    args: ["-jar", "/opt/eventbus/eventbus.jar"]
    readiness: {exec: {command: [/opt/eventbus/ready]}}
  - name: kernel
    binary: /opt/java/bin/java
    args: ["-jar", "/opt/kernel/kernel.jar"]
    ports: [5701]
    readiness: {tcp: {port: 5701}}
    stop_timeout_seconds: 30
"#;

#[test]
fn parses_a_fleet() {
    let m = Manifest::parse(FLEET).unwrap();
    assert_eq!(m.kernel, "kernel");
    let oauth = m.service("oauth").unwrap();
    assert_eq!(oauth.args, vec!["--listen", ":9001"]);
    assert_eq!(oauth.env.get("RUST_LOG").map(String::as_str), Some("info"));
    let r = oauth.readiness.as_ref().unwrap();
    assert_eq!(r.http.as_ref().unwrap().path, "/healthz");
    assert_eq!((r.timeout_seconds, r.interval_ms), (30, 100));
    let c = oauth.config.as_ref().unwrap();
    assert_eq!((c.service.as_deref(), c.secrets.as_deref()), (None, Some("oauth")));
    assert_eq!(m.service("kernel").unwrap().stop_timeout_seconds, 30);
    assert_eq!(m.service("gateway").unwrap().stop_timeout_seconds, 10);
    assert_eq!(m.restart.unhealthy_after, 5);
}

#[test]
fn starts_the_kernel_first_then_each_service_after_its_dependencies() {
    let m = Manifest::parse(FLEET).unwrap();
    assert_eq!(m.start_order().unwrap(), vec!["kernel", "oauth", "eventbus", "gateway"]);
    assert_eq!(m.dependencies(m.service("eventbus").unwrap()), vec!["kernel"]);
    assert!(m.dependencies(m.service("kernel").unwrap()).is_empty());
}

#[test]
fn backoff_doubles_up_to_its_maximum() {
    let p = RestartPolicy { initial_backoff_ms: 100, max_backoff_ms: 1000, unhealthy_after: 3, reset_after_seconds: 1 };
    let ms: Vec<u128> = (1..=6).map(|n| p.backoff(n).as_millis()).collect();
    assert_eq!(ms, vec![100, 200, 400, 800, 1000, 1000]);
    assert_eq!(p.backoff(1000).as_millis(), 1000);
}

fn refused(text: &str) -> String {
    Manifest::parse(text).unwrap_err().to_string()
}

#[test]
fn refuses_another_version() {
    assert!(refused("version: 2\nkernel: k\nservices: []\nnew_field: 1\n").contains("version 2"), "by its version");
    assert!(refused("kernel: k\nservices: []\n").contains("no version"));
}

#[test]
fn refuses_an_unknown_field() {
    let e = refused("version: 1\nkernel: k\nservices:\n  - {name: k, binary: /k, replicas: 2}\n");
    assert!(e.contains("replicas"), "{e}");
}

#[test]
fn refuses_what_cannot_run() {
    let cases = [
        ("services:\n  - {name: k, binary: /k}\n  - {name: k, binary: /k}\n", "listed twice"),
        ("services:\n  - {name: k, binary: k}\n", "absolute path"),
        ("services:\n  - {name: a, binary: /a}\n", "kernel \"k\" is not a service"),
        ("services:\n  - {name: k, binary: /k, depends_on: [a]}\n  - {name: a, binary: /a}\n", "depends on nothing"),
        ("services:\n  - {name: k, binary: /k}\n  - {name: a, binary: /a, depends_on: [b]}\n", "depends on b"),
        (
            "services:\n  - {name: k, binary: /k}\n  - {name: a, binary: /a, depends_on: [b]}\n  - {name: b, binary: /b, depends_on: [a]}\n",
            "dependency cycle among a, b",
        ),
        ("services:\n  - {name: k, binary: /k, ports: [1]}\n  - {name: a, binary: /a, ports: [1]}\n", "port 1 is also service k's"),
        ("services:\n  - {name: k, binary: /k, readiness: {}}\n", "readiness has 0 checks"),
        (
            "services:\n  - {name: k, binary: /k, readiness: {tcp: {port: 1}, exec: {command: [/x]}}}\n",
            "readiness has 2 checks",
        ),
        ("services:\n  - {name: \"a b\", binary: /k}\n", "a name is"),
        ("services:\n  - {name: k, binary: /k, args: [\"--config={config}\"]}\n", "uses {config} and has no config"),
        (
            "services:\n  - {name: k, binary: /k, env: {S: \"{secrets}\"}, config: {}}\n",
            "uses {secrets} and has no config.secrets",
        ),
        ("restart: {unhealthy_after: 0}\nservices:\n  - {name: k, binary: /k}\n", "unhealthy_after"),
    ];
    for (services, want) in cases {
        let e = refused(&format!("version: 1\nkernel: k\n{services}"));
        assert!(e.contains(want), "{services}: {e}");
    }
}
