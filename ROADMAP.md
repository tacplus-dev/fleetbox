# Roadmap: fleetbox

Owned by Jody; changed only by an agent at Jody's request. Loopers propose in PROPOSALS.md.
Design: tacplus-dev/roadmap `decisions/fleetbox-2026-11-22.md`.

## Now
- The supervisor: a Rust PID 1 that reads the fleet build's manifest, starts the kernel then each service in order, waits on readiness, forwards signals, restarts with backoff (`--fail-fast` for tests) (#1). Hub, at Jody's request, 2026-11-22.
- The image: the services' release binaries by digest, a Java runtime, the manifest; arm64; only the gateway's port exposed (#2). Hub, at Jody's request, 2026-11-22.
- The image's default action writes a config directory (uni-config, manifest, admin account file, compose.yaml, start.sh) and exits; compose or the script runs the box (#4). Hub, at Jody's request, 2026-11-23.
- `fleetbox init --keycloak`: a Keycloak realm matching the config and a Keycloak container on its own port, so a box needs no external sign-in provider (#5). Hub, at Jody's request, 2026-11-23.
- An end-to-end check: init --keycloak, compose up, the admin signs in through Keycloak, a call through the gateway reaches a service, compose down (#3). Hub, at Jody's request, 2026-11-22.

- Minimal resources: the box's footprint measured, published and guarded in CI; the Java services sized from it (#6). Hub, at Jody's request, 2026-11-23.

## Later
- The test fixture: one box per test run, per-test tenant isolation, the service under test as an external member, generated per server language by tacplus.
- amd64.
- Native host members (hardware or desktop apps) joining a box as external members.

## Not doing
- A single-process fleet or an embedded/fake kernel.
- Being on the seatrials critical path: seatrials deploys to the cluster.
