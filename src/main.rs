// SPDX-License-Identifier: Apache-2.0
//! `trcc-display` command-line entry point.

use std::path::{Path, PathBuf};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use trcc_display::api::{self, AppState};
use trcc_display::config::Config;
use trcc_display::engine;
use trcc_display::profile::{Profile, ProfileSet};
use trcc_display::protocol;
use trcc_display::render::{self, SlotValue};
use trcc_display::source::Source;
use trcc_display::state::Shared;
use trcc_display::usb;

/// Drive a Thermalright Digital cooler LED/segment display from Prometheus or
/// lm-sensors, headless or over REST.
#[derive(Parser, Debug)]
#[command(name = "trcc-display", version, about, long_about = None)]
struct Cli {
    /// Path to the JSON config file.
    #[arg(long, short, default_value = "config.json", global = true)]
    config: PathBuf,

    /// Increase log verbosity (-v = debug, -vv = trace). `RUST_LOG` overrides.
    #[arg(long, short, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the driver: refresh metrics, render, and serve the REST API (default).
    Run,
    /// List connected devices matching the configured USB VID/PID.
    Detect,
    /// Open the device, handshake, and print its identity + resolved profile.
    Probe,
    /// Fetch metrics once, render a single frame, and exit.
    Once,
    /// Render one explicit value to a slot and exit (no metric source).
    Render {
        /// Target slot (see the profile, e.g. `main`, `gpu_temp`).
        #[arg(long)]
        slot: String,
        /// Number to display.
        #[arg(long)]
        value: f64,
        /// Unit marker to light (`celsius`/`fahrenheit`/`percent`).
        #[arg(long)]
        unit: Option<String>,
        /// RGB color as `r,g,b` (0-255).
        #[arg(long, default_value = "0,200,120")]
        color: String,
    },
    /// Diagnostic: walk the LEDs (or light them all) to map the physical layout.
    TestPattern {
        /// `walk` (one LED at a time) or `all` (every LED at once).
        #[arg(long, default_value = "walk")]
        mode: String,
        /// RGB color as `r,g,b`.
        #[arg(long, default_value = "255,255,255")]
        color: String,
        /// Per-step delay in `walk` mode.
        #[arg(long, default_value_t = 250)]
        delay_ms: u64,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command.unwrap_or(Command::Run) {
        Command::Run => run(&cli.config),
        Command::Detect => detect(&cli.config),
        Command::Probe => probe(&cli.config),
        Command::Once => once(&cli.config),
        Command::Render {
            slot,
            value,
            unit,
            color,
        } => render_once(&cli.config, &slot, value, unit, &color),
        Command::TestPattern {
            mode,
            color,
            delay_ms,
        } => test_pattern(&cli.config, &mode, &color, delay_ms),
    }
}

fn init_tracing(verbose: u8) {
    let default = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(format!("trcc_display={default},warn"))
    });
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

// ── config helpers ─────────────────────────────────────────────────────────

fn load(config_path: &Path) -> Result<(Config, ProfileSet)> {
    let cfg = Config::load(config_path)?;
    let profiles = ProfileSet::load_dir(Path::new(&cfg.profile.dir))
        .with_context(|| format!("loading profiles from {}", cfg.profile.dir))?;
    Ok((cfg, profiles))
}

fn usb_config(cfg: &Config) -> usb::UsbConfig {
    // Cache dir is overridable for systemd (StateDirectory) / packaging.
    let cache_path = std::env::var_os("TRCC_CACHE_PATH")
        .map_or_else(|| PathBuf::from("state/probe_cache.json"), PathBuf::from);
    usb::UsbConfig {
        vendor_id: cfg.usb.vendor_id,
        product_id: cfg.usb.product_id,
        interface: cfg.usb.interface,
        cache_path,
    }
}

fn parse_rgb(s: &str) -> Result<protocol::Rgb> {
    let parts: Vec<&str> = s.split(',').collect();
    anyhow::ensure!(parts.len() == 3, "color must be r,g,b (got {s:?})");
    let p = |i: usize| -> Result<u8> {
        parts[i]
            .trim()
            .parse::<u8>()
            .with_context(|| format!("color component {:?} not a 0-255 integer", parts[i]))
    };
    Ok(protocol::Rgb(p(0)?, p(1)?, p(2)?))
}

/// Resolve the device's profile from an already-known handshake PM (or the
/// `profile.force` override). Pure — the caller holds the open session.
fn profile_for(cfg: &Config, profiles: &ProfileSet, pm: u8) -> Result<Profile> {
    if let Some(name) = &cfg.profile.force {
        return profiles.by_name(name).cloned().with_context(|| {
            format!(
                "forced profile {name:?} not found; have {:?}",
                profiles.names()
            )
        });
    }
    profiles.by_pm(pm).cloned().with_context(|| {
        format!(
            "no profile claims PM byte {pm}; have {:?}",
            profiles.names()
        )
    })
}

// ── subcommands ────────────────────────────────────────────────────────────

fn detect(config_path: &Path) -> Result<()> {
    let (cfg, _) = load(config_path)?;
    let found = usb::list(cfg.usb.vendor_id, cfg.usb.product_id)?;
    if found.is_empty() {
        println!(
            "no device {:04x}:{:04x} found (plugged in? passed through to this host? permissions?)",
            cfg.usb.vendor_id, cfg.usb.product_id
        );
    } else {
        println!("found {} device(s):", found.len());
        for d in found {
            println!("  {d}");
        }
    }
    Ok(())
}

fn probe(config_path: &Path) -> Result<()> {
    let (cfg, profiles) = load(config_path)?;
    let hs = usb::probe(&usb_config(&cfg))?;
    println!(
        "handshake ok: PM={} SUB={} (from_cache={})",
        hs.pm, hs.sub, hs.from_cache
    );
    match profiles.by_pm(hs.pm) {
        Some(p) => println!(
            "→ profile: {} ({} LEDs, style {})",
            p.name, p.mask_size, p.style
        ),
        None => println!(
            "→ no profile claims PM {}; add one under {} (have {:?})",
            hs.pm,
            cfg.profile.dir,
            profiles.names()
        ),
    }
    Ok(())
}

fn render_once(
    config_path: &Path,
    slot: &str,
    value: f64,
    unit: Option<String>,
    color: &str,
) -> Result<()> {
    let (cfg, profiles) = load(config_path)?;
    // Open ONCE and keep the initialized handle to write on.
    let (mut handle, hs) = usb::open_session(&usb_config(&cfg))?;
    let profile = profile_for(&cfg, &profiles, hs.pm)?;
    anyhow::ensure!(
        profile.slots.contains_key(slot),
        "slot {slot:?} not in profile {}; available: {:?}",
        profile.name,
        profile.slots.keys().collect::<Vec<_>>()
    );
    let rgb = parse_rgb(color)?;
    let sv = SlotValue {
        value: value.round() as i64,
        unit,
        color: rgb,
        indicators: Vec::new(),
    };
    let mut slots = std::collections::HashMap::new();
    slots.insert(slot.to_string(), sv);
    let indicator = protocol::Rgb(120, 120, 130);
    let frame = render::frame(&profile, &slots, indicator);
    usb::write_frame(&mut handle, &protocol::data_packet(&frame))?;
    println!("rendered {value} to slot {slot:?} on {}", profile.name);
    Ok(())
}

fn test_pattern(config_path: &Path, mode: &str, color: &str, delay_ms: u64) -> Result<()> {
    let (cfg, profiles) = load(config_path)?;
    let (mut handle, hs) = usb::open_session(&usb_config(&cfg))?;
    let profile = profile_for(&cfg, &profiles, hs.pm)?;
    let rgb = parse_rgb(color)?;
    match mode {
        "all" => {
            let colors = vec![rgb; profile.mask_size];
            let phys = profile.to_physical(&colors);
            usb::write_frame(&mut handle, &protocol::data_packet(&phys))?;
            println!("lit all {} LEDs; ctrl-c to stop", profile.mask_size);
            std::thread::sleep(Duration::from_secs(3600));
        }
        "walk" => {
            println!("walking {} LEDs ({}ms each)", profile.mask_size, delay_ms);
            for i in 0..profile.mask_size {
                let mut colors = vec![protocol::Rgb(0, 0, 0); profile.mask_size];
                colors[i] = rgb;
                let phys = profile.to_physical(&colors);
                usb::write_frame(&mut handle, &protocol::data_packet(&phys))?;
                println!("  LED {i}");
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
        }
        other => anyhow::bail!("unknown test-pattern mode {other:?} (use walk|all)"),
    }
    Ok(())
}

fn once(config_path: &Path) -> Result<()> {
    // A tiny async island for the source fetch.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    rt.block_on(async {
        let (cfg, profiles) = load(config_path)?;
        let (mut handle, hs) = usb::open_session(&usb_config(&cfg))?;
        let profile = profile_for(&cfg, &profiles, hs.pm)?;
        let source = Source::from_config(&cfg)?;
        let fetched = source.fetch(&cfg.tiles).await;
        let mut slots = std::collections::HashMap::new();
        for t in &cfg.tiles {
            if let Some(f) = fetched.iter().find(|f| f.tile == t.name) {
                match &f.value {
                    Ok(Some(v)) => {
                        println!("{:<12} = {v}", t.name);
                        slots.insert(
                            t.slot.clone(),
                            SlotValue {
                                value: v.round() as i64,
                                unit: t.unit.clone(),
                                color: render::threshold_color(*v, t.color_rgb(), t.warn, t.crit),
                                indicators: t.indicators.clone(),
                            },
                        );
                    }
                    Ok(None) => println!("{:<12} = (no data)", t.name),
                    Err(e) => println!("{:<12} = ERROR: {e}", t.name),
                }
            }
        }
        let indicator = cfg.indicator_color();
        let frame = render::frame(&profile, &slots, indicator);
        usb::write_frame(&mut handle, &protocol::data_packet(&frame))?;
        println!("sent one frame to {}", profile.name);
        Ok(())
    })
}

fn run(config_path: &Path) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    rt.block_on(run_async(config_path))
}

async fn run_async(config_path: &Path) -> Result<()> {
    let (cfg, profiles) = load(config_path)?;
    let source = Source::from_config(&cfg)?;
    tracing::info!(
        source = source.kind(),
        api = cfg.api.enabled,
        tiles = cfg.tiles.len(),
        "starting trcc-display"
    );

    let shared = Arc::new(Mutex::new(Shared::new()));
    let cfg_arc = Arc::new(Mutex::new(cfg.clone()));
    let profiles = Arc::new(profiles);

    // USB worker on its own thread.
    let (tx, rx) = sync_channel::<Vec<u8>>(2);
    let usb_cfg = usb_config(&cfg);
    let shared_usb = Arc::clone(&shared);
    std::thread::Builder::new()
        .name("usb-worker".into())
        .spawn(move || usb::run(usb_cfg, shared_usb, rx))
        .context("spawning USB worker")?;

    // Render loop.
    let engine = tokio::spawn(engine::run(
        Arc::clone(&cfg_arc),
        Arc::clone(&profiles),
        source,
        Arc::clone(&shared),
        tx,
    ));

    if cfg.api.enabled {
        let state = AppState {
            shared: Arc::clone(&shared),
            config: Arc::clone(&cfg_arc),
            config_path: config_path.to_path_buf(),
            profiles: Arc::clone(&profiles),
        };
        let shutdown = async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown signal received");
        };
        api::serve(state, &cfg.api.bind, cfg.api.preview_enabled, shutdown).await?;
    } else {
        tracing::info!("headless mode (REST API disabled); ctrl-c to stop");
        let _ = tokio::signal::ctrl_c().await;
    }

    engine.abort();
    Ok(())
}
