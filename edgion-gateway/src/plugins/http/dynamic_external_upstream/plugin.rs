use async_trait::async_trait;

use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use crate::{ExternalJumpPreset, SetJumpOutcome, UpstreamJumpTarget};
use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{DynamicExternalUpstreamConfig, UpstreamFallback};

pub struct DynamicExternalUpstream {
    config: DynamicExternalUpstreamConfig,
}

impl DynamicExternalUpstream {
    pub fn create(config: &DynamicExternalUpstreamConfig) -> Box<dyn RequestFilter> {
        let mut validated = config.clone();
        validated.validate();
        Box::new(Self { config: validated })
    }
}

#[async_trait]
impl RequestFilter for DynamicExternalUpstream {
    fn name(&self) -> &str {
        "DynamicExternalUpstream"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // 0. Only supported for HTTP routes, skip for gRPC routes
        if session.ctx().is_grpc_route_matched {
            log.push("skip grpc");
            return PluginRunningResult::GoodNext;
        }

        // 1. Extract raw routing key via KeyGet
        let raw_value = match session.key_get(&self.config.from) {
            Some(v) if !v.is_empty() => v,
            _ => {
                return match self.config.on_missing {
                    UpstreamFallback::Fallback => {
                        session.record_outcome(DyeOutcome::Fallback, "on-missing");
                        log.push("upstream miss");
                        PluginRunningResult::GoodNext
                    }
                    UpstreamFallback::Reject => {
                        session.record_outcome(DyeOutcome::Reject, "on-missing");
                        log.push_err("no routing key");
                        PluginRunningResult::ErrResponse {
                            status: 400,
                            body: Some(Bytes::from_static(b"Missing routing key for external upstream")),
                        }
                    }
                };
            }
        };

        // 2. Apply regex extraction if configured
        let routing_key = if let Some(ref extract) = self.config.extract {
            if let Some(ref regex) = self.config.compiled_regex {
                match regex.captures(&raw_value).and_then(|c| c.get(extract.group)) {
                    Some(m) => m.as_str(),
                    None => {
                        return match self.config.on_missing {
                            UpstreamFallback::Fallback => {
                                session.record_outcome(DyeOutcome::Fallback, "on-missing");
                                log.push("upstream miss");
                                PluginRunningResult::GoodNext
                            }
                            UpstreamFallback::Reject => {
                                session.record_outcome(DyeOutcome::Reject, "on-missing");
                                log.push_err("regex extract fail");
                                PluginRunningResult::ErrResponse {
                                    status: 400,
                                    body: Some(Bytes::from_static(b"Routing key regex extraction failed")),
                                }
                            }
                        };
                    }
                }
            } else {
                raw_value.as_str()
            }
        } else {
            raw_value.as_str()
        };

        // 3. Look up domain in domainMap
        let Some(target) = self.config.lookup_domain(routing_key) else {
            return match self.config.on_no_match {
                UpstreamFallback::Fallback => {
                    session.record_outcome(DyeOutcome::Fallback, "on-no-match");
                    log.push("upstream miss");
                    PluginRunningResult::GoodNext
                }
                UpstreamFallback::Reject => {
                    session.record_outcome(DyeOutcome::Reject, "on-no-match");
                    log.push_err("no upstream available");
                    PluginRunningResult::ErrResponse {
                        status: 400,
                        body: Some(Bytes::from_static(b"No matching domain for routing key")),
                    }
                }
            };
        };

        // 4. Build ExternalJumpPreset and acquire the unified jump slot.
        let info = ExternalJumpPreset {
            domain: target.domain.clone(),
            port: target.effective_port(),
            use_tls: target.tls,
            sni: target.effective_sni().to_string(),
            // Regular external server (not another gateway): let the global
            // `forward_normalized_path` / per-route annotation policy decide.
            preserve_raw_path: false,
            upstream_read_timeout_override: None,
        };
        match session.set_upstream_jump(UpstreamJumpTarget::External(info)) {
            SetJumpOutcome::Set => {
                // Apply the Host override only after this plugin owns the jump.
                // The request-stage mutation persists across retries.
                if let Some(ref host) = target.override_host {
                    let _ = session.set_upstream_host(host);
                }
                let resolved = format!("{}:{}", target.domain, target.effective_port());
                session.record_outcome(DyeOutcome::Success, &resolved);
                log.push("upstream selected");
            }
            SetJumpOutcome::Rejected => {
                log.push("jump locked, rejected");
                return PluginRunningResult::GoodNext;
            }
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/dynamic_external_upstream/plugin_test.rs"]
mod tests;
