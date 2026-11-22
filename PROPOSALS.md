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
