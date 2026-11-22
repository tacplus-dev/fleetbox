# fleetbox: user guide

`fleetbox` is the supervisor of a fleetbox image: the container's PID 1. It runs every service of a fleet (the
kernel, gateway, eventbus, oauth and the rest) as separate processes, from a manifest the fleet build writes. Design:
tacplus-dev/roadmap `decisions/fleetbox-2026-11-22.md`.

## Running it

```sh
fleetbox [run] [--manifest PATH] [--uni-config PATH] [--run-dir DIR] [--fail-fast]
fleetbox check [--manifest PATH]
fleetbox health [--run-dir DIR]
```

| Flag | Default | What |
|---|---|---|
| `--manifest` | `/etc/fleetbox/manifest.yaml` | the fleet build's manifest (below) |
| `--uni-config` | none | the deployment's uni-config: each service's config is rendered from it ([Config](#config)) |
| `--run-dir` | `/run/fleetbox` | where the rendered config (`config/`) and the box's status (`status.json`) go |
| `--fail-fast` | off | exit on the first crash of any service instead of restarting it: for tests |

- `run` (the default) runs the box until it is stopped by a signal, or until it fails.
- `check` validates a manifest and prints its start order. The fleet build can run it on the manifest it writes.
- `health` prints the box's state and each service's, and exits 0 only when the box is healthy. It is the
  image's `HEALTHCHECK`.

Exit status:

| Status | When |
|---|---|
| 0 | stopped by SIGTERM or SIGINT, with every service stopped (`health`: the box is healthy) |
| 1 | the kernel died or was never ready, or with `--fail-fast` a service crashed or was never ready. Every service is stopped first. (`health`: the box is not healthy.) |
| 2 | a usage, manifest or uni-config error. Nothing was started. |

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

## Building

See the README: `just gates`.
