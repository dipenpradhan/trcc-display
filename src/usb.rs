//! USB layer (libusb via `rusb`).
//!
//! The device speaks HID interrupt transfers on interface 0 (OUT `0x02`, IN
//! `0x81`). One quirk drives the design: **the LED firmware answers the
//! handshake only once per power cycle** — a second open gets garbage. So we
//! persist the handshake result (`PM`/`SUB`) to a small JSON cache and fall
//! back to it when a live handshake fails.
//!
//! # Threading
//!
//! The worker runs on a dedicated thread (not the tokio runtime). It consumes
//! ready-built packets from a [`Receiver`] and writes them to the device. The
//! async engine never touches libusb directly.
//!
//! # Reconnection
//!
//! The worker reconnects automatically on unplug or error. It sleeps 3
//! seconds between reconnection attempts.
//!
//! # Cache
//!
//! The handshake result is cached in `cache_path` (default `state/probe_cache.json`).
//! The cache is keyed by VID:PID and persists across restarts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use rusb::{DeviceHandle, GlobalContext};
use serde::{Deserialize, Serialize};

use crate::protocol;
use crate::state::Shared;

const WRITE_TIMEOUT: Duration = Duration::from_millis(200);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(1500);
const HANDSHAKE_RETRIES: u32 = 3;
const RECONNECT_DELAY: Duration = Duration::from_secs(3);

/// USB device configuration.
///
/// # Fields
///
/// * `vendor_id` — USB vendor ID (decimal, e.g. 1046 for 0x0416).
/// * `product_id` — USB product ID (decimal, e.g. 32769 for 0x8001).
/// * `interface` — HID interface number (usually 0).
/// * `cache_path` — path to the probe cache file.
#[derive(Clone, Debug)]
pub struct UsbConfig {
    pub vendor_id: u16,
    pub product_id: u16,
    pub interface: u8,
    pub cache_path: PathBuf,
}

/// Result of identifying a device.
///
/// # Fields
///
/// * `pm` — device family byte (selects the profile).
/// * `sub` — wire-remap sub-variant.
/// * `from_cache` — `true` if the result came from the on-disk cache.
#[derive(Debug, Clone, Copy)]
pub struct Handshake {
    pub pm: u8,
    pub sub: u8,
    pub from_cache: bool,
}

// ── One-shot helpers (used by the `detect` / `probe` CLI subcommands) ───────

/// List all connected devices matching the configured VID/PID.
///
/// # Arguments
///
/// * `vendor_id` — USB vendor ID (decimal).
/// * `product_id` — USB product ID (decimal).
///
/// # Returns
///
/// A vector of human-readable device strings.
pub fn list(vendor_id: u16, product_id: u16) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for dev in rusb::devices().context("enumerating USB devices")?.iter() {
        let desc = dev.device_descriptor().context("reading descriptor")?;
        if desc.vendor_id() == vendor_id && desc.product_id() == product_id {
            found.push(format!(
                "bus {:03} device {:03}: {:04x}:{:04x}",
                dev.bus_number(),
                dev.address(),
                desc.vendor_id(),
                desc.product_id()
            ));
        }
    }
    Ok(found)
}

/// Open the device and perform (or cache-restore) the handshake once.
///
/// # Arguments
///
/// * `cfg` — USB configuration (VID, PID, cache path).
///
/// # Returns
///
/// The handshake result (PM, SUB, from_cache).
pub fn probe(cfg: &UsbConfig) -> Result<Handshake> {
    let (_handle, hs) = open_session(cfg)?;
    Ok(hs)
}

/// Open the device **and** run the init/handshake on that same handle, returning
/// both. The display only accepts data frames on a connection that has been
/// initialized, so one-shot writers must keep and reuse this handle (don't open
/// a fresh one to write — the frame would be ignored and the screen stays blank).
///
/// # Arguments
///
/// * `cfg` — USB configuration.
///
/// # Returns
///
/// A tuple of `(DeviceHandle, Handshake)`.
pub fn open_session(cfg: &UsbConfig) -> Result<(DeviceHandle<GlobalContext>, Handshake)> {
    let mut handle = open(cfg)?;
    let hs = handshake(&mut handle, cfg)?;
    Ok((handle, hs))
}

// ── Worker thread ──────────────────────────────────────────────────────────

/// Run the USB worker until `rx` is dropped. Reconnects on unplug/error, keeps
/// [`Shared`] status current, and writes each packet it receives.
///
/// # Arguments
///
/// * `cfg` — USB configuration.
/// * `shared` — shared runtime state.
/// * `rx` — channel to receive packets from the engine.
pub fn run(cfg: UsbConfig, shared: Arc<Mutex<Shared>>, rx: Receiver<Vec<u8>>) {
    // Guard against rusb panicking on libusb_init() when USB hardware is
    // unavailable (containers, simulators, no USB bus).
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rusb::devices().map(|_| true).is_err()
    })).is_err() {
        tracing::warn!("USB subsystem unavailable (no USB hardware); USB worker disabled");
        return;
    }

    loop {
        // Re-check on each iteration in case USB became unavailable later.
        let devices_ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rusb::devices().is_ok()
        }));

        match devices_ok {
            Ok(true) => {} // USB OK, proceed
            _ => {
                tracing::warn!("USB subsystem gone; USB worker exiting");
                return;
            }
        }

        match connect(&cfg, &shared) {
            Ok(mut handle) => {
                tracing::info!("device connected");
                if let Err(e) = write_loop(&mut handle, &rx) {
                    tracing::warn!(error = %e, "write loop ended; will reconnect");
                    set_disconnected(&shared, Some(e.to_string()));
                } else {
                    // rx dropped → engine is shutting down.
                    tracing::info!("engine channel closed; USB worker exiting");
                    return;
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "device not available");
                set_disconnected(&shared, Some(e.to_string()));
            }
        }
        std::thread::sleep(RECONNECT_DELAY);
    }
}

fn connect(cfg: &UsbConfig, shared: &Arc<Mutex<Shared>>) -> Result<DeviceHandle<GlobalContext>> {
    let mut handle = open(cfg)?;
    let hs = handshake(&mut handle, cfg)?;
    if let Ok(mut s) = shared.lock() {
        s.connected = true;
        s.pm = Some(hs.pm);
        s.sub = Some(hs.sub);
        s.last_error = None;
    }
    tracing::info!(
        pm = hs.pm,
        sub = hs.sub,
        from_cache = hs.from_cache,
        "handshake ok"
    );
    Ok(handle)
}

/// Drain packets and write them until the channel closes or a write fails.
fn write_loop(handle: &mut DeviceHandle<GlobalContext>, rx: &Receiver<Vec<u8>>) -> Result<()> {
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(packet) => write_frame(handle, &packet)?,
            // Idle: no frame this second — loop and check again.
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Chunk a packet into 64-byte HID reports and write them to the device.
///
/// # Arguments
///
/// * `handle` — open device handle.
/// * `packet` — a full data packet from `protocol::data_packet`.
pub fn write_frame(handle: &mut DeviceHandle<GlobalContext>, packet: &[u8]) -> Result<()> {
    for report in protocol::chunks(packet) {
        handle
            .write_interrupt(protocol::EP_WRITE, &report, WRITE_TIMEOUT)
            .context("interrupt write to display")?;
    }
    Ok(())
}

/// Open the device and claim its interface (Linux detaches the kernel HID
/// driver automatically). Public for the one-shot CLI subcommands.
pub fn open(cfg: &UsbConfig) -> Result<DeviceHandle<GlobalContext>> {
    let handle = open_handle(cfg)?;
    // Linux: hand the interface over from the kernel HID driver. Other OSes
    // return NotSupported — harmless.
    let _ = handle.set_auto_detach_kernel_driver(true);
    handle.claim_interface(cfg.interface).with_context(|| {
        format!(
            "claiming interface {} on {:04x}:{:04x} (already in use by another process?)",
            cfg.interface, cfg.vendor_id, cfg.product_id
        )
    })?;
    Ok(handle)
}

/// Locate and open the device, giving a precise error for the common failures:
/// device absent vs. permission denied (the udev-rule case).
fn open_handle(cfg: &UsbConfig) -> Result<DeviceHandle<GlobalContext>> {
    for dev in rusb::devices().context("enumerating USB devices")?.iter() {
        let desc = dev.device_descriptor().context("reading USB descriptor")?;
        if desc.vendor_id() != cfg.vendor_id || desc.product_id() != cfg.product_id {
            continue;
        }
        return dev.open().map_err(|e| match e {
            rusb::Error::Access => anyhow::anyhow!(
                "permission denied opening {:04x}:{:04x} (bus {:03} device {:03}). \
                 Install the udev rule (packaging/udev/99-thermalright-trcc.rules) and \
                 replug — or run as root.",
                cfg.vendor_id,
                cfg.product_id,
                dev.bus_number(),
                dev.address()
            ),
            other => anyhow::anyhow!(
                "opening {:04x}:{:04x}: {other}",
                cfg.vendor_id,
                cfg.product_id
            ),
        });
    }
    anyhow::bail!(
        "no device {:04x}:{:04x} found — not plugged in, or not passed through to this host",
        cfg.vendor_id,
        cfg.product_id
    )
}

fn handshake(handle: &mut DeviceHandle<GlobalContext>, cfg: &UsbConfig) -> Result<Handshake> {
    let init = protocol::init_packet();
    let mut last_err: Option<anyhow::Error> = None;

    for attempt in 1..=HANDSHAKE_RETRIES {
        std::thread::sleep(Duration::from_millis(50));
        if let Err(e) = handle.write_interrupt(protocol::EP_WRITE, &init, HANDSHAKE_TIMEOUT) {
            last_err = Some(anyhow::Error::new(e).context("handshake write"));
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        std::thread::sleep(Duration::from_millis(200));

        let mut buf = [0u8; protocol::HID_REPORT_SIZE];
        match handle.read_interrupt(protocol::EP_READ, &mut buf, HANDSHAKE_TIMEOUT) {
            Ok(n) => {
                if let Some((pm, sub)) = protocol::parse_handshake(&buf[..n]) {
                    cache_save(&cfg.cache_path, cfg.vendor_id, cfg.product_id, pm, sub);
                    return Ok(Handshake {
                        pm,
                        sub,
                        from_cache: false,
                    });
                }
                last_err = Some(anyhow::anyhow!("handshake response too short ({n} bytes)"));
            }
            Err(e) => last_err = Some(anyhow::Error::new(e).context("handshake read")),
        }
        tracing::debug!(attempt, "handshake attempt failed");
        std::thread::sleep(Duration::from_millis(500));
    }

    // Live handshake exhausted — the firmware likely already answered earlier
    // this power cycle. Fall back to the on-disk cache.
    if let Some((pm, sub)) = cache_load(&cfg.cache_path, cfg.vendor_id, cfg.product_id) {
        tracing::warn!("live handshake failed; using cached identity");
        return Ok(Handshake {
            pm,
            sub,
            from_cache: true,
        });
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("handshake failed")))
        .context("no cached identity available either")
}

fn set_disconnected(shared: &Arc<Mutex<Shared>>, err: Option<String>) {
    if let Ok(mut s) = shared.lock() {
        s.connected = false;
        if err.is_some() {
            s.last_error = err;
        }
    }
}

// ── Probe cache (handshake answers only once per power cycle) ───────────────

#[derive(Serialize, Deserialize, Default)]
struct ProbeCache(HashMap<String, CacheEntry>);

#[derive(Serialize, Deserialize, Clone, Copy)]
struct CacheEntry {
    pm: u8,
    sub: u8,
}

fn cache_key(vid: u16, pid: u16) -> String {
    format!("{vid:04x}_{pid:04x}")
}

fn cache_save(path: &PathBuf, vid: u16, pid: u16, pm: u8, sub: u8) {
    let mut cache = read_cache(path).unwrap_or_default();
    cache.0.insert(cache_key(vid, pid), CacheEntry { pm, sub });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(&cache) {
        Ok(text) => {
            if let Err(e) = std::fs::write(path, text) {
                tracing::warn!(error = %e, "could not write probe cache");
            }
        }
        Err(e) => tracing::warn!(error = %e, "could not serialize probe cache"),
    }
}

fn cache_load(path: &PathBuf, vid: u16, pid: u16) -> Option<(u8, u8)> {
    let cache = read_cache(path)?;
    let e = cache.0.get(&cache_key(vid, pid))?;
    Some((e.pm, e.sub))
}

fn read_cache(path: &PathBuf) -> Option<ProbeCache> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── UsbConfig ────────────────────────────────────────────────────────────

    #[test]
    fn usb_config_clone() {
        let cfg = UsbConfig {
            vendor_id: 1046,
            product_id: 32769,
            interface: 0,
            cache_path: PathBuf::from("state/probe_cache.json"),
        };
        let _ = cfg.clone();
    }

    // ── Handshake ────────────────────────────────────────────────────────────

    #[test]
    fn handshake_debug() {
        let hs = Handshake {
            pm: 3,
            sub: 9,
            from_cache: false,
        };
        let _ = format!("{hs:?}");
    }

    // ── cache_key ────────────────────────────────────────────────────────────

    #[test]
    fn cache_key_format() {
        assert_eq!(cache_key(1046, 32769), "0416_8001");
        assert_eq!(cache_key(1, 256), "0001_0100");
    }

    // ── Probe cache save/load ────────────────────────────────────────────────

    #[test]
    fn cache_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        cache_save(&path, 1046, 32769, 3, 9);
        let result = cache_load(&path, 1046, 32769);
        assert_eq!(result, Some((3, 9)));
    }

    #[test]
    fn cache_load_missing_file() {
        let path = PathBuf::from("/nonexistent/cache.json");
        let result = cache_load(&path, 1046, 32769);
        assert!(result.is_none());
    }

    #[test]
    fn cache_load_wrong_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        cache_save(&path, 1046, 32769, 3, 9);
        let result = cache_load(&path, 1046, 0xFFFF);
        assert!(result.is_none());
    }

    #[test]
    fn cache_multiple_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        cache_save(&path, 1046, 32769, 3, 9);
        cache_save(&path, 1234, 5678, 5, 10);

        assert_eq!(cache_load(&path, 1046, 32769), Some((3, 9)));
        assert_eq!(cache_load(&path, 1234, 5678), Some((5, 10)));
    }

    #[test]
    fn cache_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        cache_save(&path, 1046, 32769, 1, 1);
        cache_save(&path, 1046, 32769, 2, 2);
        assert_eq!(cache_load(&path, 1046, 32769), Some((2, 2)));
    }

    #[test]
    fn cache_creates_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deep/cache.json");
        cache_save(&path, 1046, 32769, 3, 9);
        assert!(path.exists());
        assert_eq!(cache_load(&path, 1046, 32769), Some((3, 9)));
    }
}
