# State: fleetbox

Last updated: 2026-11-22 by the looper (#1).

**Version:** none released.

**What works:** the supervisor, `fleetbox` (#1; docs/USER.md). It reads manifest version 1, which it refuses on any
other version or an unknown field. It starts the kernel first and each service once its dependencies are ready (tcp,
http or exec readiness), forwards SIGTERM/SIGINT and stops services in reverse order with a per-service timeout and
SIGKILL, and reaps every child and orphan. It restarts a crashed service with exponential backoff and reports it
unhealthy after repeated crashes (`status.json`, `fleetbox health`). It exits 1 if the kernel dies, or with
`--fail-fast` on any crash. Each service's log lines are prefixed with its name on stdout/stderr. Each service's
config and secrets are rendered owner-only from the uni-config's YAML/JSON form. `just gates`: build and tests (unit
tests and the binary's, against stand-in `sh` children).

**Gaps:** the uni-config is read as a document, after operator#17's schema on its branch, which has not landed
(PROPOSALS.md). There is no per-process chassis bootstrap config yet. There is no image (#2) and no end-to-end check
(#3).

**Blockers:** none.
