// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

//! Each service's config, rendered from the deployment's uni-config (tacplus-dev/operator#17): no operator and
//! no Kubernetes in the box, so the supervisor does for each process what the operator does for each pod.
//!
//! The uni-config is the operator's `operatorv1.UniConfig` in its YAML or JSON form. The supervisor reads two of
//! its fields: `services`, each service's config as YAML text keyed by service name, and `secrets`, which holds
//! each service's own `Secrets` message under the service's name. A service's entries are written to files only
//! it reads, in a directory only the supervisor's user reads, and the service is pointed at them through the
//! `{config}` and `{secrets}` placeholders of its args and environment. Nothing of the document is printed.

use crate::manifest::{Error, Manifest, Service};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub const CONFIG: &str = "{config}";
pub const SECRETS: &str = "{secrets}";

/// The parts of the uni-config the supervisor reads.
pub struct UniConfig {
    services: BTreeMap<String, String>,
    secrets: BTreeMap<String, Value>,
}

impl UniConfig {
    pub fn load(path: &Path) -> Result<UniConfig, Error> {
        let text = std::fs::read_to_string(path).map_err(|e| Error(format!("uni-config {}: {e}", path.display())))?;
        UniConfig::parse(&text).map_err(|e| Error(format!("uni-config {}: {e}", path.display())))
    }

    pub fn parse(text: &str) -> Result<UniConfig, Error> {
        // The error of a document that does not parse may quote it, and it carries secrets: say only where.
        let doc: Value = serde_saphyr::from_str(text).map_err(|e| match e.location() {
            Some(l) => Error(format!("not YAML or JSON (line {}, column {})", l.line(), l.column())),
            None => Error("not YAML or JSON".into()),
        })?;
        let Value::Object(doc) = doc else { return Err(Error("not a mapping".into())) };
        let mut services = BTreeMap::new();
        if let Some(s) = doc.get("services") {
            let Value::Object(s) = s else { return Err(Error("services is not a mapping".into())) };
            for (k, v) in s {
                let Value::String(v) = v else {
                    return Err(Error("services maps each service's name to its config's YAML text".into()));
                };
                services.insert(k.clone(), v.clone());
            }
        }
        let mut secrets = BTreeMap::new();
        if let Some(s) = doc.get("secrets") {
            let Value::Object(s) = s else { return Err(Error("secrets is not a mapping".into())) };
            for (k, v) in s {
                secrets.insert(k.clone(), v.clone());
            }
        }
        Ok(UniConfig { services, secrets })
    }
}

/// The files rendered for one service.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub config: Option<PathBuf>,
    pub secrets: Option<PathBuf>,
}

impl Rendered {
    /// `s` with its placeholders replaced by the rendered files' paths.
    pub fn substitute(&self, s: &str) -> String {
        let mut out = s.to_string();
        if let Some(p) = &self.config {
            out = out.replace(CONFIG, &p.to_string_lossy());
        }
        if let Some(p) = &self.secrets {
            out = out.replace(SECRETS, &p.to_string_lossy());
        }
        out
    }
}

/// Renders every service's config into `dir` (created owner-only), before anything starts, so a missing secret
/// fails the box at once rather than when its service comes up. A service with no `config` renders nothing.
pub fn render_all(m: &Manifest, uni: Option<&UniConfig>, dir: &Path) -> Result<BTreeMap<String, Rendered>, Error> {
    let mut out = BTreeMap::new();
    if !m.services.iter().any(|s| s.config.is_some()) {
        return Ok(out);
    }
    let Some(uni) = uni else {
        let names: Vec<&str> = m.services.iter().filter(|s| s.config.is_some()).map(|s| s.name.as_str()).collect();
        return Err(Error(format!("{} render config from the uni-config, and no --uni-config was given", names.join(", "))));
    };
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| Error(format!("config dir {}: {e}", dir.display())))?;
    for s in &m.services {
        out.insert(s.name.clone(), render(s, uni, dir)?);
    }
    Ok(out)
}

fn render(s: &Service, uni: &UniConfig, dir: &Path) -> Result<Rendered, Error> {
    let Some(c) = &s.config else { return Ok(Rendered::default()) };
    let key = c.service.as_deref().unwrap_or(&s.name);
    // A deployment that sets nothing for a service has no entry: the service runs on its defaults.
    let text = uni.services.get(key).map(String::as_str).unwrap_or("{}\n");
    let config = dir.join(format!("{}.yaml", s.name));
    write_private(&config, text.as_bytes())?;
    let mut r = Rendered { config: Some(config), secrets: None };
    if let Some(k) = &c.secrets {
        let v = uni
            .secrets
            .get(k)
            .ok_or_else(|| Error(format!("service {}: the uni-config has no secrets.{k}", s.name)))?;
        let text = serde_saphyr::to_string(v).map_err(|e| Error(format!("service {}: secrets.{k}: {e}", s.name)))?;
        let path = dir.join(format!("{}-secrets.yaml", s.name));
        write_private(&path, text.as_bytes())?;
        r.secrets = Some(path);
    }
    Ok(r)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let _ = std::fs::remove_file(path);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| f.write_all(bytes))
        .map_err(|e| Error(format!("{}: {e}", path.display())))
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
