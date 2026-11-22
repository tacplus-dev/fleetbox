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

clean:
  rm -rf target
