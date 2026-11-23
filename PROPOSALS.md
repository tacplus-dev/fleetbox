# Proposals: fleetbox

Loopers propose here; Jody decides, and an accepted proposal becomes a ROADMAP line.

- **Read the uni-config through the operator's generated `UniConfig` once tacplus-dev/operator#17 lands.** The
  supervisor (#1) reads the uni-config's YAML or JSON form as a document and takes only `services` and `secrets`
  (src/config.rs), following the schema on `looper/operator-17` (269332b), which is not on the operator's main yet. A
  typed reader generated from `proto/services/uniconfig.proto` would refuse a malformed document the way
  `schematic-operator validate` does, read the binary proto form too, and follow the schema when it changes. It needs
  a tacplus Rust client of the operator's proto here (a `tacplus.yaml` depending on `operator`), so it waits on
  operator#17 and on whether fleetbox should become a tacplus project at all.
- **The services' typed config path.** A service learns its rendered config's path through `{config}` in its
  manifest args or env (docs/USER.md, Config), because the chassis still reads `BOOTSTRAP_CONFIG`/`SERVICE_CONFIG`
  from the environment. Once tacplus-dev/tacplus#262 gives the chassis a fixed config path, the fleet build's manifest
  can drop those variables, and the supervisor could render to that path directly.
- **Per-process bootstrap config.** The operator renders more than `services/<name>.yaml` for a pod: the chassis
  bootstrap config (kernel addresses, the kernel token, the PKI). A box needs the same, rendered by the supervisor
  from `operator` and `secrets.pki`. That is part of the image and its end-to-end check (#2, #3), which will show
  exactly what each service needs inside one container.
- **A `build.lock`.** The gates run in the manager's job (looper#71) until the hub's pin issue gives this repository a
  `build.lock` with a Rust build image.
- **The fleet build writes the box lock.** `fleetbox image` reads a box lock (docs/USER.md, The box lock: the
  fleetbox image and each service's release image by digest, and the files the box takes from each), because
  `fleet.lock` names commits, not images. With `box: true` in a `fleet.yaml`, `tacplus fleet build` would write it
  beside the manifest from the images it built or resolved, each service's copies from what its language's image
  holds (a jar at `/app/<name>.jar`, a binary at `/usr/local/bin/<name>`, the gateway's `/opt/tacplus/`): a change
  in tacplus-dev/tacplus, on the decision's "part of the fleet build, when enabled".
- **The fleetbox image as a release.** A box is built on the fleetbox image by digest. The build pipeline's publish
  job builds a repository's `Dockerfile` on a release, so fleetbox's first release publishes it
  (`us-docker.pkg.dev/jody1-328023/tacplus/fleetbox:X.Y.Z`), for amd64 and arm64; until then only the image
  workflow's smoke builds it, and pushes it nowhere.
- **Writable data for the services.** The box has one writable directory, `/run/fleetbox`, the supervisor's. A
  service that keeps state on disk (the kernel's `/app/data`, stratum's) needs its own, owned by the box's user,
  and a volume to outlive the container. The end-to-end check (#3) will show which; a manifest field (`data:`), or
  the supervisor making `<data-dir>/<service>` and passing it as `{data}`, would give it.
- **`fleetbox init` keeps what was set in the uni-config.** Re-running `init` regenerates `uni-config.yaml` from its
  flags (#4, as the issue asks), so a service's config or a secret added to it by hand is lost. Once boxes carry real
  service config, `init` could keep an existing file's `services` and `secrets` and rewrite only `operator.fqdn` and
  `operator.port`, or read them from a file of their own beside it.
- **`init` without `--image`.** A container cannot name its own image, so `init` takes the box's image as a flag and
  the README's command names it twice. `fleetbox image` could write the box's reference into the box (an `ENV` or a
  file under `/etc/fleetbox`) once the fleet build tags the box it builds, and `init` default to it.
- **`init`, `compose.yaml` and `start.sh` run for real in CI.** No gate has a container runtime, so the files are
  checked as YAML and the scripts against stand-in runtimes. `image/byhand.sh` runs them for real (init, `start.sh`,
  `stop.sh`, Compose `up` and `down`) on Docker and on rootless Podman, with the smoke's box; the image workflow
  (#2's `.github/workflows/image.yml`, which origin takes from the looper only on an issue labelled
  `looper:workflows`) would run it after the smoke: `just byhand docker`, then `docker save fleetbox-box:smoke |
  podman load`, `pip install podman-compose` and `just byhand podman`. Until it does, the tmpfs options (`uid=`,
  `gid=`, `mode=`), `userns_mode` from `$FLEETBOX_USERNS` and the box running as the directory's owner are unchecked
  on any runtime.
- **Rootless Docker and rootless nerdctl.** There the container's root is you, and the user `--user $(id -u)` names is
  one of your subordinate ids, so your directory is root's inside: `init` refuses it, and a box as your id could not
  read its uni-config. Neither has keep-id. `init` could take `--rootless`, writing a box that runs as root inside,
  which is you outside.
- **Apple's `container`, checked.** `start.sh` runs it with `--user`, a plain `--tmpfs` and none of the hardening keys
  it lacks, untested: whether its shared directories show your files as yours, and whether a plain tmpfs is writable
  by the box's user, need a Mac. A run on one, and `byhand.sh` taking `container`, would settle it.
