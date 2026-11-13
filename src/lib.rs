//! RustTemp: a terminal-only CPU temperature monitor.
//!
//! Module layout:
//! - [`cli`]: command-line parsing and validated configuration.
//! - [`sensors`]: sensor model, CPU-sensor classification, platform backends.
//! - [`cpuinfo`]: CPU identification (manufacturer, model, topology, caches).
//! - [`health`]: throttling and thermal-headroom summary.
//! - [`perf`]: CPU speed, load and power.
//! - [`measurement`]: measured / unavailable / not-applicable values with sources.
//! - [`hardware`]: hardware identification tables.
//! - [`report`]: `cpu` / `gpu` report data models.
//! - [`commands`]: the direct `cpu` / `gpu` commands.
//! - [`monitoring`]: sampling loop, statistics and thermal thresholds.
//! - [`terminal`]: human-readable rendering.
//! - [`output`]: JSON serialization.
//! - [`app`]: wires the pieces together for each command.

pub mod app;
pub mod cli;
pub mod commands;
pub mod cpuinfo;
pub mod errors;
pub mod hardware;
pub mod health;
pub mod measurement;
pub mod monitoring;
pub mod output;
pub mod perf;
pub mod report;
pub mod sensors;
pub mod terminal;
pub mod time;

pub use errors::{Error, Result};
