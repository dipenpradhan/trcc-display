// SPDX-License-Identifier: Apache-2.0
//! # trcc-display
//!
//! Drive Thermalright "Digital" cooler LED/segment displays (USB `0416:8001`)
//! from live metrics. Values can come from **Prometheus** or the local
//! **`sensors -j`** (lm-sensors) output, and the display can be driven headless
//! or via a small **REST API**. It is a clean-room reimplementation of the LED
//! wire protocol (the same one [`trrc-linux`](https://github.com/Lexonight1/thermalright-trrc-linux)
//! speaks); no Python, no libusb at runtime (libusb is vendored into the binary).
//!
//! ## Architecture
//!
//! ```text
//!   metric source ──▶ engine ──▶ render ──▶ protocol ──▶ usb worker ──▶ cooler
//!   (prometheus |     (loop +    (7-seg     (packet      (libusb, own
//!    sensors -j)      overrides)  digits)    framing)     thread)
//!         ▲
//!    REST API (optional) sets overrides / raw frames
//! ```
//!
//! ## Modules
//!
//! * [`protocol`] — pure wire framing + 7-segment font (no I/O).
//! * [`profile`] — per-model LED geometry, loaded from JSON.
//! * [`render`] — value → physical-order color frame.
//! * [`config`] — the JSON config file.
//! * [`source`] / [`prometheus`] / [`sensors`] — where values come from.
//! * [`usb`] — libusb worker (handshake, probe cache, frame writes).
//! * [`engine`] — the render loop.
//! * [`api`] — optional REST control surface.
//! * [`state`] — shared runtime state (`Shared`, `Override`, `RawOverride`).
//! * [`util`] — shared utilities (lock helpers).
//!
//! ## Quick start
//!
//! ```no_run
//! use trcc_display::config::Config;
//! use trcc_display::profile::ProfileSet;
//! use std::path::Path;
//!
//! let cfg = Config::load(Path::new("config/config.json")).unwrap();
//! let profiles = ProfileSet::load_dir(Path::new(&cfg.profile.dir)).unwrap();
//! ```
//!
//! ## Safety
//!
//! This crate uses **zero** `unsafe` code. All libusb interaction is through
//! the safe `rusb` crate. The wire protocol is pure byte manipulation.

pub mod api;
pub mod config;
pub mod engine;
pub mod preview;
pub mod profile;
pub mod prometheus;
pub mod protocol;
pub mod render;
pub mod sensors;
pub mod source;
pub mod state;
pub mod usb;
pub mod util;
