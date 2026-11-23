// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

//! `fleetbox init`: the config directory a box runs from by hand (docs/USER.md, Running a box by hand).
//!
//! The image's default action. Run from the box's own image, so the host needs only a container runtime (Docker,
//! Podman, nerdctl, Apple's `container`), it writes the fleet's deployment config in the uni-config's shape, the
//! manifest the image holds, an admin account, a `compose.yaml` that any Compose implementation runs, and a
//! `start.sh` and `stop.sh` that run the same box without Compose. Every file is its directory's owner's, as whoever
//! made the directory wrote it: a person, or a test that then reads the admin account to sign in. Re-running it
//! regenerates every file but the admin account, which is never overwritten.

use crate::manifest::{Error, Manifest};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The files of a config directory.
pub const UNI_CONFIG: &str = "uni-config.yaml";
pub const MANIFEST: &str = "manifest.yaml";
pub const ADMIN: &str = "admin.yaml";
pub const COMPOSE: &str = "compose.yaml";
pub const START: &str = "start.sh";
pub const STOP: &str = "stop.sh";
const GITIGNORE: &str = ".gitignore";

/// Where the box sees its config directory, and where `init` is pointed at it by default.
pub const MOUNT: &str = "/config";

/// The admin account's name and its password's length, in letters and digits (about 190 bits).
const ADMIN_USER: &str = "admin";
const PASSWORD_LEN: usize = 32;

#[derive(Debug, Clone)]
pub struct Options {
    /// The box's image: `compose.yaml` runs it.
    pub image: String,
    /// The fleet's domain, the uni-config's `operator.fqdn`.
    pub domain: String,
    /// The gateway's port on the host, the uni-config's `operator.port`.
    pub port: u16,
    /// The manifest's gateway service: its first port is the one published.
    pub gateway: String,
}

impl Default for Options {
    fn default() -> Self {
        Options { image: String::new(), domain: "localhost".into(), port: 8443, gateway: "gateway".into() }
    }
}

fn err<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error(msg.into()))
}

impl Options {
    /// Refuses what a generated file would misread. Every value is quoted where it is written, too.
    pub fn validate(&self) -> Result<(), Error> {
        let label = |l: &str| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        };
        if self.domain.len() > 253 || !self.domain.split('.').all(label) {
            return err(format!("--domain {:?} is not a hostname", self.domain));
        }
        if self.port == 0 {
            return err("--port is 1 to 65535");
        }
        if self.image.is_empty() || !self.image.chars().all(|c| c.is_ascii_alphanumeric() || "._/:@-".contains(c)) {
            return err(format!("--image {:?} is not an image reference", self.image));
        }
        Ok(())
    }
}

/// A YAML scalar for any string: JSON's quoting, which YAML reads.
fn quote(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

/// The box's deployment config in the uni-config's shape (tacplus-dev/operator#17, `operatorv1.UniConfig`): the
/// operator part a cluster's has, with the domain and the gateway's port, and no service config or secret yet.
pub fn uni_config(o: &Options) -> String {
    format!(
        "# The box's deployment config, in the uni-config's shape (tacplus-dev/operator#17): one format for a box and a\n\
         # cluster. Written by `fleetbox init`, which regenerates it: change its flags (--domain, --port) and re-run it.\n\
         operator:\n  fqdn: {}\n  port: {}\n\
         # Each service's config, as the YAML text of services/<name>.yaml, keyed by service name.\n\
         services: {{}}\n\
         # Each service's Secrets, keyed by service name.\n\
         secrets: {{}}\n",
        quote(&o.domain),
        o.port
    )
}

/// The admin account file.
pub fn admin(password: &str) -> String {
    format!(
        "# The box's admin account, written once by `fleetbox init`. Its owner's alone (0600): never put it in a repository.\n\
         username: {}\npassword: {}\n",
        quote(ADMIN_USER),
        quote(password)
    )
}

/// A random password of letters and digits, from the kernel's random source.
pub fn password() -> Result<String, Error> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut random = std::fs::File::open("/dev/urandom").map_err(|e| Error(format!("/dev/urandom: {e}")))?;
    let mut out = String::with_capacity(PASSWORD_LEN);
    let mut buf = [0u8; 64];
    while out.len() < PASSWORD_LEN {
        random.read_exact(&mut buf).map_err(|e| Error(format!("/dev/urandom: {e}")))?;
        // 248 is the largest multiple of 62 a byte holds: anything above it would favour the first letters.
        for b in buf.iter().filter(|b| (**b as usize) < 248).take(PASSWORD_LEN - out.len()) {
            out.push(ALPHABET[*b as usize % ALPHABET.len()] as char);
        }
    }
    Ok(out)
}

/// What `compose.yaml` and `start.sh` both run: the box as the config directory's owner (so it reads the files that
/// are that owner's alone), its run directory that user's alone, and the config's port on the gateway's.
struct BoxRun {
    uid: u32,
    gid: u32,
    /// `<host port>:<the gateway's port>`.
    ports: String,
    /// The run directory's tmpfs, with its owner and mode.
    tmpfs: String,
    /// Seconds to stop in: the supervisor stops the services one at a time, each within its stop timeout, and a
    /// runtime's default 10s would kill the box half stopped.
    grace: u64,
}

impl BoxRun {
    fn new(m: &Manifest, o: &Options, (uid, gid): (u32, u32)) -> Result<BoxRun, Error> {
        let gateway = m
            .service(&o.gateway)
            .ok_or_else(|| Error(format!("gateway {:?} is not a service of the manifest (--gateway)", o.gateway)))?;
        let Some(port) = gateway.ports.first() else {
            return err(format!("gateway {}: has no port in the manifest to publish", gateway.name));
        };
        Ok(BoxRun {
            uid,
            gid,
            ports: format!("{}:{port}", o.port),
            tmpfs: format!("{}:uid={uid},gid={gid},mode=0700", crate::image::RUN_DIR),
            grace: m.services.iter().map(|s| s.stop_timeout_seconds).sum::<u64>() + 10,
        })
    }
}

/// `compose.yaml`: the box, the config directory mounted read-only, in the Compose specification's keys alone, so
/// that `docker compose`, `podman compose` (or `podman-compose`) and `nerdctl compose` all run it.
pub fn compose(m: &Manifest, o: &Options, owner: (u32, u32)) -> Result<String, Error> {
    let b = BoxRun::new(m, o, owner)?;
    Ok(format!(
        "# The box (docs/USER.md, Running a box by hand): `docker compose up -d` in this directory starts it, and\n\
         # `docker compose down` stops it; `podman compose` and `nerdctl compose` too. Rootless Podman:\n\
         # `FLEETBOX_USERNS=keep-id podman compose up -d`. Written by `fleetbox init`, which regenerates it: do not\n\
         # edit it.\n\
         services:\n\
         \x20 box:\n\
         \x20   image: {image}\n\
         \x20   # The config directory's owner, who alone reads the uni-config.\n\
         \x20   user: {user}\n\
         \x20   # Rootless Podman shows the owner's files as root's unless the owner is mapped to itself (keep-id).\n\
         \x20   userns_mode: \"${{FLEETBOX_USERNS:-}}\"\n\
         \x20   command: [run, --manifest, {mount}/{MANIFEST}, --uni-config, {mount}/{UNI_CONFIG}]\n\
         \x20   volumes:\n\
         \x20     - {volume}\n\
         \x20   # The supervisor's run directory, its user's alone.\n\
         \x20   tmpfs:\n\
         \x20     - {tmpfs}\n\
         \x20   # The gateway's port alone: the services talk to each other inside the box.\n\
         \x20   ports:\n\
         \x20     - {ports}\n\
         \x20   cap_drop: [ALL]\n\
         \x20   security_opt: [no-new-privileges:true]\n\
         \x20   stop_grace_period: {grace}s\n\
         \x20   restart: unless-stopped\n",
        image = quote(&o.image),
        user = quote(&format!("{}:{}", b.uid, b.gid)),
        mount = MOUNT,
        volume = quote(&format!("./:{MOUNT}:ro")),
        tmpfs = quote(&b.tmpfs),
        ports = quote(&b.ports),
        grace = b.grace,
    ))
}

/// What both scripts begin with: the runtime, `$FLEETBOX_RUNTIME` or the first found, and the container's name,
/// `$FLEETBOX_NAME` or `fleetbox`. Shell builtins alone, so it runs wherever `/bin/sh` does.
const SCRIPT_HEAD: &str = r#"set -eu
runtime=${FLEETBOX_RUNTIME:-}
if [ -z "$runtime" ]; then
  for r in docker podman nerdctl container; do
    if command -v "$r" > /dev/null 2>&1; then
      runtime=$r
      break
    fi
  done
fi
if [ -z "$runtime" ]; then
  echo "fleetbox: no container runtime: none of docker, podman, nerdctl or container is on the PATH (set FLEETBOX_RUNTIME)" >&2
  exit 1
fi
echo "fleetbox: $runtime" >&2
name=${FLEETBOX_NAME:-fleetbox}
"#;

/// A shell word for a string: single-quoted.
fn sh(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `start.sh`: the box `compose.yaml` runs, without Compose, on any runtime.
pub fn start(m: &Manifest, o: &Options, owner: (u32, u32)) -> Result<String, Error> {
    let b = BoxRun::new(m, o, owner)?;
    Ok(format!(
        r#"#!/bin/sh
# Starts the box without Compose (docs/USER.md, Running a box by hand): the box compose.yaml runs, on
# $FLEETBOX_RUNTIME if set, else the first of docker, podman, nerdctl and container found, as the container
# $FLEETBOX_NAME (fleetbox). ./stop.sh stops it. Written by `fleetbox init`, which regenerates it: do not edit it.
{SCRIPT_HEAD}case $0 in
  */*) dir=$(cd "${{0%/*}}" && pwd) ;;
  *) dir=$(pwd) ;;
esac
# As the config directory's owner, the directory read-only, the gateway's port alone published.
set -- --name "$name" --user {user} --volume "$dir:{MOUNT}:ro" --publish {ports}
case ${{runtime##*/}} in
  container)
    # Apple's container: each container a virtual machine of its own, with no capability, restart policy or stop
    # timeout to set.
    set -- "$@" --tmpfs {run_dir}
    ;;
  *)
    set -- "$@" --tmpfs {tmpfs} --cap-drop ALL --security-opt no-new-privileges --stop-timeout {grace} \
      --restart unless-stopped
    # Rootless Podman shows the owner's files as root's unless the owner is mapped to itself.
    if [ "${{runtime##*/}}" = podman ] && [ "$("$runtime" info --format '{{{{.Host.Security.Rootless}}}}')" = true ]; then
      set -- "$@" --userns keep-id
    fi
    ;;
esac
# A box stopped before, so that its name is free.
"$runtime" rm "$name" > /dev/null 2>&1 || true
exec "$runtime" run --detach "$@" {image} run --manifest {MOUNT}/{MANIFEST} --uni-config {MOUNT}/{UNI_CONFIG}
"#,
        user = sh(&format!("{}:{}", b.uid, b.gid)),
        ports = sh(&b.ports),
        run_dir = crate::image::RUN_DIR,
        tmpfs = sh(&b.tmpfs),
        grace = b.grace,
        image = sh(&o.image),
    ))
}

/// `stop.sh`: stops the box `start.sh` started, within its grace period, and removes it.
pub fn stop(m: &Manifest, o: &Options, owner: (u32, u32)) -> Result<String, Error> {
    let b = BoxRun::new(m, o, owner)?;
    Ok(format!(
        r#"#!/bin/sh
# Stops the box ./start.sh started (docs/USER.md, Running a box by hand), on the same runtime and name, every service
# within its stop timeout, and removes it. Written by `fleetbox init`, which regenerates it: do not edit it.
{SCRIPT_HEAD}"$runtime" stop -t {grace} "$name"
"$runtime" rm "$name"
"#,
        grace = b.grace,
    ))
}

/// Whom `init` gives its files to: `Some` owner when it runs as root, `None` when it runs as the directory's owner.
/// It refuses to write as anyone else, and a directory of root's, whose box would run as root.
pub fn chown_to(dir_owner: (u32, u32), euid: u32) -> Result<Option<(u32, u32)>, Error> {
    if dir_owner.0 == 0 {
        return err("the config directory belongs to root, and the box would run as root: \
                    make it first (mkdir) as the user the box runs as (under rootless Podman, add --userns=keep-id)");
    }
    if euid == 0 {
        return Ok(Some(dir_owner));
    }
    if euid == dir_owner.0 {
        return Ok(None);
    }
    err(format!(
        "the config directory belongs to {}:{} and init runs as {euid}: run it as its owner (docker run --user {}:{})",
        dir_owner.0, dir_owner.1, dir_owner.0, dir_owner.1
    ))
}

/// Writes `bytes` to `dir/name` with `mode`, given to `owner`: through a new file renamed over the old one, so a
/// symlink in its place is replaced, never written through, and a reader never sees half a file.
fn write(dir: &Path, name: &str, bytes: &[u8], mode: u32, owner: Option<(u32, u32)>) -> Result<(), Error> {
    let path = dir.join(name);
    let tmp = dir.join(format!(".{name}.init"));
    let fail = |e: std::io::Error| Error(format!("{}: {e}", path.display()));
    let _ = std::fs::remove_file(&tmp);
    let mut f = OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp).map_err(fail)?;
    let result = give(&f, owner).and_then(|_| f.write_all(bytes)).and_then(|_| f.sync_all());
    drop(f);
    if let Err(e) = result.and_then(|_| std::fs::rename(&tmp, &path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(fail(e));
    }
    Ok(())
}

/// Writes `bytes` to `dir/name` only when there is nothing there: `Ok(false)` when there is.
fn write_once(dir: &Path, name: &str, bytes: &[u8], mode: u32, owner: Option<(u32, u32)>) -> Result<bool, Error> {
    let path = dir.join(name);
    let fail = |e: std::io::Error| Error(format!("{}: {e}", path.display()));
    let mut f = match OpenOptions::new().write(true).create_new(true).mode(mode).open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(fail(e)),
    };
    give(&f, owner).and_then(|_| f.write_all(bytes)).and_then(|_| f.sync_all()).map_err(fail)?;
    Ok(true)
}

fn give(f: &std::fs::File, owner: Option<(u32, u32)>) -> std::io::Result<()> {
    match owner {
        Some((uid, gid)) => std::os::unix::fs::fchown(f, Some(uid), Some(gid)),
        None => Ok(()),
    }
}

/// Writes the config directory `dir`, which must exist, from the manifest at `manifest`. Everything is checked
/// before anything is written. Returns what it did, a line per file.
pub fn init(dir: &Path, manifest: &Path, o: &Options) -> Result<Vec<String>, Error> {
    o.validate()?;
    let m = Manifest::load(manifest)?;
    // The manifest as it was read and checked, byte for byte.
    let manifest_bytes = std::fs::read(manifest).map_err(|e| Error(format!("manifest {}: {e}", manifest.display())))?;
    let meta = std::fs::metadata(dir).map_err(|e| {
        Error(format!("config directory {}: {e}: mount it there (-v \"$PWD/fleetbox:{MOUNT}\")", dir.display()))
    })?;
    if !meta.is_dir() {
        return err(format!("config directory {}: not a directory", dir.display()));
    }
    let owner = (meta.uid(), meta.gid());
    // SAFETY: geteuid(2) takes nothing and cannot fail.
    let chown = chown_to(owner, unsafe { libc::geteuid() })?;
    let compose = compose(&m, o, owner)?;
    let (start, stop) = (start(&m, o, owner)?, stop(&m, o, owner)?);
    let in_dir = |name: &str| -> PathBuf { dir.join(name) };

    let mut done = Vec::new();
    let password = password()?;
    if write_once(dir, ADMIN, admin(&password).as_bytes(), 0o600, chown)? {
        done.push(format!("{}: the admin account, its password new", in_dir(ADMIN).display()));
    } else {
        done.push(format!("{}: kept, as it was", in_dir(ADMIN).display()));
    }
    write(dir, UNI_CONFIG, uni_config(o).as_bytes(), 0o600, chown)?;
    write(dir, MANIFEST, &manifest_bytes, 0o644, chown)?;
    write(dir, COMPOSE, compose.as_bytes(), 0o644, chown)?;
    write(dir, START, start.as_bytes(), 0o755, chown)?;
    write(dir, STOP, stop.as_bytes(), 0o755, chown)?;
    write(dir, GITIGNORE, format!("# fleetbox init: the admin account never goes in a repository.\n{ADMIN}\n").as_bytes(), 0o644, chown)?;
    for name in [UNI_CONFIG, MANIFEST, COMPOSE, START, STOP, GITIGNORE] {
        done.push(format!("{}: written", in_dir(name).display()));
    }
    Ok(done)
}

#[cfg(test)]
#[path = "init_tests.rs"]
mod tests;
