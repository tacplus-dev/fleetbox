# fleetbox: user guide

`fleetbox` is the supervisor of a fleetbox image: the container's PID 1. It runs every service of a fleet (the
kernel, gateway, eventbus, oauth and the rest) as separate processes, from a manifest the fleet build writes. Design:
tacplus-dev/roadmap `decisions/fleetbox-2026-11-22.md`.

## Running it

```sh
fleetbox [init] --image IMAGE [--config-dir DIR] [--manifest PATH] [--domain NAME] [--port N] [--gateway NAME]
fleetbox run [--manifest PATH] [--uni-config PATH] [--run-dir DIR] [--fail-fast]
fleetbox check [--manifest PATH]
fleetbox health [--run-dir DIR]
fleetbox image --lock PATH [--manifest PATH] --out DIR
```

| Flag | Default | What |
|---|---|---|
| `--manifest` | `/etc/fleetbox/manifest.yaml` | the fleet build's manifest (below) |
| `--uni-config` | none | the deployment's uni-config: each service's config is rendered from it ([Config](#config)) |
| `--run-dir` | `/run/fleetbox` | where the rendered config (`config/`) and the box's status (`status.json`) go |
| `--fail-fast` | off | exit on the first crash of any service instead of restarting it: for tests |
| `--lock` | none | `image`: the fleet's box lock ([The image](#the-image)) |
| `--out` | none | `image`: the directory the box's build context is written to |
| `--image` | none | `init`: the box's image, which `compose.yaml` and `start.sh` run ([Running a box by hand](#running-a-box-by-hand)) |
| `--config-dir` | `/config` | `init`: the config directory it writes, which must exist |
| `--domain` | `localhost` | `init`: the fleet's domain, the uni-config's `operator.fqdn` |
| `--port` | `8443` | `init`: the gateway's port on the host, the uni-config's `operator.port` |
| `--gateway` | `gateway` | `init`: the manifest's gateway service, whose first port is the one published |

- `init` (the default, and so the image's default action) writes the config directory a box runs from by hand, and
  exits ([Running a box by hand](#running-a-box-by-hand)).
- `run` runs the box until it is stopped by a signal, or until it fails.
- `check` validates a manifest and prints its start order. The fleet build can run it on the manifest it writes.
- `health` prints the box's state and each service's, and exits 0 only when the box is healthy. It is the
  image's `HEALTHCHECK`.
- `image` writes a box's build context from its box lock and manifest ([The image](#the-image)).

Exit status:

| Status | When |
|---|---|
| 0 | stopped by SIGTERM or SIGINT, with every service stopped (`health`: the box is healthy) |
| 1 | the kernel died or was never ready, or with `--fail-fast` a service crashed or was never ready. Every service is stopped first. (`health`: the box is not healthy.) |
| 2 | a usage, manifest, uni-config, box lock or `init` error. Nothing was started (`image`, `init`: nothing was written). |

## The manifest

The fleet build writes the manifest, in YAML (or JSON, which YAML reads). It is versioned: this supervisor reads
version **1** and refuses any other version by its number. It also refuses any field it does not know, so a manifest
from a newer fleet build fails at once instead of running half understood. A change that an older supervisor would
misread is a new version.

```yaml
version: 1
kernel: kernel                    # the kernel's service: it starts first
restart:                          # the crash policy; each field has the default shown
  initial_backoff_ms: 1000
  max_backoff_ms: 30000
  unhealthy_after: 5
  reset_after_seconds: 60
services:
  - name: kernel
    binary: /opt/java/bin/java
    args: ["-jar", "/opt/kernel/kernel.jar", "--config", "{config}"]
    env: {JAVA_OPTS: "-Xmx1g"}
    ports: [5701]
    readiness: {tcp: {port: 5701}, timeout_seconds: 180}
    config: {}
    stop_timeout_seconds: 30
  - name: oauth
    binary: /opt/oauth/oauth
    args: ["--config", "{config}", "--secrets", "{secrets}"]
    ports: [9001]
    readiness: {http: {port: 9001, path: /healthz}}
    depends_on: [eventbus]
    config: {secrets: oauth}
  - name: eventbus
    binary: /opt/java/bin/java
    args: ["-jar", "/opt/eventbus/eventbus.jar"]
    readiness: {exec: {command: [/opt/eventbus/bin/ready]}}
```

### Top level

| Field | Required | What |
|---|---|---|
| `version` | yes | `1` |
| `kernel` | yes | the name of the kernel's service. It starts first, depends on nothing, and every other service depends on it. |
| `services` | yes | the services, below |
| `restart` | no | the crash policy ([Crashes](#crashes)) |

### A service

| Field | Required | Default | What |
|---|---|---|---|
| `name` | yes | | letters, digits, `-` and `_`; unique. It prefixes the service's log lines. |
| `binary` | yes | | an absolute path |
| `args` | no | none | its arguments. `{config}` and `{secrets}` become the paths of its rendered config ([Config](#config)). |
| `env` | no | none | variables added to the supervisor's environment, with the same placeholders |
| `ports` | no | none | the ports it listens on. Two services cannot share one in a container, so the manifest refuses that. |
| `readiness` | no | ready once started | how the supervisor knows it is ready, below |
| `depends_on` | no | none | the services that must be ready before it starts. The kernel is always one of them. |
| `config` | no | none | what of the uni-config is rendered for it ([Config](#config)) |
| `stop_timeout_seconds` | no | 10 | how long it has to exit on SIGTERM or SIGINT before it is killed |

### Readiness

`readiness` has exactly one check, which is tried every `interval_ms` until it passes or `timeout_seconds` pass:

| Check | Ready when |
|---|---|
| `tcp: {port: N}` | 127.0.0.1:N accepts a connection |
| `http: {port: N, path: /p}` | `GET /p` on 127.0.0.1:N answers 2xx (`path` defaults to `/`) |
| `exec: {command: [/bin/x, arg]}` | the command exits 0 (each attempt is killed after 5s) |

| Field | Default |
|---|---|
| `timeout_seconds` | 120 |
| `interval_ms` | 250 |

A service that is not ready in time is killed, and that counts as a crash. For the kernel, it ends the box.

## Order

The supervisor starts the kernel and waits for it to be ready. It then starts each service as soon as every service it
depends on is ready, so services with nothing between them start together. A dependency cycle, or a dependency
that is not in the manifest, is refused before anything starts. `fleetbox check` prints the start order: the kernel
first, then each service after its dependencies, otherwise in the manifest's order.

A service whose dependency crashes later keeps running. A service that is restarted waits again until its
dependencies are ready.

## Signals

On SIGTERM or SIGINT, the supervisor stops every service in the reverse of the start order, one at a time, so the
kernel stops last. Each service is sent the signal the supervisor received. A service that has not exited within its
`stop_timeout_seconds` is sent SIGKILL. When all of them have stopped, the supervisor exits 0. A second signal while
it is stopping changes nothing.

Each service runs in a process group of its own, so the signal reaches everything the service started. When a
service exits, whatever it left in its group is killed with it. As PID 1, the supervisor reaps every process that
exits in the container, including orphans that a service left behind. When it is not PID 1 (under a test, say), it
makes itself the subreaper of its services, so it reaps their orphans all the same.

## Crashes

A service that exits on its own has crashed, whatever its exit status: services are not meant to exit. What happens
next:

- **The kernel:** the box fails. Every other service is stopped, and the supervisor exits 1.
- **With `--fail-fast`, any service:** the same.
- **Otherwise:** the service is restarted after a backoff of `initial_backoff_ms`, doubled for each further crash in
  a row, up to `max_backoff_ms`. At `unhealthy_after` crashes in a row, the service is reported unhealthy, on the
  log and in the box's health, and it keeps being restarted. A service that runs for `reset_after_seconds` after a
  start has its count reset, and it is healthy again.

## Logs

Each line a service writes is written as `<service> | <line>`. A line from its stdout goes to the supervisor's
stdout, and a line from its stderr goes to the supervisor's stderr. Lines are never interleaved: each is written
whole. The supervisor's own lines are prefixed `fleetbox | `, on stderr:

```text
fleetbox | starting kernel, eventbus, oauth, the kernel (kernel) first
fleetbox | kernel started (pid 7)
kernel | Hazelcast member started
fleetbox | kernel ready
fleetbox | oauth exited 1
fleetbox | oauth restarts in 1000ms (1 crashes in a row)
```

## Config

There is no operator and no Kubernetes in the box. The supervisor renders each service's config from the deployment's
uni-config ([tacplus-dev/operator#17](https://github.com/tacplus-dev/operator/issues/17), the operator's
`docs/uni-config.md`), as the operator does for each pod.

`--uni-config` is the operator's `operatorv1.UniConfig`, in its YAML or JSON form: what `schematic-operator uni-config
assemble` writes and the `uni-config` Secret holds. Its binary proto form is not read. The supervisor reads two of
its fields:

- `services`: each service's config, as the YAML text of `services/<name>.yaml`, keyed by service name;
- `secrets`: the deployment's secrets, with each service's own `Secrets` message under the service's name (`oauth`,
  `mailbox`).

For a service with `config` in the manifest:

| `config` field | Default | What |
|---|---|---|
| `service` | the service's name | its key in the uni-config's `services` |
| `secrets` | none | its key in the uni-config's `secrets`, when it has secrets |

- The service's entry in `services` is written to `<run-dir>/config/<name>.yaml`, and `{config}` in its args and env
  becomes that path. A deployment that sets nothing for the service has no entry, and the service gets an empty
  config (`{}`): its defaults.
- With `secrets`, its entry in `secrets` is written as YAML to `<run-dir>/config/<name>-secrets.yaml`, and `{secrets}`
  becomes that path. A missing entry is an error.

Every service is rendered before anything starts, so a uni-config that cannot run the box fails at once (exit 2). The
directory is readable by the supervisor's user only (0700), and so are the files (0600). Nothing of the uni-config is
printed: an error says where it is (line and column), never what it says. A manifest that uses `{config}` with no
`config`, or `{secrets}` with no `config.secrets`, is refused.

How each service reads the path (a flag, or the config path the chassis reads) is the fleet build's choice, made in
the manifest's args or env.

## Health

The supervisor writes the box's state to `<run-dir>/status.json` whenever it changes. `fleetbox health` reads it:

| State | Meaning |
|---|---|
| `starting` | not every service has been ready yet |
| `healthy` | every service is ready, and none is unhealthy |
| `degraded` | every service has been ready, and one is now restarting, below `unhealthy_after` |
| `unhealthy` | a service has crashed `unhealthy_after` times in a row |
| `stopping`, `stopped`, `failed` | the box is stopping on a signal, stopped, or failed ([Crashes](#crashes)) |

```text
$ fleetbox health
healthy
eventbus: ready (restarts 0)
kernel: ready (restarts 0)
oauth: ready (restarts 2)
```

## The image

A fleet's box is one image, built in two layers of images, every one by digest and nothing rebuilt:

- **The fleetbox image** ([Dockerfile](../Dockerfile)): the supervisor at `/usr/local/bin/fleetbox`, on the Java 25
  runtime the kernel's release image runs on (distroless `java25-debian13`: `/usr/bin/java`, Debian 13's glibc, no
  shell and no setuid binary), its run directory `/run/fleetbox` the box user's alone. It is the same for every fleet.
- **The box**, built on it: each service's files copied from the service's release image, and the fleet's manifest at
  `/etc/fleetbox/manifest.yaml`. `fleetbox image` writes its `Dockerfile` from the fleet's **box lock** and checks
  the two against each other first.

```sh
fleetbox image --lock box.lock --manifest manifest.yaml --out box/
docker build --platform linux/arm64 -t <fleet>-box box/
docker run -p 8443:8443 <fleet>-box run [--uni-config PATH] [--fail-fast]
```

The box runs as `65532:65532` (distroless's `nonroot`), needs no capability (it runs with `--cap-drop ALL` and
`no-new-privileges`), exposes the gateway's ports alone (the services talk to each other inside it), and its
`HEALTHCHECK` is `fleetbox health` ([Health](#health)), with a start period of the manifest's longest chain of
readiness timeouts. Its entrypoint is the supervisor, with no command: given flags alone it runs `init`
([Running a box by hand](#running-a-box-by-hand)), and `run` runs the box (`--uni-config`, `--fail-fast`). arm64 first (the cluster and Jody's Macs are arm64); amd64 is on the roadmap.

### The box lock

The fleet build writes it beside the manifest. Like the manifest it is versioned, version **1**, and refuses any other
version and any field it does not know.

```yaml
version: 1
fleetbox: us-docker.pkg.dev/jody1-328023/tacplus/fleetbox:0.2.0@sha256:…   # the fleetbox image
gateway: gateway                                                          # the service whose ports are exposed
services:
  - name: kernel
    image: us-docker.pkg.dev/jody1-328023/tacplus/kernel:1.4.0@sha256:…
    copy:
      - {from: /app/kernel.jar, to: /opt/kernel/kernel.jar}
  - name: gateway
    image: us-docker.pkg.dev/jody1-328023/tacplus/gateway:2.1.0@sha256:…
    copy:
      - {from: /usr/local/bin/gateway, to: /opt/gateway/gateway}
      - {from: /opt/tacplus/webapp, to: /opt/tacplus/webapp}
```

| Field | What |
|---|---|
| `fleetbox` | the fleetbox image the box is built on |
| `gateway` | the manifest's gateway service: its `ports` are the box's `EXPOSE`, and it has at least one |
| `services[].name` | a service of the manifest. Every service of the manifest is in the lock, and only those. |
| `services[].image` | its release image |
| `services[].copy` | what the box takes from that image, at least one: a file or a directory (`from`), and where it goes (`to`) |

Every image is `<name>[:tag]@sha256:<digest>`: a tag alone could move under the lock. Every path is absolute, of
letters, digits and `._+@=,:-` alone, with no `.` or `..` component, so the `Dockerfile` reads it as it is. No two
copies land on or under one another, nor on the fleetbox image's own (`/usr/local/bin/fleetbox`, `/etc/fleetbox`,
`/run/fleetbox`, `/usr/bin/java`). Each service's `binary` in the manifest is the Java runtime, `/usr/bin/java` (a
Java service, its jar among its copies), or a path its own copies put in the box. Copied files keep their mode and
belong to root: a service cannot change its own binary.

### Checking an image

`image/smoke.sh` (`just smoke`) builds the fleetbox image from the checkout, a box on it from the test lock
([tests/image/](../tests/image)), and runs that box: it must be healthy with no capability, its services running as
`65532`, its gateway answering on its published port, its Java service on the image's runtime, and it must stop
with status 0 on SIGTERM. It needs Docker and a registry to push the fleetbox image to, for its digest
(`REGISTRY`, `localhost:5000`): the image workflow runs it on arm64 for every change.

## Running a box by hand

A box needs no pipeline, and no Docker in particular: any container runtime runs it. The box's image, run with flags
alone, runs `init`: it writes a config directory and exits. Compose (`up`, `down`) or the directory's `start.sh` and
`stop.sh` then start and stop the box from it; either one, not both.

```sh
mkdir fleetbox
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD/fleetbox:/config" <box> --image <box> [--domain NAME] [--port N]
docker compose -f fleetbox/compose.yaml up -d     # or ./fleetbox/start.sh
docker compose -f fleetbox/compose.yaml down      # or ./fleetbox/stop.sh
```

| Runtime | init | start | stop |
|---|---|---|---|
| Docker | `docker run --rm --user "$(id -u):$(id -g)" -v "$PWD/fleetbox:/config" <box> --image <box>` | `docker compose -f fleetbox/compose.yaml up -d` | `docker compose -f fleetbox/compose.yaml down` |
| Podman, rootless | `podman run --rm --userns=keep-id --user "$(id -u):$(id -g)" -v "$PWD/fleetbox:/config" <box> --image <box>` | `FLEETBOX_USERNS=keep-id podman compose -f fleetbox/compose.yaml up -d` | `FLEETBOX_USERNS=keep-id podman compose -f fleetbox/compose.yaml down` |
| Podman, as root | `podman run --rm --user "$(id -u):$(id -g)" -v "$PWD/fleetbox:/config" <box> --image <box>` | `podman compose -f fleetbox/compose.yaml up -d` | `podman compose -f fleetbox/compose.yaml down` |
| nerdctl (containerd) | `nerdctl run --rm --user "$(id -u):$(id -g)" -v "$PWD/fleetbox:/config" <box> --image <box>` | `nerdctl compose -f fleetbox/compose.yaml up -d` | `nerdctl compose -f fleetbox/compose.yaml down` |
| Apple's `container` | `container run --rm --user "$(id -u):$(id -g)" -v "$PWD/fleetbox:/config" <box> --image <box>` | `./fleetbox/start.sh` | `./fleetbox/stop.sh` |

`./fleetbox/start.sh` and `./fleetbox/stop.sh` work with every one of them (below). The image cannot name itself, so
`--image` names the box `compose.yaml` and `start.sh` run (PROPOSALS.md).

| File | Mode | What |
|---|---|---|
| `uni-config.yaml` | 0600 | the fleet's deployment config, in the uni-config's shape ([Config](#config)): one format for a box and a cluster. `operator.fqdn` is `--domain`, `operator.port` is `--port`; `services` and `secrets` are empty, so every service runs on its defaults. |
| `manifest.yaml` | 0644 | the box's manifest (`--manifest`, by default the image's own), byte for byte, checked first |
| `compose.yaml` | 0644 | the box, for any Compose implementation (below) |
| `start.sh`, `stop.sh` | 0755 | the same box without Compose, on any runtime (below) |
| `admin.yaml` | 0600 | the admin account: `username` (`admin`) and a random `password` of 32 letters and digits |
| `.gitignore` | 0644 | `admin.yaml` |

- **Every file is the config directory's owner's:** a person, or an automated test, which then reads `admin.yaml` to
  sign in. Run as that owner (`--user $(id -u):$(id -g)`, as above), `init` writes them as itself; run as root, it
  gives each to the directory's owner. It refuses to run as anyone else (the image's own user, without `--user`), and
  a directory of root's (the one `docker run -v` makes when there is none), whose box would run as root: make it first.
  Rootless Podman shows your directory as root's unless it maps you to yourself: `--userns=keep-id`, as above.
- **`admin.yaml` is written once** and never overwritten, nor ever regenerated: it never goes in a repository.
  Nothing in the box reads it yet; `init --keycloak` (#5) will make it the Keycloak realm's first user.
- **Re-running `init` regenerates every other file**, from its flags: what was edited in them is replaced.
- Everything is checked before anything is written: the manifest, the gateway service and its port, the flags.

`compose.yaml` runs one service, `box`, in the Compose specification's keys alone, so `docker compose`, `podman
compose` (or `podman-compose`) and `nerdctl compose` all run it: the box's image (`--image`) with the command `run`, as
the config directory's owner, so it reads the uni-config that is that owner's alone; the directory mounted read-only
at `/config` and the box run from its manifest and uni-config; the supervisor's run directory a tmpfs of its user's
alone; the config's port published on the gateway service's first port, and no other; no capability and no new
privilege; a stop grace period of every service's stop timeout plus 10 seconds, since the supervisor stops them one at
a time; and `restart: unless-stopped`. Its `userns_mode` is `$FLEETBOX_USERNS`, empty unless set: rootless Podman's
`keep-id`, which no other runtime has.

`start.sh` runs the same box without Compose, as the container `$FLEETBOX_NAME` (`fleetbox`), on `$FLEETBOX_RUNTIME`
if it is set, else on the first of `docker`, `podman`, `nerdctl` and `container` it finds, and says which. It removes
a stopped box of that name first. Under rootless Podman it adds `--userns keep-id` itself. Apple's `container` runs
each container in a virtual machine of its own, with no capability, restart policy or stop timeout to set, so there
it sets none. `stop.sh` stops the box on the same runtime, within the grace period, and removes it.

`image/byhand.sh` (`just byhand docker`, `just byhand podman`) runs all of it, as above, with the test box the
smoke builds: init and its files, `start.sh` to a healthy box running as the directory's owner and answering on the
gateway's port, `stop.sh`, a re-run keeping `admin.yaml`, and Compose `up` and `down`. Like the smoke it needs a
container runtime, which no gate has.

The box renders no per-process bootstrap config yet (the kernel's addresses, its token, the PKI): the uni-config has
no PKI to give it (PROPOSALS.md).

## Building

See the README: `just gates`.
