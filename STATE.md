# State: fleetbox

Last updated: 2026-11-23 by the looper (#4).

**Version:** none released.

**What works:** the supervisor, `fleetbox` (#1; docs/USER.md). It reads manifest version 1, which it refuses on any
other version or an unknown field. It starts the kernel first and each service once its dependencies are ready (tcp,
http or exec readiness), forwards SIGTERM/SIGINT and stops services in reverse order with a per-service timeout and
SIGKILL, and reaps every child and orphan. It restarts a crashed service with exponential backoff and reports it
unhealthy after repeated crashes (`status.json`, `fleetbox health`). It exits 1 if the kernel dies, or with
`--fail-fast` on any crash. Each service's log lines are prefixed with its name on stdout/stderr. Each service's
config and secrets are rendered owner-only from the uni-config's YAML/JSON form. `just gates`: build and tests (unit
tests and the binary's, against stand-in `sh` children).

The image (#2; docs/USER.md, The image): the fleetbox image (`Dockerfile`: the supervisor on the distroless Java 25
runtime the kernel runs on, as 65532, its HEALTHCHECK `fleetbox health`), and `fleetbox image`, which writes a box's
`Dockerfile` from a box lock (version 1: the fleetbox image and each service's release image by digest, and the files
the box copies from each) checked against the manifest, exposing the gateway's ports alone. `image/smoke.sh` builds
and runs a box from the test lock (tests/image/); it needs Docker, so no gate runs it.

Running a box by hand (#4; docs/USER.md, Running a box by hand): `init` is the image's default action. Run with flags
alone, the box's image writes a config directory and exits: `uni-config.yaml` in the uni-config's shape with the
domain and the gateway's port, the manifest, `compose.yaml` (the Compose specification's keys alone), `start.sh` and
`stop.sh` (the same box on `$FLEETBOX_RUNTIME` or the first of docker, podman, nerdctl, container found), and
`admin.yaml`, the admin account with a random password, 0600. Every file is the directory's owner's; a re-run
regenerates all but `admin.yaml`. Compose `up` / `down` or the scripts start and stop the box, as that owner, only the
gateway's port published; rootless Podman with keep-id. The README shows the commands. `image/byhand.sh` runs it all
on Docker and rootless Podman.

**Gaps:** the uni-config is read as a document, after operator#17's schema on its branch, which has not landed
(PROPOSALS.md). There is no per-process chassis bootstrap config yet. Nothing runs the image's smoke yet: its workflow (`.github/workflows/image.yml`, on
arm64) needs the looper's workflows permission on #2, so neither image has been built yet. No fleet build writes a box
lock, and the fleetbox image is not published (PROPOSALS.md). There is no end-to-end check (#3), and nothing runs `image/byhand.sh` yet (it belongs in #2's image workflow): no gate
has a container runtime, so `compose.yaml` and the scripts have been checked, not run. Rootless Docker and nerdctl, and
Apple's `container`, are unchecked (PROPOSALS.md). Nothing
reads `admin.yaml` until `init --keycloak` (#5). A re-run of `init` replaces what was set in `uni-config.yaml` (PROPOSALS.md).

**Blockers:** none.
