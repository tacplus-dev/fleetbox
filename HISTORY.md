# History: fleetbox

Append-only: add entries at the end, never edit or remove one (CI enforces it). One line each:
`- <YYYY-MM-DD> <who: looper|agent|jody> <kind: issue|decision|release|deploy> <one line> (<links>)`

- 2026-11-22 jody decision Created, from the fleet-in-a-box direction of 2026-11-08 (tacplus-dev/roadmap decisions/fleetbox-2026-11-22.md)
- 2026-11-22 looper issue The supervisor: a Rust PID 1 that runs a fleet's services from the fleet build's manifest (#1)
- 2026-11-24 looper issue Pinned images behind their lines: relock (Dockerfile: rust:1.100.0-slim-trixie → rust:1.100.0-slim-trixie) (#7)
- 2026-11-25 looper issue Pinned images behind their lines: relock (Dockerfile: rust:1.100.0-slim-trixie → rust:1.100.0-slim-trixie) (#8)
