// Copyright (c) 2026 Jody Scott
// SPDX-License-Identifier: Apache-2.0

//! fleetbox: the supervisor that runs a whole tacplus fleet in one container (docs/USER.md).

pub mod config;
pub mod health;
pub mod image;
pub mod logs;
pub mod manifest;
pub mod readiness;
pub mod reaper;
pub mod supervisor;
