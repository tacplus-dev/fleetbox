// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

//! `fleetbox image`: the Dockerfile of a fleet's box, from its box lock and its manifest (docs/USER.md, The image).
//!
//! Box lock version 1. It names the fleetbox image (the supervisor, the Java runtime and the box's user) and, per
//! service, its release image and the files the box takes from it, every image by digest: nothing is rebuilt. The
//! lock is checked against the manifest before anything is written, so a box whose services could not start is
//! never built: every service of one is in the other, each binary is one the box has, and only the gateway's ports
//! are exposed.

use crate::manifest::{Error, Manifest};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// The one box lock version this supervisor reads.
pub const VERSION: u32 = 1;

/// The box's user and group: distroless's `nonroot`, as the services' own images run (src: Dockerfile).
pub const USER: &str = "65532:65532";
/// The Java runtime of the fleetbox image, which the Java services' manifest entries name as their binary.
pub const JAVA: &str = "/usr/bin/java";
/// Where the supervisor is, and where it reads the manifest and writes its run directory (docs/USER.md).
pub const SUPERVISOR: &str = "/usr/local/bin/fleetbox";
pub const MANIFEST: &str = "/etc/fleetbox/manifest.yaml";
pub const RUN_DIR: &str = "/run/fleetbox";

/// What no copy may put in the box, or replace: the fleetbox image's own.
const RESERVED: [&str; 4] = [SUPERVISOR, "/etc/fleetbox", RUN_DIR, JAVA];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    pub version: u32,
    /// The fleetbox image the box is built on, by digest.
    pub fleetbox: String,
    /// The service whose ports the box exposes.
    pub gateway: String,
    pub services: Vec<LockedService>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedService {
    pub name: String,
    /// Its release image, by digest.
    pub image: String,
    /// What the box takes from its image: its binary or jar, and what else it reads from disk.
    pub copy: Vec<Copy>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Copy {
    /// A file or directory of the service's image.
    pub from: String,
    /// Where it goes in the box.
    pub to: String,
}

fn err<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error(msg.into()))
}

impl Lock {
    pub fn load(path: &Path) -> Result<Lock, Error> {
        let text = std::fs::read_to_string(path).map_err(|e| Error(format!("box lock {}: {e}", path.display())))?;
        Lock::parse(&text).map_err(|e| Error(format!("box lock {}: {e}", path.display())))
    }

    /// Parses and validates a box lock on its own; [`dockerfile`] checks it against the manifest. The version is
    /// read first, as the manifest's is.
    pub fn parse(text: &str) -> Result<Lock, Error> {
        #[derive(Deserialize)]
        struct Versioned {
            version: Option<u32>,
        }
        let v: Versioned = serde_saphyr::from_str(text).map_err(|e| Error(e.to_string()))?;
        match v.version {
            None => return err(format!("no version: this supervisor reads box lock version {VERSION}")),
            Some(VERSION) => {}
            Some(n) => return err(format!("version {n}: this supervisor reads box lock version {VERSION}")),
        }
        let l: Lock = serde_saphyr::from_str(text).map_err(|e| Error(e.to_string()))?;
        l.validate()?;
        Ok(l)
    }

    fn validate(&self) -> Result<(), Error> {
        if !by_digest(&self.fleetbox) {
            return err(format!("fleetbox: image {:?} is not <name>@sha256:<digest>", self.fleetbox));
        }
        let mut names = HashSet::new();
        let mut targets: Vec<(&str, &str)> = RESERVED.iter().map(|r| (*r, "the fleetbox image")).collect();
        for s in &self.services {
            if !names.insert(s.name.as_str()) {
                return err(format!("service {}: listed twice", s.name));
            }
            if !by_digest(&s.image) {
                return err(format!("service {}: image {:?} is not <name>@sha256:<digest>", s.name, s.image));
            }
            if s.copy.is_empty() {
                return err(format!("service {}: copies nothing from its image", s.name));
            }
            for c in &s.copy {
                for p in [&c.from, &c.to] {
                    if !plain_path(p) {
                        return err(format!(
                            "service {}: {p:?} is not an absolute path of letters, digits and ._+@=,:- alone",
                            s.name
                        ));
                    }
                }
                if let Some((other, owner)) = targets.iter().find(|(t, _)| within(t, &c.to) || within(&c.to, t)) {
                    return err(format!("service {}: copies to {}, over {other} of {owner}", s.name, c.to));
                }
                targets.push((&c.to, &s.name));
            }
        }
        Ok(())
    }
}

/// `<name>[:tag]@sha256:<64 hex>`: an image that cannot move under the lock.
fn by_digest(image: &str) -> bool {
    let Some((name, digest)) = image.split_once("@sha256:") else { return false };
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "._/:-".contains(c))
        && digest.len() == 64
        && digest.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// An absolute path a Dockerfile's COPY reads as it is: no space, quote, glob, variable or relative component.
fn plain_path(p: &str) -> bool {
    p == "/"
        || p.strip_prefix('/').is_some_and(|rest| {
            rest.split('/').all(|c| {
                !c.is_empty()
                    && c != "."
                    && c != ".."
                    && c.chars().all(|ch| ch.is_ascii_alphanumeric() || "._+@=,:-".contains(ch))
            })
        })
}

/// Whether `inner` is `outer` or under it.
fn within(outer: &str, inner: &str) -> bool {
    outer == "/" || inner == outer || inner.strip_prefix(outer).is_some_and(|r| r.starts_with('/'))
}

/// How long the box may take to start before its HEALTHCHECK counts: the longest chain of dependencies, each
/// service in it given its readiness timeout (none for a service ready once started). Docker's start period ends
/// at the first healthy check, so a long one delays nothing.
pub fn start_period(m: &Manifest) -> u64 {
    let order = m.start_order().unwrap_or_default();
    let mut ready_by: HashMap<&str, u64> = HashMap::new();
    for name in &order {
        let Some(s) = m.service(name) else { continue };
        let after = m.dependencies(s).iter().filter_map(|d| ready_by.get(d.as_str())).max().copied().unwrap_or(0);
        ready_by.insert(name, after + s.readiness.as_ref().map_or(0, |r| r.timeout_seconds));
    }
    ready_by.values().max().copied().unwrap_or(0)
}

/// The box's Dockerfile. Its build context holds the manifest as `manifest.yaml` (`fleetbox image --out` writes
/// both).
pub fn dockerfile(lock: &Lock, m: &Manifest) -> Result<String, Error> {
    let locked: HashMap<&str, &LockedService> = lock.services.iter().map(|s| (s.name.as_str(), s)).collect();
    for s in &lock.services {
        if m.service(&s.name).is_none() {
            return err(format!("service {}: in the box lock and not in the manifest", s.name));
        }
    }
    for s in &m.services {
        let Some(l) = locked.get(s.name.as_str()) else {
            return err(format!("service {}: in the manifest and not in the box lock", s.name));
        };
        if s.binary != JAVA && !l.copy.iter().any(|c| within(&c.to, &s.binary)) {
            return err(format!(
                "service {}: binary {} is neither the Java runtime ({JAVA}) nor copied from its image",
                s.name, s.binary
            ));
        }
    }
    let gateway = m
        .service(&lock.gateway)
        .ok_or_else(|| Error(format!("gateway {:?} is not a service of the manifest", lock.gateway)))?;
    if gateway.ports.is_empty() {
        return err(format!("gateway {}: has no port in the manifest to expose", gateway.name));
    }

    let mut d = String::new();
    d.push_str("# The fleetbox image of a fleet: written by `fleetbox image` from its box lock and manifest. Regenerate it,\n");
    d.push_str("# do not edit it (docs/USER.md, The image).\n");
    d.push_str(&format!("FROM {}\n", lock.fleetbox));
    for name in m.start_order()? {
        let l = locked[name.as_str()];
        d.push_str(&format!("# {name}\n"));
        for c in &l.copy {
            d.push_str(&format!("COPY --from={} {} {}\n", l.image, c.from, c.to));
        }
    }
    d.push_str(&format!("COPY manifest.yaml {MANIFEST}\n"));
    let ports: Vec<String> = gateway.ports.iter().map(u16::to_string).collect();
    d.push_str(&format!("# Only the gateway's ports: the services talk to each other inside the box.\nEXPOSE {}\n", ports.join(" ")));
    d.push_str(&format!("USER {USER}\n"));
    d.push_str(&format!(
        "HEALTHCHECK --interval=10s --timeout=5s --start-period={}s --start-interval=2s --retries=3 \
         CMD [\"{SUPERVISOR}\", \"health\"]\n",
        start_period(m)
    ));
    // No command: the supervisor's default, init, is the box's default action; compose.yaml and start.sh run `run`.
    d.push_str(&format!("ENTRYPOINT [\"{SUPERVISOR}\"]\n"));
    Ok(d)
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
