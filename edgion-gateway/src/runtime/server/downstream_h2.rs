//! Startup-only, bounded downstream HTTP/2 policy shared by TLS H2 and h2c.

use crate::cli::config::ServerConfig;
use anyhow::{ensure, Context, Result};
use pingora_core::protocols::http::v2::server::{default_h2_options, H2Options};
use pingora_core::services::listening::Service;
use pingora_proxy::{HttpProxy, ProxyHttp};

const DEFAULT_RECEIVE_WINDOW: u32 = 65_535;
const DEFAULT_CONCURRENT_STREAMS: u32 = 100;
const MAX_RECEIVE_WINDOW: u32 = 16 * 1024 * 1024;
const MAX_CONCURRENT_STREAMS: u32 = 1024;
const MAX_STREAM_WINDOW_BUDGET: u64 = 64 * 1024 * 1024;

/// Validated process policy. No settings are parsed or resolved on the request path.
#[derive(Clone)]
pub struct DownstreamH2Policy {
    options: H2Options,
}

impl TryFrom<&ServerConfig> for DownstreamH2Policy {
    type Error = anyhow::Error;

    fn try_from(config: &ServerConfig) -> Result<Self> {
        let stream_window = config
            .downstream_h2_stream_window_size
            .unwrap_or(DEFAULT_RECEIVE_WINDOW);
        let connection_window = config
            .downstream_h2_connection_window_size
            .unwrap_or(DEFAULT_RECEIVE_WINDOW);
        let streams = config
            .downstream_h2_max_concurrent_streams
            .unwrap_or(DEFAULT_CONCURRENT_STREAMS);
        for (field, value) in [
            ("downstream_h2_stream_window_size", stream_window),
            ("downstream_h2_connection_window_size", connection_window),
        ] {
            ensure!(
                (DEFAULT_RECEIVE_WINDOW..=MAX_RECEIVE_WINDOW).contains(&value),
                "server.{field} must be 65535..=16777216 bytes"
            );
        }
        ensure!(
            (1..=MAX_CONCURRENT_STREAMS).contains(&streams),
            "server.downstream_h2_max_concurrent_streams must be 1..=1024"
        );
        // This bounds configured per-stream flow-control credit, not total process memory.
        ensure!(
            u64::from(stream_window) * u64::from(streams) <= MAX_STREAM_WINDOW_BUDGET,
            "downstream HTTP/2 stream window times concurrent streams must not exceed 67108864 bytes"
        );

        let mut options = default_h2_options();
        if let Some(value) = config.downstream_h2_stream_window_size {
            options.initial_window_size(value);
        }
        if let Some(value) = config.downstream_h2_connection_window_size {
            options.initial_connection_window_size(value);
        }
        if let Some(value) = config.downstream_h2_max_concurrent_streams {
            options.max_concurrent_streams(value);
        }
        Ok(Self { options })
    }
}

impl DownstreamH2Policy {
    /// Install the immutable policy before a proxy service starts accepting connections.
    pub fn apply_to<SV>(&self, service: &mut Service<HttpProxy<SV>>) -> Result<()>
    where
        SV: ProxyHttp + Send + Sync + 'static,
        SV::CTX: Send + Sync + 'static,
    {
        service
            .app_logic_mut()
            .context("downstream HTTP/2 policy must be installed before service startup")?
            .h2_options = Some(self.options.clone());
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/runtime/server/downstream_h2_test.rs"]
mod tests;
