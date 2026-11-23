# The fleetbox image: the supervisor on a Java runtime, as the box's user. A fleet's box is built on it by digest
# (`fleetbox image`, docs/USER.md: The image), which adds each service's files from its release image and the
# manifest; on its own, `run` runs a box from a manifest mounted at /etc/fleetbox/manifest.yaml.
#
# Every base is pinned by its multi-arch index digest, the tag it was resolved from beside it: a fix in a base
# reaches the image only when its digest moves, in a commit of its own.

# rust:1.100.0-slim-trixie, resolved 2026-11-22.
FROM rust:1.100.0-slim-trixie@sha256:6f14b2b4b278ec0d50aa1d69541c304ce0f95e0f6268487aa9924af1c2e9aebe AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
RUN cargo build --release --locked --bin fleetbox \
 && mkdir -p /out/run/fleetbox /out/etc/fleetbox

# gcr.io/distroless/java25-debian13:latest, resolved 2026-11-17: the base the kernel's release image runs on
# (tacplus-dev/kernel's Dockerfile), so the box's Java services (kernel, eventbus, stratum) run on the same Java 25
# runtime they do in a cluster, /usr/bin/java. Debian 13's glibc and libgcc run the Rust and Go services too. It has
# no shell, no package manager and no setuid binary.
FROM gcr.io/distroless/java25-debian13@sha256:6558dff7db91bf89d34254014dec8ff1f0a3e13f48a575d5a51ce01d1597d671

ARG VCS_REF
ARG VERSION
LABEL org.opencontainers.image.authors="Jody Scott <jody@thescottsweb.com>" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.vendor="tacplus.dev" \
      org.opencontainers.image.title="fleetbox" \
      org.opencontainers.image.description="A whole tacplus fleet in one container: the supervisor and the Java runtime"

COPY --from=build /src/target/release/fleetbox /usr/local/bin/fleetbox
COPY --from=build /out/etc/fleetbox /etc/fleetbox
# The run directory: the rendered config and status.json, the box user's alone (docs/USER.md: Config, Health).
COPY --from=build --chown=65532:65532 --chmod=0700 /out/run/fleetbox /run/fleetbox

# distroless's nonroot, by number so a Kubernetes runAsNonRoot can check it. No service needs root or a capability.
USER 65532:65532
HEALTHCHECK --interval=10s --timeout=5s --start-period=300s --start-interval=2s --retries=3 \
  CMD ["/usr/local/bin/fleetbox", "health"]
# No command: the supervisor's default action, init, is the image's: given flags, it writes the config directory at
# /config and exits (docs/USER.md, Running a box by hand). `run` runs the box.
ENTRYPOINT ["/usr/local/bin/fleetbox"]
