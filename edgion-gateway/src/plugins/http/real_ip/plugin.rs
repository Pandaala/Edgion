//! Real IP plugin implementation
//!
//! Extracts the real client IP address from HTTP headers when behind trusted proxies.

use async_trait::async_trait;

use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, PluginSessionHeaderExt, RequestFilter};
use crate::util::RealIpExtractor;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::RealIpConfig;

/// RealIp plugin: extracts real client IP using a per-instance RealIpExtractor.
pub struct RealIp {
    extractor: Option<RealIpExtractor>,
    header_log: &'static str,
}

impl RealIp {
    fn header_log(real_ip_header: &str) -> &'static str {
        match real_ip_header.to_ascii_lowercase().as_str() {
            "x-forwarded-for" => "header xff",
            "x-real-ip" => "header real-ip",
            "forwarded" => "header forwarded",
            _ => "header custom",
        }
    }

    /// Create a new RealIp plugin from configuration
    pub fn create(config: &RealIpConfig) -> Box<dyn RequestFilter> {
        // Build RealIpExtractor from plugin config
        let extractor = if !config.trusted_ips.is_empty() {
            match RealIpExtractor::new(
                &config.trusted_ips,
                config.real_ip_header.clone(),
                config.recursive,
                config.max_trusted_hops,
            ) {
                Ok(e) => Some(e),
                Err(err) => {
                    tracing::error!(
                        error = ?err,
                        "Failed to create RealIpExtractor from plugin config"
                    );
                    None
                }
            }
        } else {
            tracing::warn!("RealIp plugin: trusted_ips is empty, plugin will be no-op");
            None
        };

        let plugin = RealIp {
            extractor,
            header_log: Self::header_log(&config.real_ip_header),
        };

        Box::new(plugin)
    }
}

#[async_trait]
impl RequestFilter for RealIp {
    fn name(&self) -> &str {
        "RealIp"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Check if extractor is available
        let Some(ref extractor) = self.extractor else {
            plugin_log.push_err("no extractor");
            return PluginRunningResult::GoodNext;
        };

        plugin_log.push(self.header_log);

        let header_value = session.header_str(extractor.real_ip_header());
        let resolution = extractor.resolve_real_ip(session.downstream_peer_ip(), header_value);

        if resolution.downstream_trusted {
            plugin_log.push("trusted proxy:match");
        } else {
            plugin_log.push("trusted proxy:miss");
        }

        let remote_ip_changed = session.remote_ip() != resolution.remote_ip.as_str();
        if remote_ip_changed {
            if let Err(_e) = session.set_remote_ip(&resolution.remote_ip) {
                plugin_log.push_err("set remote IP failed");
                return PluginRunningResult::GoodNext;
            }

            plugin_log.push_fmt("ip updated: {}", la![sensitive(&resolution.remote_ip)]);
        } else {
            plugin_log.push("ip unchanged");
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/real_ip/plugin_test.rs"]
mod tests;
