#!/usr/bin/env bash
# Copyright (c) 2026 Jody Scott
# SPDX-License-Identifier: Apache-2.0
#
# Running a box by hand (#4), as the README does it, on one container runtime: the box's image, given flags alone,
# writes the config directory, every file its owner's; `start.sh` starts the box, which becomes healthy as that owner
# and answers on the gateway's port; `stop.sh` stops it; a re-run of init keeps `admin.yaml`; and Compose `up` and
# `down` do the same as the scripts. It takes the test box (image/smoke.sh builds it, fleetbox-box:smoke), already
# in the runtime's images, and the runtime: docker, or podman (rootless too). The image workflow runs it for both.
#
#   image/byhand.sh fleetbox-box:smoke docker
#   docker save fleetbox-box:smoke | podman load && image/byhand.sh fleetbox-box:smoke podman
set -euo pipefail
box=${1:?usage: image/byhand.sh BOX RUNTIME}
runtime=${2:?usage: image/byhand.sh BOX RUNTIME}
port=${PORT:-18443}
name=fleetbox-byhand-$$
work=$(mktemp -d)
dir=$work/fleetbox
export FLEETBOX_RUNTIME=$runtime FLEETBOX_NAME=$name
compose=("$runtime" compose -f "$dir/compose.yaml" -p "$name")
trap '"$runtime" rm -f "$name" > /dev/null 2>&1 || true; "${compose[@]}" down > /dev/null 2>&1 || true; rm -rf "$work"' EXIT

say() { echo "== $runtime: $*"; }
fail() {
  echo "byhand ($runtime): $*" >&2
  "$runtime" logs "$name" 2>&1 | tail -50 >&2 || true
  exit 1
}

# Rootless Podman maps the user to itself only with keep-id: the README's commands for it.
userns=()
if [ "$runtime" = podman ] && [ "$(podman info --format '{{.Host.Security.Rootless}}')" = true ]; then
  say "rootless"
  userns=(--userns=keep-id)
  export FLEETBOX_USERNS=keep-id
fi

# Waits until `$@` (the box's `fleetbox health`) exits 0.
healthy() {
  for _ in $(seq 90); do
    if "$@" > /dev/null 2>&1; then
      "$@"
      return 0
    fi
    sleep 2
  done
  fail "the box is not healthy after 180s"
}

answers() {
  curl -fsS --max-time 10 "http://127.0.0.1:$port/" | grep -q '^root:' || fail "the gateway on $port did not answer"
}

say "init, the image's default action"
mkdir "$dir"
"$runtime" run --rm "${userns[@]}" --user "$(id -u):$(id -g)" -v "$dir:/config" "$box" --image "$box" --port "$port"
me="$(id -u):$(id -g)"
for f in admin.yaml:600 uni-config.yaml:600 manifest.yaml:644 compose.yaml:644 start.sh:755 stop.sh:755 .gitignore:644; do
  [ "$(stat -c '%u:%g %a' "$dir/${f%:*}")" = "$me ${f#*:}" ] || fail "${f%:*} is $(stat -c '%u:%g %a' "$dir/${f%:*}"), not $me ${f#*:}"
done
password=$(sed -n 's/^password: "\(.*\)"$/\1/p' "$dir/admin.yaml")
[ ${#password} = 32 ] || fail "admin.yaml has no password of 32"

say "start.sh"
"$dir/start.sh"
healthy "$runtime" exec "$name" /usr/local/bin/fleetbox health
[ "$("$runtime" exec "$name" /opt/kernel/busybox id -u)" = "$(id -u)" ] || fail "the box does not run as the directory's owner"
answers
"$dir/stop.sh"
! "$runtime" inspect "$name" > /dev/null 2>&1 || fail "stop.sh left $name"

say "a re-run keeps admin.yaml"
cp "$dir/admin.yaml" "$work/admin.yaml"
"$runtime" run --rm "${userns[@]}" --user "$(id -u):$(id -g)" -v "$dir:/config" "$box" --image "$box" --port "$port"
cmp -s "$dir/admin.yaml" "$work/admin.yaml" || fail "a re-run changed admin.yaml"

say "compose up"
"${compose[@]}" up -d
healthy "${compose[@]}" exec -T box /usr/local/bin/fleetbox health
answers
say "compose down"
"${compose[@]}" down
say "ok"
