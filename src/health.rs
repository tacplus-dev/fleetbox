// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

//! `fleetbox health`: the box's state, from the status the supervisor writes (docs/USER.md, Health).

use std::path::Path;

/// The box's state and one line per service, and whether the box is healthy.
pub fn read(run_dir: &Path) -> Result<(bool, String), String> {
    let path = run_dir.join("status.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let state = doc["state"].as_str().unwrap_or("unknown");
    let mut out = format!("{state}\n");
    if let Some(services) = doc["services"].as_object() {
        for (name, s) in services {
            out.push_str(&format!(
                "{name}: {} (restarts {}{})\n",
                s["state"].as_str().unwrap_or("unknown"),
                s["restarts"],
                if s["unhealthy"].as_bool() == Some(true) { ", unhealthy" } else { "" }
            ));
        }
    }
    Ok((state == "healthy", out))
}
