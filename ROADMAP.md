# Roadmap: fleetbox

Owned by Jody; changed only by an agent at Jody's request. Loopers propose in PROPOSALS.md.
Design: tacplus-dev/roadmap `decisions/fleetbox-2026-11-22.md`.

## Now
- The supervisor: a Rust PID 1 that reads the fleet build's manifest, starts the kernel then each service in order, waits on readiness, forwards signals, restarts with backoff (`--fail-fast` for tests) (#1). Hub, at Jody's request, 2026-11-22.
- The image: the services' release binaries by digest, a Java runtime, the manifest; arm64; only the gateway's port exposed (#2). Hub, at Jody's request, 2026-11-22.
- An end-to-end check: a box from a small fleet starts, a call through the gateway reaches a service, the box stops cleanly (#3). Hub, at Jody's request, 2026-11-22.

## Later
- The test fixture: one box per test run, per-test tenant isolation, the service under test as an external member, generated per server language by tacplus.
- amd64.
- Native host members (hardware or desktop apps) joining a box as external members.

## Not doing
- A single-process fleet or an embedded/fake kernel.
- Being on the seatrials critical path: seatrials deploys to the cluster.
