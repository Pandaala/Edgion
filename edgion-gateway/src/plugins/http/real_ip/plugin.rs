//! Real IP plugin implementation
//!
//! Extracts the real client IP address from HTTP headers when behind trusted proxies.

use async_trait::async_trait;

use crate::plugins::runtime::conditions::ip_refs::{BoundIpPredicate, IpConditionCompiler};
use crate::plugins::runtime::edgion_config_data::EdgionConfigDataSnapshot;
use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::util::real_ip_extractor::RealIpResolution;
use crate::util::RealIpExtractor;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::PluginValidation;
use edgion_resources::resources::edgion_plugins::RealIpConfig;
use std::sync::Arc;

/// One validated policy and its actual-owner-bound trust references.
pub struct RealIpPolicy {
    extractor: RealIpExtractor,
    trusted: Arc<BoundIpPredicate>,
}
impl RealIpPolicy {
    pub(crate) fn bind(config: &RealIpConfig, compiler: &IpConditionCompiler<'_>) -> Result<Self, String> {
        if let Some(error) = config.validation_error() {
            return Err(error);
        }
        let trusted = compiler
            .bind(&config.trusted)
            .ok_or_else(|| "invalid owning RealIp trust references".to_string())?;
        let header = http::HeaderName::from_bytes(config.real_ip_header.as_bytes())
            .map_err(|_| "'real_ip_header' must be a valid HTTP header name".to_string())?;
        Ok(Self {
            extractor: RealIpExtractor::new(header, config.recursive, config.max_trusted_hops),
            trusted,
        })
    }
    pub(crate) fn real_ip_header(&self) -> &http::HeaderName {
        self.extractor.real_ip_header()
    }
    pub(crate) fn resolve_session(&self, session: &dyn PluginSession, header_value: Option<&str>) -> RealIpResolution {
        let available = self.trusted.available(session);
        self.extractor
            .resolve_real_ip(session.downstream_peer_ip(), header_value, |ip| {
                available && self.trusted.contains(session, *ip)
            })
    }
    pub(crate) fn resolve_in(
        &self,
        view: &EdgionConfigDataSnapshot,
        peer: &str,
        header_value: Option<&str>,
    ) -> RealIpResolution {
        let available = self.trusted.available_in(view);
        self.extractor.resolve_real_ip(peer, header_value, |ip| {
            available && self.trusted.contains_in(view, *ip)
        })
    }
}

pub struct RealIp {
    policy: RealIpPolicy,
    header_log: &'static str,
}

impl RealIp {
    fn header_log(real_ip_header: &str) -> &'static str {
        if real_ip_header.eq_ignore_ascii_case("x-forwarded-for") {
            "header xff"
        } else if real_ip_header.eq_ignore_ascii_case("x-real-ip") {
            "header real-ip"
        } else if real_ip_header.eq_ignore_ascii_case("forwarded") {
            "header forwarded"
        } else {
            "header custom"
        }
    }

    pub(crate) fn create(
        config: &RealIpConfig,
        compiler: &IpConditionCompiler<'_>,
    ) -> Result<Box<dyn RequestFilter>, String> {
        let policy = RealIpPolicy::bind(config, compiler)?;
        Ok(Box::new(Self {
            policy,
            header_log: Self::header_log(&config.real_ip_header),
        }))
    }
}

#[async_trait]
impl RequestFilter for RealIp {
    fn name(&self) -> &str {
        "RealIp"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        session.ensure_config_data_view();
        plugin_log.push(self.header_log);
        let header_value = session
            .request_header_map()
            .get(self.policy.real_ip_header())
            .and_then(|value| value.to_str().ok());
        let resolution = self.policy.resolve_session(session, header_value);

        if resolution.downstream_trusted {
            plugin_log.push("trusted proxy:match");
        } else {
            plugin_log.push("trusted proxy:miss");
        }

        let remote_ip = resolution.remote_ip.to_string();
        let remote_ip_changed = session.remote_ip() != remote_ip.as_str();
        if remote_ip_changed {
            if let Err(_e) = session.set_remote_ip(&remote_ip) {
                plugin_log.push_err("set remote IP failed");
                return PluginRunningResult::GoodNext;
            }

            plugin_log.push_fmt("ip updated: {}", la![sensitive(&remote_ip)]);
        } else {
            plugin_log.push("ip unchanged");
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/real_ip/plugin_test.rs"]
mod tests;
