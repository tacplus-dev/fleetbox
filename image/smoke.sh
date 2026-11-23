#!/usr/bin/env bash
# Copyright (c) 2026 Jody Scott
# SPDX-License-Identifier: Apache-2.0
#
# The image's check (#2): builds the fleetbox image from this checkout, builds a box on it from the test lock
# (tests/image/box.lock, tests/image/manifest.yaml) with `fleetbox image`, and runs the box: it must become healthy
# with no capability, as the box's user, answer on the gateway's port alone, run its Java service, and stop cleanly
# on SIGTERM. It needs Docker and a registry it can push to, for the fleetbox image's digest: the image workflow
# runs one beside it (.github/workflows/image.yml), or
#
#   docker run -d -p 5000:5000 registry:3     # then
#   image/smoke.sh                            # REGISTRY=localhost:5000, PLATFORM=linux/arm64
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
registry=${REGISTRY:-localhost:5000}
platform=${PLATFORM:-linux/arm64}
box=fleetbox-box:smoke
name=fleetbox-smoke-$$
work=$(mktemp -d)
trap 'docker rm -f "$name" > /dev/null 2>&1 || true; rm -rf "$work"' EXIT

say() { echo "== $*"; }
fail() {
  echo "smoke: $*" >&2
  docker logs "$name" 2>&1 | tail -50 >&2 || true
  exit 1
}

say "the fleetbox image, $platform"
docker build --platform "$platform" --build-arg "VCS_REF=$(git -C "$here" rev-parse HEAD 2> /dev/null || echo unknown)" \
  -t "$registry/fleetbox:smoke" "$here"
docker push "$registry/fleetbox:smoke"
fleetbox=$(docker image inspect --format '{{range .RepoDigests}}{{println .}}{{end}}' "$registry/fleetbox:smoke" |
  grep -m1 "^$registry/fleetbox@sha256:") || fail "no digest for $registry/fleetbox:smoke after its push"
say "fleetbox: $fleetbox"

say "the test box's build context, from its lock"
sed "s|^fleetbox: .*|fleetbox: $fleetbox|" "$here/tests/image/box.lock" > "$work/box.lock"
cp "$here/tests/image/manifest.yaml" "$work/manifest.yaml"
docker run --rm --user "$(id -u):$(id -g)" -v "$work:/work" "$fleetbox" \
  image --lock /work/box.lock --manifest /work/manifest.yaml --out /work/box
cat "$work/box/Dockerfile"

say "the test box"
docker build --platform "$platform" -t "$box" "$work/box"
[ "$(docker image inspect --format '{{.Architecture}}' "$box")" = "${platform#linux/}" ] || fail "$box is not $platform"
[ "$(docker image inspect --format '{{.Config.User}}' "$box")" = 65532:65532 ] || fail "$box does not run as 65532:65532"
[ "$(docker image inspect --format '{{range $p, $_ := .Config.ExposedPorts}}{{$p}} {{end}}' "$box")" = "8443/tcp " ] ||
  fail "$box exposes more or less than the gateway's 8443"
docker image inspect --format '{{json .Config.Healthcheck}}' "$box" | grep -q '"/usr/local/bin/fleetbox","health"' ||
  fail "$box has no HEALTHCHECK running fleetbox health"

say "runs, with no capability and no new privilege"
docker run -d --name "$name" --cap-drop ALL --security-opt no-new-privileges -p 127.0.0.1::8443 "$box" run > /dev/null
for _ in $(seq 90); do
  status=$(docker inspect --format '{{.State.Status}} {{.State.Health.Status}}' "$name")
  case "$status" in
    "running healthy") break ;;
    running*) sleep 2 ;;
    *) fail "the box is $status" ;;
  esac
done
[ "$status" = "running healthy" ] || fail "the box is not healthy after 180s: $status"
docker exec "$name" /usr/local/bin/fleetbox health
[ "$(docker exec "$name" /opt/kernel/busybox id -u)" = 65532 ] || fail "the services do not run as 65532"
docker exec "$name" /usr/bin/java -version

say "the gateway answers on its published port"
port=$(docker port "$name" 8443/tcp | head -1)
curl -fsS --max-time 10 "http://$port/" | grep -q '^root:' || fail "the gateway at $port did not answer with its index"

say "stops on SIGTERM, every service stopped"
docker stop -t 60 "$name" > /dev/null
code=$(docker inspect --format '{{.State.ExitCode}}' "$name")
logs=$(docker logs "$name" 2>&1)
echo "$logs" | tail -20
[ "$code" = 0 ] || fail "the box exited $code, not 0"
for s in gateway eventbus kernel; do
  echo "$logs" | grep -q "^fleetbox | $s started" || fail "$s never started"
done
echo "$logs" | grep -q '^fleetbox | stopped' || fail "the supervisor did not say it stopped"
say "ok"
