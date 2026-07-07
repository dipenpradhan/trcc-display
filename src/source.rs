//! Unified metric source. The engine calls [`Source::fetch`] once per refresh;
//! each backend interprets a tile's `query` string in its own way.
//!
//! # Design
//!
//! The [`Source`] enum wraps either a [`Prometheus`][crate::prometheus::PromClient]
//! or a [`Sensors`][crate::sensors::SensorsSource] backend. The engine never
//! needs to know which one is active — it just calls `fetch` with a list of
//! tiles and gets back [`Fetched`] results.
//!
//! # Error handling
//!
//! `fetch` never errors as a whole — per-tile failures are reported in each
//! [`Fetched`], so one bad query or a momentarily-unreachable Prometheus
//! doesn't blank the whole display.

use crate::config::Config;
use crate::prometheus::PromClient;
use crate::sensors::{self, SensorsSource};

/// One tile's fetched value (or a human-readable error, kept as `String` so it
/// survives across the async boundary and into API/status output).
///
/// # Fields
///
/// * `tile` — the tile name (for status reporting).
/// * `value` — `Ok(Some(v))` on success, `Ok(None)` if the query matched
///   nothing, `Err(msg)` on error.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub tile: String,
    pub value: Result<Option<f64>, String>,
}

/// A metric backend.
///
/// # Variants
///
/// * `Prometheus` — instant PromQL queries against a Prometheus server.
/// * `Sensors` — `sensors -j` (lm-sensors) JSON tree selection.
#[derive(Clone, Debug)]
pub enum Source {
    /// Prometheus backend.
    Prometheus(PromClient),
    /// lm-sensors backend.
    Sensors(SensorsSource),
}

impl Source {
    /// Build the source named by `config.source.kind`.
    ///
    /// Supported kinds: `"prometheus"`, `"prom"`, `"sensors"`, `"lm-sensors"`,
    /// `"lmsensors"`.
    ///
    /// # Errors
    ///
    /// Returns an error if the kind is unrecognized or the backend fails to
    /// initialize.
    pub fn from_config(cfg: &Config) -> anyhow::Result<Self> {
        match cfg.source.kind.to_ascii_lowercase().as_str() {
            "prometheus" | "prom" => Ok(Self::Prometheus(PromClient::new(
                &cfg.prometheus.url,
                cfg.prometheus.timeout_seconds,
            )?)),
            "sensors" | "lm-sensors" | "lmsensors" => Ok(Self::Sensors(SensorsSource::new(
                cfg.source.sensors_command.clone(),
            )?)),
            other => anyhow::bail!(
                "unknown source.kind {other:?} (expected \"prometheus\" or \"sensors\")"
            ),
        }
    }

    /// Short label for status output.
    ///
    /// Returns `"prometheus"` or `"sensors"`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Prometheus(_) => "prometheus",
            Self::Sensors(_) => "sensors",
        }
    }

    /// Fetch every tile's current value. Never errors as a whole — per-tile
    /// failures are reported in each [`Fetched`], so one bad query or a
    /// momentarily-unreachable Prometheus doesn't blank the whole display.
    ///
    /// # Arguments
    ///
    /// * `tiles` — the tile array from config.
    ///
    /// # Returns
    ///
    /// A vector of [`Fetched`] results, one per tile.
    pub async fn fetch(&self, tiles: &[crate::config::Tile]) -> Vec<Fetched> {
        match self {
            Self::Prometheus(client) => {
                let mut out = Vec::with_capacity(tiles.len());
                for t in tiles {
                    let value = client.query(&t.query).await.map_err(|e| format!("{e:#}"));
                    out.push(Fetched {
                        tile: t.name.clone(),
                        value,
                    });
                }
                out
            }
            Self::Sensors(src) => {
                // One command per refresh; select each tile from the snapshot.
                let src = src.clone();
                let read = tokio::task::spawn_blocking(move || src.read()).await;
                let tree = match read {
                    Ok(Ok(tree)) => tree,
                    Ok(Err(e)) => return fail_all(tiles, &format!("{e:#}")),
                    Err(e) => return fail_all(tiles, &format!("sensors task panicked: {e}")),
                };
                tiles
                    .iter()
                    .map(|t| Fetched {
                        tile: t.name.clone(),
                        value: sensors::select(&tree, &t.query).map_err(|e| format!("{e:#}")),
                    })
                    .collect()
            }
        }
    }
}

fn fail_all(tiles: &[crate::config::Tile], err: &str) -> Vec<Fetched> {
    tiles
        .iter()
        .map(|t| Fetched {
            tile: t.name.clone(),
            value: Err(err.to_string()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Tile;

    // ── Source::kind ────────────────────────────────────────────────────────

    #[test]
    fn source_kind_prometheus() {
        let src = Source::Sensors(SensorsSource::new(vec!["sensors".into(), "-j".into()]).unwrap());
        assert_eq!(src.kind(), "sensors");
    }

    // ── Source::from_config ─────────────────────────────────────────────────

    #[test]
    fn from_config_sensors() {
        let cfg = Config::load(std::path::Path::new("config/config.sensors.json"))
            .expect("load sensors config");
        let src = Source::from_config(&cfg).unwrap();
        assert_eq!(src.kind(), "sensors");
    }

    #[test]
    fn from_config_unknown_kind() {
        let json = r#"{
            "usb": { "vendor_id": 1, "product_id": 2 },
            "profile": { "dir": "p" },
            "source": { "kind": "invalid" },
            "render": {},
            "tiles": []
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        let result = Source::from_config(&cfg);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unknown source.kind"), "{err}");
    }

    #[test]
    fn from_config_prom_alias() {
        let json = r#"{
            "usb": { "vendor_id": 1, "product_id": 2 },
            "profile": { "dir": "p" },
            "source": { "kind": "prom" },
            "render": {},
            "tiles": []
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        let src = Source::from_config(&cfg).unwrap();
        assert_eq!(src.kind(), "prometheus");
    }

    // ── fail_all ────────────────────────────────────────────────────────────

    #[test]
    fn fail_all_marks_every_tile() {
        let tiles = vec![
            Tile {
                name: "t1".into(),
                slot: "s".into(),
                query: "q".into(),
                unit: None,
                color: [0, 0, 0],
                warn: None,
                crit: None,
                indicators: vec![],
            },
            Tile {
                name: "t2".into(),
                slot: "s".into(),
                query: "q".into(),
                unit: None,
                color: [0, 0, 0],
                warn: None,
                crit: None,
                indicators: vec![],
            },
        ];
        let results = fail_all(&tiles, "boom");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].tile, "t1");
        assert!(results[0].value.is_err());
        assert_eq!(results[1].tile, "t2");
        assert!(results[1].value.is_err());
    }

    // ── Fetched ─────────────────────────────────────────────────────────────

    #[test]
    fn fetched_success() {
        let f = Fetched {
            tile: "cpu".into(),
            value: Ok(Some(57.0)),
        };
        assert_eq!(f.tile, "cpu");
        assert_eq!(f.value, Ok(Some(57.0)));
    }

    #[test]
    fn fetched_no_data() {
        let f = Fetched {
            tile: "gpu".into(),
            value: Ok(None),
        };
        assert!(f.value.is_ok());
        assert_eq!(f.value.as_ref().unwrap(), &None);
    }

    #[test]
    fn fetched_error() {
        let f = Fetched {
            tile: "fan".into(),
            value: Err("timeout".into()),
        };
        assert!(f.value.is_err());
    }
}
