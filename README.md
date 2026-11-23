# Fleetbox

A whole tacplus fleet in one container: kernel, gateway, eventbus, oauth, stratum and the rest run as separate
processes under a small supervisor. Two purposes: testing with no external OAuth dependency, and a micro fleet
deployment that runs on minimal resources.

- **Real, not a mock:** the same release binaries a cluster runs, and a real kernel (one Hazelcast member).
- **It scales:** container → compose → cluster with the same binaries.
- **A test harness:** any service with fleet dependencies can be tested against a box, without a full deployment.
- **Simple deployments, by hand:** run the image with flags and it writes a config directory (the fleet's config, an
  admin account file, a `compose.yaml` and a `start.sh`) and exits; then Compose `up` or `./start.sh`. No pipeline,
  and no Docker assumed: Docker, Podman, nerdctl or Apple's `container` all run it.
- **Its own sign-in, optional:** `--keycloak` adds a Keycloak, on its own port, with a realm matching the
  config, so a box needs no external sign-in provider.
- **Part of the fleet build, when enabled:** a fleet with `box: true` in its `fleet.yaml` also gets a fleetbox image
  built from its lock.

Design: tacplus-dev/roadmap `decisions/fleetbox-2026-11-22.md`. Rust. Apache-2.0.

## The supervisor

`fleetbox`, the box's PID 1: it reads the manifest the fleet build writes, starts the kernel and then each service once
its dependencies are ready, forwards SIGTERM/SIGINT and stops the services in reverse order, restarts a crashed
service with backoff (`--fail-fast` for tests), prefixes each service's log lines with its name, and renders each
service's config from the deployment's uni-config. The manifest format and everything else:
[docs/USER.md](docs/USER.md).

## The image

A box is the fleetbox image ([Dockerfile](Dockerfile): the supervisor on the Java runtime the kernel runs on, as an
unprivileged user) with each service's files copied from its release image by digest and the fleet's manifest.
`fleetbox image` writes its `Dockerfile` from the fleet's box lock: [docs/USER.md](docs/USER.md#the-image).

## Running a box by hand

From nothing to a running box, on any container runtime (`<box>` is a fleet's box image). Run with flags alone, the
image writes the config directory and exits; Compose or the directory's `start.sh` then runs the box.

```sh
mkdir fleetbox
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD/fleetbox:/config" <box> --image <box>
docker compose -f fleetbox/compose.yaml up -d          # or ./fleetbox/start.sh
```

Rootless Podman, which must map you to yourself (`keep-id`):

```sh
mkdir fleetbox
podman run --rm --userns=keep-id --user "$(id -u):$(id -g)" -v "$PWD/fleetbox:/config" <box> --image <box>
FLEETBOX_USERNS=keep-id podman compose -f fleetbox/compose.yaml up -d    # or ./fleetbox/start.sh
```

nerdctl: `nerdctl run` and `nerdctl compose`, as Docker's. Apple's `container`: `container run`, as Docker's, then
`./fleetbox/start.sh`. `start.sh` uses `$FLEETBOX_RUNTIME` if set, else the first of `docker`, `podman`, `nerdctl`
and `container` it finds, and says which.

The directory, every file yours: the fleet's deployment config in the uni-config's shape (`uni-config.yaml`), the
box's manifest, `compose.yaml` (the Compose specification alone, so any Compose implementation runs it), `start.sh`
and `stop.sh` (the gateway on 8443; `--port`, `--domain`), and `admin.yaml`, the admin account with a random password,
readable by you alone. **`admin.yaml` never goes in a repository** (the directory's `.gitignore` leaves it out).
Re-running the image regenerates everything but `admin.yaml`. Compose `down`, or `./fleetbox/stop.sh`, stops the box.
The rest: [docs/USER.md](docs/USER.md#running-a-box-by-hand).

## Building

The gates, `just gates` ([justfile](justfile)): `cargo build --all-targets --locked` and `cargo test --locked`. The
tests run the supervisor against stand-in child processes (`/bin/sh` scripts), in process (`src/*_tests.rs`) and as
the binary a container runs (`tests/binary.rs`, which signals it and reads its output). fleetbox is no tacplus project:
it has no `tacplus.yaml` and generates nothing. The image's check, `just smoke` (image/smoke.sh), needs Docker, which no
gate has: the image workflow runs it, on arm64. Running a box by hand, `just byhand docker` and
`just byhand podman` (image/byhand.sh), needs a runtime too, and the box the smoke builds. The security checks' cargo-deny reads [deny.toml](deny.toml),
looper-scan's default policy.
