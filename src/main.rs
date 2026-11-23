// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

//! `fleetbox`: the box's PID 1 (docs/USER.md).

use fleetbox::manifest::Manifest;
use fleetbox::supervisor::{Options, Supervisor};
use fleetbox::{health, logs};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::path::PathBuf;
use std::process::exit;

const USAGE: &str = "usage:
  fleetbox [init] --image IMAGE [--config-dir DIR] [--manifest PATH] [--domain NAME] [--port N] [--gateway NAME]
  fleetbox run [--manifest PATH] [--uni-config PATH] [--run-dir DIR] [--fail-fast]
  fleetbox check [--manifest PATH]
  fleetbox health [--run-dir DIR]
  fleetbox image --lock PATH [--manifest PATH] --out DIR

  --manifest PATH     the fleet build's manifest (default /etc/fleetbox/manifest.yaml)
  --uni-config PATH   the deployment's uni-config, each service's config is rendered from
  --run-dir DIR       rendered config and status.json (default /run/fleetbox)
  --fail-fast         exit on the first crash of any service instead of restarting it (for tests)
  --lock PATH         image: the fleet's box lock, every image by digest
  --out DIR           image: where the box's Dockerfile and manifest.yaml (its build context) are written
  --image IMAGE       init: the box's image, which compose.yaml and start.sh run
  --config-dir DIR    init: the config directory it writes, which must exist (default /config)
  --domain NAME       init: the fleet's domain (default localhost)
  --port N            init: the gateway's port on the host (default 8443)
  --gateway NAME      init: the manifest's gateway service, whose first port is published (default gateway)

exit status: 0 stopped by SIGTERM or SIGINT (health: healthy); 1 the kernel died, a service crashed
with --fail-fast, or the box is not healthy; 2 a usage, manifest, uni-config, box lock or init error";

struct Args {
    command: String,
    manifest: PathBuf,
    uni_config: Option<PathBuf>,
    run_dir: PathBuf,
    fail_fast: bool,
    lock: Option<PathBuf>,
    out: Option<PathBuf>,
    config_dir: PathBuf,
    init: fleetbox::init::Options,
}

fn usage_error(msg: &str) -> ! {
    eprintln!("fleetbox: {msg}\n{USAGE}");
    exit(2)
}

fn parse_args() -> Args {
    let mut a = Args {
        // The image's default action: given flags alone, it writes the config directory and exits.
        command: "init".into(),
        manifest: "/etc/fleetbox/manifest.yaml".into(),
        uni_config: None,
        run_dir: "/run/fleetbox".into(),
        fail_fast: false,
        lock: None,
        out: None,
        config_dir: fleetbox::init::MOUNT.into(),
        init: Default::default(),
    };
    let mut it = std::env::args().skip(1).peekable();
    if let Some(c) = it.peek().filter(|c| !c.starts_with('-')) {
        a.command = c.clone();
        it.next();
    }
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        let mut value = || inline.clone().or_else(|| it.next()).unwrap_or_else(|| usage_error(&format!("{flag} needs a value")));
        match flag.as_str() {
            "--manifest" => a.manifest = value().into(),
            "--uni-config" => a.uni_config = Some(value().into()),
            "--run-dir" => a.run_dir = value().into(),
            "--fail-fast" if inline.is_none() => a.fail_fast = true,
            "--lock" => a.lock = Some(value().into()),
            "--out" => a.out = Some(value().into()),
            "--image" => a.init.image = value(),
            "--config-dir" => a.config_dir = value().into(),
            "--domain" => a.init.domain = value(),
            "--port" => a.init.port = value().parse().unwrap_or_else(|_| usage_error("--port is 1 to 65535")),
            "--gateway" => a.init.gateway = value(),
            "-h" | "--help" => {
                println!("{USAGE}");
                exit(0)
            }
            "-V" | "--version" => {
                println!("fleetbox {}", env!("CARGO_PKG_VERSION"));
                exit(0)
            }
            _ => usage_error(&format!("unknown argument {arg}")),
        }
    }
    a
}

fn load(path: &std::path::Path) -> Manifest {
    Manifest::load(path).unwrap_or_else(|e| {
        logs::say(e.to_string());
        exit(2)
    })
}

fn main() {
    let args = parse_args();
    match args.command.as_str() {
        "run" => run(args),
        "check" => {
            let m = load(&args.manifest);
            println!("{}: version {}, {} services, start order {}", args.manifest.display(), m.version, m.services.len(),
                m.start_order().map(|o| o.join(", ")).unwrap_or_default());
        }
        "health" => match health::read(&args.run_dir) {
            Ok((healthy, text)) => {
                print!("{text}");
                exit(if healthy { 0 } else { 1 })
            }
            Err(e) => {
                eprintln!("fleetbox: {e}");
                exit(1)
            }
        },
        "image" => image(args),
        "init" => init(args),
        c => usage_error(&format!("unknown command {c}")),
    }
}

/// Writes the box's build context: its Dockerfile, and the manifest it copies (docs/USER.md, The image).
fn image(args: Args) {
    let (Some(lock), Some(out)) = (&args.lock, &args.out) else { usage_error("image needs --lock and --out") };
    let m = load(&args.manifest);
    let fail = |e: String| -> ! {
        logs::say(e);
        exit(2)
    };
    let lock = fleetbox::image::Lock::load(lock).unwrap_or_else(|e| fail(e.to_string()));
    let dockerfile = fleetbox::image::dockerfile(&lock, &m).unwrap_or_else(|e| fail(e.to_string()));
    let write = |name: &str, bytes: &[u8]| {
        let path = out.join(name);
        std::fs::write(&path, bytes).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    };
    std::fs::create_dir_all(out).unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));
    // The manifest as it was read and checked, byte for byte.
    write("manifest.yaml", &std::fs::read(&args.manifest).unwrap_or_else(|e| fail(format!("{}: {e}", args.manifest.display()))));
    write("Dockerfile", dockerfile.as_bytes());
}

/// Writes the config directory a box runs from by hand (docs/USER.md, Running a box by hand).
fn init(args: Args) {
    if args.init.image.is_empty() {
        usage_error("init needs --image")
    }
    match fleetbox::init::init(&args.config_dir, &args.manifest, &args.init) {
        Ok(done) => done.iter().for_each(|l| println!("{l}")),
        Err(e) => {
            logs::say(e.to_string());
            exit(2)
        }
    }
}

fn run(args: Args) {
    // Not PID 1 (under a test, or behind another init): orphans of the services are reparented to the
    // supervisor all the same, and it reaps them.
    if std::process::id() != 1 {
        // SAFETY: prctl with integer arguments.
        unsafe {
            libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
        }
    }
    let m = load(&args.manifest);
    let opts = Options { fail_fast: args.fail_fast, run_dir: args.run_dir, uni_config: args.uni_config, observer: None };
    let (sup, handle) = Supervisor::new(m, opts).unwrap_or_else(|e| {
        logs::say(e.to_string());
        exit(2)
    });
    let mut signals = Signals::new([SIGTERM, SIGINT]).unwrap_or_else(|e| {
        logs::say(format!("cannot handle signals: {e}"));
        exit(2)
    });
    std::thread::spawn(move || {
        for sig in signals.forever() {
            handle.signal(sig);
        }
    });
    exit(sup.run().exit_code())
}
