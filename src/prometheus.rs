//! Minimal Prometheus HTTP query client — instant queries only.
//!
//! This module handles communication with Prometheus servers. It sends
//! instant PromQL queries and extracts the first series' scalar value.
//!
//! # Design
//!
//! The client is async and uses `reqwest` for HTTP. Each query is independent;
//! there is no caching or connection pooling beyond what `reqwest` provides.
//!
//! # Error handling
//!
//! `query` returns `Result<Option<f64>>`:
//! - `Ok(Some(v))` — query succeeded with data
//! - `Ok(None)` — query succeeded but matched no series
//! - `Err(e)` — network error or malformed response

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

/// Prometheus instant query client.
///
/// Sends PromQL queries to a Prometheus server and extracts the first
/// series' scalar value.
#[derive(Clone, Debug)]
pub struct PromClient {
    http: reqwest::Client,
    base: String,
}

#[derive(Deserialize)]
struct QueryResponse {
    status: String,
    #[serde(default)]
    data: Option<QueryData>,
}

#[derive(Deserialize)]
struct QueryData {
    result: Vec<InstantResult>,
}

#[derive(Deserialize)]
struct InstantResult {
    /// `[unix_ts, "value"]`
    value: (f64, String),
}

impl PromClient {
    /// Create a new Prometheus client.
    ///
    /// # Arguments
    ///
    /// * `base` — base URL (e.g., `"http://localhost:9090"`).
    /// * `timeout_seconds` — per-query timeout (minimum 1 second).
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be built.
    pub fn new(base: &str, timeout_seconds: u64) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_seconds.max(1)))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
        })
    }

    /// Run an instant query and return the first series' scalar value.
    ///
    /// `Ok(None)` means the query succeeded but matched no series; `Err` means
    /// the request or the response was malformed (kept separate so the caller
    /// can distinguish "no data" from "Prometheus unreachable").
    ///
    /// # Arguments
    ///
    /// * `promql` — the PromQL expression to evaluate.
    pub async fn query(&self, promql: &str) -> Result<Option<f64>> {
        let url = format!("{}/api/v1/query", self.base);
        let resp = self
            .http
            .get(&url)
            .query(&[("query", promql)])
            .send()
            .await
            .with_context(|| format!("querying {url}"))?
            .error_for_status()
            .context("Prometheus returned an HTTP error")?
            .json::<QueryResponse>()
            .await
            .context("decoding Prometheus response")?;

        anyhow::ensure!(resp.status == "success", "query status: {}", resp.status);
        let Some(data) = resp.data else {
            return Ok(None);
        };
        let Some(first) = data.result.first() else {
            return Ok(None);
        };
        let v = first
            .value
            .1
            .parse::<f64>()
            .with_context(|| format!("parsing value {:?}", first.value.1))?;
        Ok(Some(v))
    }
}
