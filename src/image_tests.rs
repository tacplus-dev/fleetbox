// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

use super::*;

const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

const MANIFEST: &str = r#"
version: 1
kernel: kernel
services:
  - name: kernel
    binary: /usr/bin/java
    args: ["-jar", "/opt/kernel/kernel.jar"]
    ports: [5701]
    readiness: {tcp: {port: 5701}, timeout_seconds: 180}
  - name: eventbus
    binary: /usr/bin/java
    args: ["-jar", "/opt/eventbus/eventbus.jar"]
    readiness: {tcp: {port: 9000}, timeout_seconds: 60}
  - name: oauth
    binary: /opt/oauth/oauth
    ports: [9001]
    depends_on: [eventbus]
  - name: gateway
    binary: /opt/gateway/bin/gateway
    ports: [8443, 8080]
    depends_on: [oauth]
    readiness: {http: {port: 8443}, timeout_seconds: 30}
"#;

fn lock() -> String {
    format!(
        r#"
version: 1
fleetbox: us-docker.pkg.dev/p/tacplus/fleetbox:0.2.0@{DIGEST}
gateway: gateway
services:
  - name: kernel
    image: us-docker.pkg.dev/p/tacplus/kernel:1.0.0@{DIGEST}
    copy:
      - {{from: /app/kernel.jar, to: /opt/kernel/kernel.jar}}
  - name: eventbus
    image: us-docker.pkg.dev/p/tacplus/eventbus@{DIGEST}
    copy:
      - {{from: /app/eventbus.jar, to: /opt/eventbus/eventbus.jar}}
  - name: oauth
    image: localhost:5000/oauth@{DIGEST}
    copy:
      - {{from: /usr/local/bin/oauth, to: /opt/oauth/oauth}}
  - name: gateway
    image: us-docker.pkg.dev/p/tacplus/gateway:2.1.0@{DIGEST}
    copy:
      - {{from: /usr/local/bin/gateway, to: /opt/gateway/bin/gateway}}
      - {{from: /opt/tacplus/webapp, to: /opt/tacplus/webapp}}
"#
    )
}

fn build(lock: &str, manifest: &str) -> Result<String, Error> {
    let m = Manifest::parse(manifest).unwrap();
    dockerfile(&Lock::parse(lock)?, &m)
}

fn refused(lock: &str, manifest: &str) -> String {
    match build(lock, manifest) {
        Ok(d) => panic!("built a Dockerfile:\n{d}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn starts_from_the_fleetbox_image_by_digest() {
    let d = build(&lock(), MANIFEST).unwrap();
    let froms: Vec<&str> = d.lines().filter(|l| l.starts_with("FROM ")).collect();
    assert_eq!(froms, vec![format!("FROM us-docker.pkg.dev/p/tacplus/fleetbox:0.2.0@{DIGEST}")]);
}

#[test]
fn copies_each_services_files_from_its_image_by_digest() {
    let d = build(&lock(), MANIFEST).unwrap();
    for want in [
        format!("COPY --from=us-docker.pkg.dev/p/tacplus/kernel:1.0.0@{DIGEST} /app/kernel.jar /opt/kernel/kernel.jar"),
        format!("COPY --from=localhost:5000/oauth@{DIGEST} /usr/local/bin/oauth /opt/oauth/oauth"),
        format!("COPY --from=us-docker.pkg.dev/p/tacplus/gateway:2.1.0@{DIGEST} /opt/tacplus/webapp /opt/tacplus/webapp"),
    ] {
        assert!(d.lines().any(|l| l == want), "no line {want:?} in\n{d}");
    }
    // Root owns what is copied: the services, which run as the box's user, cannot change their own binaries.
    assert!(!d.contains("--chown"), "{d}");
}

#[test]
fn copies_the_manifest_where_the_supervisor_reads_it() {
    let d = build(&lock(), MANIFEST).unwrap();
    assert!(d.lines().any(|l| l == "COPY manifest.yaml /etc/fleetbox/manifest.yaml"), "{d}");
}

#[test]
fn exposes_the_gateways_ports_alone() {
    let d = build(&lock(), MANIFEST).unwrap();
    let exposed: Vec<&str> = d.lines().filter(|l| l.starts_with("EXPOSE")).collect();
    assert_eq!(exposed, vec!["EXPOSE 8443 8080"]);
}

#[test]
fn runs_as_the_unprivileged_user_with_the_supervisor_as_entrypoint_and_its_health() {
    let d = build(&lock(), MANIFEST).unwrap();
    assert!(d.lines().any(|l| l == "USER 65532:65532"), "{d}");
    assert!(d.lines().any(|l| l == r#"ENTRYPOINT ["/usr/local/bin/fleetbox"]"#), "{d}");
    assert!(d.lines().any(|l| l == r#"CMD ["run"]"#), "{d}");
    let hc = d.lines().find(|l| l.starts_with("HEALTHCHECK")).unwrap();
    assert!(hc.ends_with(r#"CMD ["/usr/local/bin/fleetbox", "health"]"#), "{hc}");
}

#[test]
fn gives_the_box_its_longest_readiness_chain_to_start() {
    // kernel 180 + eventbus 60 + oauth 0 (no readiness) + gateway 30.
    assert_eq!(start_period(&Manifest::parse(MANIFEST).unwrap()), 270);
    let d = build(&lock(), MANIFEST).unwrap();
    assert!(d.contains("--start-period=270s"), "{d}");
}

#[test]
fn refuses_another_version() {
    let e = refused(&lock().replace("version: 1", "version: 2"), MANIFEST);
    assert!(e.contains("version 2"), "{e}");
}

#[test]
fn refuses_an_unknown_field() {
    let e = refused(&lock().replace("gateway: gateway", "gateway: gateway\nplatform: arm64"), MANIFEST);
    assert!(e.contains("platform"), "{e}");
}

#[test]
fn refuses_an_image_that_is_not_by_digest() {
    let e = refused(&lock().replace(&format!("kernel:1.0.0@{DIGEST}"), "kernel:1.0.0"), MANIFEST);
    assert!(e.contains("kernel") && e.contains("digest"), "{e}");
    let e = refused(&lock().replace(&format!("fleetbox:0.2.0@{DIGEST}"), "fleetbox:latest"), MANIFEST);
    assert!(e.contains("fleetbox") && e.contains("digest"), "{e}");
}

#[test]
fn refuses_a_service_the_manifest_does_not_have_or_lacks() {
    let e = refused(&lock().replace("name: oauth", "name: mailbox"), MANIFEST);
    assert!(e.contains("mailbox"), "{e}");
    let e = refused(&lock(), &format!("{MANIFEST}  - name: stratum\n    binary: /usr/bin/java\n"));
    assert!(e.contains("stratum"), "{e}");
}

#[test]
fn refuses_a_binary_no_copy_puts_in_the_box() {
    let e = refused(&lock(), &MANIFEST.replace("/opt/oauth/oauth", "/opt/oauth/bin/oauth"));
    assert!(e.contains("oauth") && e.contains("/opt/oauth/bin/oauth"), "{e}");
}

#[test]
fn refuses_a_gateway_with_no_port_or_not_in_the_manifest() {
    let e = refused(&lock().replace("gateway: gateway", "gateway: oauth2"), MANIFEST);
    assert!(e.contains("oauth2"), "{e}");
    let e = refused(&lock().replace("gateway: gateway", "gateway: eventbus"), MANIFEST);
    assert!(e.contains("eventbus") && e.contains("port"), "{e}");
}

#[test]
fn refuses_copies_onto_each_other_or_onto_the_box() {
    let twice = lock().replace("to: /opt/oauth/oauth", "to: /opt/kernel/kernel.jar");
    assert!(refused(&twice, MANIFEST).contains("/opt/kernel/kernel.jar"));
    let nested = lock().replace("to: /opt/tacplus/webapp", "to: /opt/gateway/bin/gateway/web");
    assert!(refused(&nested, MANIFEST).contains("/opt/gateway/bin/gateway"));
    for path in ["/usr/local/bin/fleetbox", "/etc/fleetbox", "/run/fleetbox/x", "/", "/usr/bin/java"] {
        let e = refused(&lock().replace("to: /opt/tacplus/webapp", &format!("to: {path}")), MANIFEST);
        assert!(e.contains(path), "{path}: {e}");
    }
}

#[test]
fn refuses_a_path_or_image_a_dockerfile_would_misread() {
    for bad in ["/opt/a b", "/opt/../etc/passwd", "opt/relative", "/opt/a\nRUN x", "/opt/$HOME", "/opt/[ab]"] {
        let l = lock().replace("from: /app/kernel.jar", &format!("from: {bad:?}"));
        let e = refused(&l, MANIFEST);
        assert!(e.contains("kernel"), "{bad:?}: {e}");
    }
    let e = refused(&lock().replace("localhost:5000/oauth@", "localhost:5000/oauth --from=x@"), MANIFEST);
    assert!(e.contains("oauth"), "{e}");
}

#[test]
fn refuses_a_service_with_nothing_to_copy() {
    let l = lock().replace("    copy:\n      - {from: /usr/local/bin/oauth, to: /opt/oauth/oauth}\n", "    copy: []\n");
    let e = refused(&l, &MANIFEST.replace("/opt/oauth/oauth", "/usr/bin/java"));
    assert!(e.contains("oauth") && e.contains("copies nothing"), "{e}");
}
