use async_trait::async_trait;

use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::{
    DynamicExternalUpstreamConfig, ExtUpstreamOnMissing, ExtUpstreamOnNoMatch,
};
use crate::types::{ExternalJumpPreset, SetJumpOutcome, UpstreamJumpTarget};

pub struct DynamicExternalUpstream {
    name: String,
    config: DynamicExternalUpstreamConfig,
}

impl DynamicExternalUpstream {
    pub fn create(config: &DynamicExternalUpstreamConfig) -> Box<dyn RequestFilter> {
        let mut validated = config.clone();
        validated.validate();
        Box::new(Self {
            name: "DynamicExternalUpstream".to_string(),
            config: validated,
        })
    }
}

#[async_trait]
impl RequestFilter for DynamicExternalUpstream {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // 0. Only supported for HTTP routes, skip for gRPC routes
        if session.ctx().is_grpc_route_matched {
            log.push("skip grpc");
            return PluginRunningResult::GoodNext;
        }

        // 1. Extract raw routing key via KeyGet
        let raw_value = match session.key_get(&self.config.from).await {
            Some(v) if !v.is_empty() => v,
            _ => {
                return match self.config.on_missing {
                    ExtUpstreamOnMissing::Skip => {
                        log.push("upstream miss");
                        PluginRunningResult::GoodNext
                    }
                    ExtUpstreamOnMissing::Reject => {
                        log.push_err("no routing key");
                        PluginRunningResult::ErrResponse {
                            status: 400,
                            body: Some("Missing routing key for external upstream".to_string()),
                        }
                    }
                };
            }
        };

        // 2. Apply regex extraction if configured
        let routing_key = if let Some(ref extract) = self.config.extract {
            if let Some(ref regex) = self.config.compiled_regex {
                match regex.captures(&raw_value).and_then(|c| c.get(extract.group)) {
                    Some(m) => m.as_str().to_string(),
                    None => {
                        return match self.config.on_missing {
                            ExtUpstreamOnMissing::Skip => {
                                log.push("upstream miss");
                                PluginRunningResult::GoodNext
                            }
                            ExtUpstreamOnMissing::Reject => {
                                log.push_err("regex extract fail");
                                PluginRunningResult::ErrResponse {
                                    status: 400,
                                    body: Some("Routing key regex extraction failed".to_string()),
                                }
                            }
                        };
                    }
                }
            } else {
                raw_value
            }
        } else {
            raw_value
        };

        // 3. Look up domain in domainMap
        let target = match self.config.lookup_domain(&routing_key) {
            Some(t) => t,
            None => {
                return match self.config.on_no_match {
                    ExtUpstreamOnNoMatch::Skip => {
                        log.push("upstream miss");
                        PluginRunningResult::GoodNext
                    }
                    ExtUpstreamOnNoMatch::Reject => {
                        log.push_err("no upstream available");
                        PluginRunningResult::ErrResponse {
                            status: 400,
                            body: Some("No matching domain for routing key".to_string()),
                        }
                    }
                };
            }
        };

        // 4. Apply Host header override if configured
        //    This is done in request_filter stage via set_upstream_host(),
        //    so it persists across retries.
        if let Some(ref host) = target.override_host {
            let _ = session.set_upstream_host(host);
        }

        // 5. Build ExternalJumpPreset and set in context
        let info = ExternalJumpPreset {
            domain: target.domain.clone(),
            port: target.effective_port(),
            use_tls: target.tls,
            sni: target.effective_sni().to_string(),
            // Regular external server (not another gateway): let the global
            // `forward_normalized_path` / per-route annotation policy decide.
            preserve_raw_path: false,
        };
        match session.set_upstream_jump(UpstreamJumpTarget::External(info)) {
            SetJumpOutcome::Set => {
                log.push("upstream selected");
            }
            SetJumpOutcome::Rejected => {
                log.push("jump locked, rejected");
                return PluginRunningResult::GoodNext;
            }
        }

        // 6. Optionally set debug request header (sent to upstream for tracing)
        if self.config.debug_header {
            let header_value = format!("{}:{}", target.domain, target.effective_port());
            let _ = session.set_request_header("X-Dynamic-External-Upstream", &header_value);
        }

        PluginRunningResult::GoodNext
    }
}
