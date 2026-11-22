// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::os::unix::fs::PermissionsExt;

// The shape `schematic-operator uni-config assemble` writes (tacplus-dev/operator#17, docs/uni-config.md).
const UNI: &str = r#"
operator:
  fqdn: box.example
services:
  oauth: |
    issuer: https://box.example
    providers: []
  eventbus: "partitions: 3\n"
opa_policies: {}
secrets:
  pki:
    root_ca: ROOT
  oauth:
    apple_signin_key_p8: KEY
"#;

const MANIFEST: &str = r#"
version: 1
kernel: kernel
services:
  - name: kernel
    binary: /k
  - name: oauth
    binary: /o
    args: ["--config", "{config}"]
    env: {OAUTH_SECRETS: "{secrets}"}
    config: {secrets: oauth}
  - name: bus
    binary: /b
    config: {service: eventbus}
  - name: gateway
    binary: /g
    config: {}
"#;

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn renders_each_services_config_and_secrets_owner_only() {
    let m = Manifest::parse(MANIFEST).unwrap();
    let uni = UniConfig::parse(UNI).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config");
    let r = render_all(&m, Some(&uni), &cfg).unwrap();
    assert_eq!(mode(&cfg), 0o700);

    let oauth = &r["oauth"];
    let c = oauth.config.as_ref().unwrap();
    assert_eq!(std::fs::read_to_string(c).unwrap(), "issuer: https://box.example\nproviders: []\n");
    assert_eq!(mode(c), 0o600);
    let s = oauth.secrets.as_ref().unwrap();
    let secrets: Value = serde_saphyr::from_str(&std::fs::read_to_string(s).unwrap()).unwrap();
    assert_eq!(secrets["apple_signin_key_p8"].as_str(), Some("KEY"));
    assert_eq!(mode(s), 0o600);
    assert_eq!(oauth.substitute("--config={config}"), format!("--config={}", c.display()));
    assert_eq!(oauth.substitute("{secrets}"), s.display().to_string());

    // Keyed by config.service when it names one; a service with no entry runs on its defaults.
    assert_eq!(std::fs::read_to_string(r["bus"].config.as_ref().unwrap()).unwrap(), "partitions: 3\n");
    assert_eq!(std::fs::read_to_string(r["gateway"].config.as_ref().unwrap()).unwrap(), "{}\n");
    assert_eq!(r["kernel"], Rendered::default());
}

#[test]
fn reads_the_json_form_too() {
    let uni = UniConfig::parse(r#"{"services": {"oauth": "a: 1\n"}, "secrets": {"oauth": {"k": "v"}}}"#).unwrap();
    assert_eq!(uni.services["oauth"], "a: 1\n");
}

#[test]
fn refuses_a_missing_secret_and_a_missing_uni_config() {
    let m = Manifest::parse(MANIFEST).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let uni = UniConfig::parse("services: {}\nsecrets: {}\n").unwrap();
    let e = render_all(&m, Some(&uni), dir.path()).unwrap_err().to_string();
    assert!(e.contains("secrets.oauth"), "{e}");
    let e = render_all(&m, None, dir.path()).unwrap_err().to_string();
    assert!(e.contains("oauth, bus, gateway render config") && e.contains("--uni-config"), "{e}");
}

#[test]
fn never_quotes_the_document_in_an_error() {
    let e = UniConfig::parse("secrets:\n  pki: {root_ca: SECRET-VALUE\n").err().unwrap().to_string();
    assert!(!e.contains("SECRET"), "{e}");
    let e = UniConfig::parse("services: {oauth: [SECRET-VALUE]}\n").err().unwrap().to_string();
    assert!(!e.contains("SECRET"), "{e}");
}

#[test]
fn a_manifest_without_config_needs_no_uni_config() {
    let m = Manifest::parse("version: 1\nkernel: k\nservices:\n  - {name: k, binary: /k}\n").unwrap();
    assert!(render_all(&m, None, Path::new("/nonexistent")).unwrap().is_empty());
}
