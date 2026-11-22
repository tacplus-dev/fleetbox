# State: fleetbox

Last updated: 2026-11-22 by the looper (#2).

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

**Gaps:** the uni-config is read as a document, after operator#17's schema on its branch, which has not landed
(PROPOSALS.md). There is no per-process chassis bootstrap config yet. Nothing runs the image's smoke yet: its workflow (`.github/workflows/image.yml`, on
arm64) needs the looper's workflows permission on #2, so neither image has been built yet. No fleet build writes a box
lock, and the fleetbox image is not published (PROPOSALS.md). There is no end-to-end check (#3).

**Blockers:** none.
