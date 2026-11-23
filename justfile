# Copyright (c) 2026 Jody Scott
# SPDX-License-Identifier: Apache-2.0

# `just gates`: the checks every change to main passes (README: Building). fleetbox is no tacplus project: it
# generates nothing, so its gates are the build and the tests.

default: gates

# Nothing under a build output may be tracked.
check-tracked:
  test -z "$(git ls-files target)" || { echo "check-tracked: target/ is build output and must not be committed" >&2; exit 1; }

# Cargo.lock is committed and never moved by a build (--locked).
build:
  cargo build --all-targets --locked

# The unit tests (src/*_tests.rs) and the binary's (tests/binary.rs), against stand-in child processes.
test: build
  cargo test --locked

# The gates every change passes before it lands.
gates: check-tracked test

# The image's check (image/smoke.sh): builds the fleetbox image and a box on it from the test lock, and runs it. It
# needs Docker and a registry at $REGISTRY (localhost:5000), so it is the image workflow's (.github/workflows/image.yml),
# not a gate: the gates run with no container runtime.
smoke:
  image/smoke.sh

# Running a box by hand (image/byhand.sh): init, start.sh, stop.sh, Compose up and down, with the test box the smoke
# builds, on RUNTIME (docker, or podman, rootless too). Like the smoke, it needs a container runtime: no gate runs it.
byhand runtime="docker" box="fleetbox-box:smoke":
  image/byhand.sh {{box}} {{runtime}}

clean:
  rm -rf target
