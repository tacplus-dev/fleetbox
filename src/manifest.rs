// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

//! The manifest: what the fleet build writes and the supervisor runs (docs/USER.md, The manifest).
//!
//! Version 1. A manifest names its kernel and lists each service with its binary, args, environment, ports,
//! readiness check, dependencies and config. Every field is checked: an unknown one is refused, so a manifest
//! from a newer fleet build fails here instead of running half understood.

use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::time::Duration;

/// The one manifest version this supervisor reads.
pub const VERSION: u32 = 1;

const DEFAULT_READY_TIMEOUT_SECONDS: u64 = 120;
const DEFAULT_READY_INTERVAL_MS: u64 = 250;
const DEFAULT_STOP_TIMEOUT_SECONDS: u64 = 10;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    /// The name of the kernel's service: started first, every other service depends on it.
    pub kernel: String,
    pub services: Vec<Service>,
    #[serde(default)]
    pub restart: RestartPolicy,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub name: String,
    /// An absolute path: the box has no PATH lookup to rely on.
    pub binary: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub ports: Vec<u16>,
    #[serde(default)]
    pub readiness: Option<Readiness>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub config: Option<ServiceConfig>,
    #[serde(default = "default_stop_timeout")]
    pub stop_timeout_seconds: u64,
}

/// How the supervisor knows a service is ready: exactly one of `tcp`, `http` or `exec`. A service with no
/// readiness is ready once it has started.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Readiness {
    #[serde(default)]
    pub tcp: Option<TcpCheck>,
    #[serde(default)]
    pub http: Option<HttpCheck>,
    #[serde(default)]
    pub exec: Option<ExecCheck>,
    #[serde(default = "default_ready_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_ready_interval")]
    pub interval_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TcpCheck {
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpCheck {
    pub port: u16,
    #[serde(default = "default_http_path")]
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecCheck {
    /// The command and its arguments; exit status 0 is ready.
    pub command: Vec<String>,
}

/// Which parts of the uni-config the supervisor renders for a service (src/config.rs).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    /// The key of the service's entry in the uni-config's `services`; the service's name when absent.
    #[serde(default)]
    pub service: Option<String>,
    /// The key of the service's entry in the uni-config's `secrets`, when it has one.
    #[serde(default)]
    pub secrets: Option<String>,
}

/// The crash policy: a crashed service is restarted after `initial_backoff_ms`, doubled on each further crash up
/// to `max_backoff_ms`; after `unhealthy_after` crashes in a row the box reports unhealthy. A service that has
/// run `reset_after_seconds` since its last start has its count reset.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RestartPolicy {
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub unhealthy_after: u32,
    pub reset_after_seconds: u64,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        RestartPolicy { initial_backoff_ms: 1000, max_backoff_ms: 30_000, unhealthy_after: 5, reset_after_seconds: 60 }
    }
}

impl RestartPolicy {
    /// The delay before the restart that follows the `crashes`-th crash in a row (1 for the first).
    pub fn backoff(&self, crashes: u32) -> Duration {
        let shift = crashes.saturating_sub(1).min(32);
        let ms = self.initial_backoff_ms.saturating_mul(1u64 << shift).min(self.max_backoff_ms);
        Duration::from_millis(ms)
    }
}

fn default_ready_timeout() -> u64 {
    DEFAULT_READY_TIMEOUT_SECONDS
}
fn default_ready_interval() -> u64 {
    DEFAULT_READY_INTERVAL_MS
}
fn default_stop_timeout() -> u64 {
    DEFAULT_STOP_TIMEOUT_SECONDS
}
fn default_http_path() -> String {
    "/".to_string()
}

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn err<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error(msg.into()))
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Manifest, Error> {
        let text = std::fs::read_to_string(path).map_err(|e| Error(format!("manifest {}: {e}", path.display())))?;
        Manifest::parse(&text).map_err(|e| Error(format!("manifest {}: {e}", path.display())))
    }

    /// Parses and validates a manifest. The version is read first, so a newer manifest is refused by its version
    /// rather than by the first field this supervisor does not know.
    pub fn parse(text: &str) -> Result<Manifest, Error> {
        #[derive(Deserialize)]
        struct Versioned {
            version: Option<u32>,
        }
        let v: Versioned = serde_saphyr::from_str(text).map_err(|e| Error(e.to_string()))?;
        match v.version {
            None => return err("no version: this supervisor reads manifest version 1"),
            Some(VERSION) => {}
            Some(n) => return err(format!("version {n}: this supervisor reads manifest version {VERSION}")),
        }
        let m: Manifest = serde_saphyr::from_str(text).map_err(|e| Error(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    fn validate(&self) -> Result<(), Error> {
        let mut names = HashSet::new();
        let mut ports = HashMap::new();
        for s in &self.services {
            if s.name.is_empty() || !s.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return err(format!("service {:?}: a name is letters, digits, '-' and '_'", s.name));
            }
            if !names.insert(s.name.as_str()) {
                return err(format!("service {}: listed twice", s.name));
            }
            if !s.binary.starts_with('/') {
                return err(format!("service {}: binary {:?} is not an absolute path", s.name, s.binary));
            }
            for p in &s.ports {
                if let Some(other) = ports.insert(*p, s.name.as_str()) {
                    return err(format!("service {}: port {p} is also service {other}'s", s.name));
                }
            }
            let uses = |p: &str| s.args.iter().chain(s.env.values()).any(|a| a.contains(p));
            if uses(crate::config::CONFIG) && s.config.is_none() {
                return err(format!("service {}: uses {{config}} and has no config", s.name));
            }
            if uses(crate::config::SECRETS) && s.config.as_ref().and_then(|c| c.secrets.as_ref()).is_none() {
                return err(format!("service {}: uses {{secrets}} and has no config.secrets", s.name));
            }
            if let Some(r) = &s.readiness {
                let n = r.tcp.is_some() as u8 + r.http.is_some() as u8 + r.exec.is_some() as u8;
                if n != 1 {
                    return err(format!("service {}: readiness has {n} checks; it has exactly one of tcp, http, exec", s.name));
                }
                if let Some(e) = &r.exec {
                    if e.command.is_empty() {
                        return err(format!("service {}: readiness exec has no command", s.name));
                    }
                }
                if r.interval_ms == 0 {
                    return err(format!("service {}: readiness interval_ms is 0", s.name));
                }
            }
        }
        if !names.contains(self.kernel.as_str()) {
            return err(format!("kernel {:?} is not a service of the manifest", self.kernel));
        }
        for s in &self.services {
            for d in &s.depends_on {
                if !names.contains(d.as_str()) {
                    return err(format!("service {}: depends on {d}, which is not a service of the manifest", s.name));
                }
                if *d == s.name {
                    return err(format!("service {}: depends on itself", s.name));
                }
            }
            if s.name == self.kernel && !s.depends_on.is_empty() {
                return err(format!("kernel {}: the kernel starts first and depends on nothing", s.name));
            }
        }
        if self.restart.initial_backoff_ms == 0 || self.restart.max_backoff_ms < self.restart.initial_backoff_ms {
            return err("restart: initial_backoff_ms is above 0 and max_backoff_ms is at least it");
        }
        if self.restart.unhealthy_after == 0 {
            return err("restart: unhealthy_after is at least 1");
        }
        self.start_order().map(|_| ())
    }

    /// The services' dependencies, with the kernel added to every other service's.
    pub fn dependencies(&self, service: &Service) -> Vec<String> {
        let mut deps = service.depends_on.clone();
        if service.name != self.kernel && !deps.contains(&self.kernel) {
            deps.insert(0, self.kernel.clone());
        }
        deps
    }

    /// A start order: the kernel first, every service after its dependencies, otherwise the manifest's order.
    /// The supervisor stops in the reverse of it. A cycle is refused, naming its services.
    pub fn start_order(&self) -> Result<Vec<String>, Error> {
        let mut order: Vec<String> = Vec::new();
        let mut done: HashSet<&str> = HashSet::new();
        while order.len() < self.services.len() {
            let next = self.services.iter().find(|s| {
                !done.contains(s.name.as_str()) && self.dependencies(s).iter().all(|d| done.contains(d.as_str()))
            });
            match next {
                Some(s) => {
                    done.insert(s.name.as_str());
                    order.push(s.name.clone());
                }
                None => {
                    let left: Vec<&str> =
                        self.services.iter().map(|s| s.name.as_str()).filter(|n| !done.contains(n)).collect();
                    return err(format!("dependency cycle among {}", left.join(", ")));
                }
            }
        }
        Ok(order)
    }

    pub fn service(&self, name: &str) -> Option<&Service> {
        self.services.iter().find(|s| s.name == name)
    }
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
