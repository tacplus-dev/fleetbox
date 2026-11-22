# Fleetbox

A whole tacplus fleet in one container: kernel, gateway, eventbus, oauth, stratum and the rest run as separate
processes under a small supervisor.

- **Real, not a mock:** the same release binaries a cluster runs, and a real kernel (one Hazelcast member).
- **It scales:** container → compose → cluster with the same binaries.
- **A test harness:** any service with fleet dependencies can be tested against a box, without a full deployment.
- **Part of the fleet build, when enabled:** a fleet with `box: true` in its `fleet.yaml` also gets a fleetbox image
  built from its lock.

Design: tacplus-dev/roadmap `decisions/fleetbox-2026-11-22.md`. Rust. Apache-2.0.
