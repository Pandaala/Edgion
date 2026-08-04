use async_trait::async_trait;

use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use crate::{ExternalJumpPreset, SetJumpOutcome, UpstreamJumpTarget};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{
    DynamicExternalUpstreamConfig, ExtUpstreamOnMissing, ExtUpstreamOnNoMatch,
};

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
                        session.record_outcome(DyeOutcome::Fallback, "on-missing");
                        log.push("upstream miss");
                        PluginRunningResult::GoodNext
                    }
                    ExtUpstreamOnMissing::Reject => {
                        session.record_outcome(DyeOutcome::Reject, "on-missing");
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
                                session.record_outcome(DyeOutcome::Fallback, "on-missing");
                                log.push("upstream miss");
                                PluginRunningResult::GoodNext
                            }
                            ExtUpstreamOnMissing::Reject => {
                                session.record_outcome(DyeOutcome::Reject, "on-missing");
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
                        session.record_outcome(DyeOutcome::Fallback, "on-no-match");
                        log.push("upstream miss");
                        PluginRunningResult::GoodNext
                    }
                    ExtUpstreamOnNoMatch::Reject => {
                        session.record_outcome(DyeOutcome::Reject, "on-no-match");
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
            upstream_read_timeout_override: None,
        };
        let resolved = format!("{}:{}", target.domain, target.effective_port());
        match session.set_upstream_jump(UpstreamJumpTarget::External(info)) {
            SetJumpOutcome::Set => {
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
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::EdgionHttpContext;
    use edgion_resources::common::KeyGet;
    use edgion_resources::resources::edgion_plugins::plugin_configs::DomainTarget;
    use std::collections::HashMap;

    fn base_config() -> DynamicExternalUpstreamConfig {
        DynamicExternalUpstreamConfig {
            from: KeyGet::Header {
                name: "X-Target-Region".to_string(),
            },
            extract: None,
            domain_map: HashMap::from([(
                "us".to_string(),
                DomainTarget {
                    domain: "api-us.example.com".to_string(),
                    port: Some(8443),
                    tls: true,
                    override_host: None,
                    sni: None,
                },
            )]),
            on_missing: ExtUpstreamOnMissing::default(),
            on_no_match: ExtUpstreamOnNoMatch::default(),
            compiled_regex: None,
        }
    }

    /// A successful selection (`SetJumpOutcome::Set`) must record `Success`
    /// with the resolved `"domain:port"` value — the value the deleted
    /// `apply_dye_headers` call used to pass.
    #[tokio::test]
    async fn deu_records_success_outcome_with_domain_port() {
        let plugin = DynamicExternalUpstream {
            name: "DynamicExternalUpstream".to_string(),
            config: base_config(),
        };

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session.expect_key_get().return_const(Some("us".to_string()));
        session
            .expect_set_upstream_jump()
            .withf(|target| match target {
                UpstreamJumpTarget::External(jump) => jump.upstream_read_timeout_override.is_none(),
                _ => false,
            })
            .return_const(SetJumpOutcome::Set);
        session
            .expect_record_outcome()
            .withf(|o, v| *o == DyeOutcome::Success && v == "api-us.example.com:8443")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DynamicExternalUpstream");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    /// Missing routing key with `on_missing: Reject` must record `Reject` with
    /// cause `on-missing` and return the 400 local reply.
    #[tokio::test]
    async fn deu_records_reject_outcome_on_missing_key() {
        let config = DynamicExternalUpstreamConfig {
            on_missing: ExtUpstreamOnMissing::Reject,
            ..base_config()
        };
        let plugin = DynamicExternalUpstream {
            name: "DynamicExternalUpstream".to_string(),
            config,
        };

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session.expect_key_get().return_const(None::<String>);
        session
            .expect_record_outcome()
            .withf(|o, v| *o == DyeOutcome::Reject && v == "on-missing")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DynamicExternalUpstream");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 400,
                body: Some("Missing routing key for external upstream".to_string()),
            }
        );
    }

    /// `on_no_match: Skip` (default) on a routing key with no domain entry must
    /// record `Fallback` with cause `on-no-match` and pass the request through.
    #[tokio::test]
    async fn deu_records_fallback_outcome_on_no_match() {
        let plugin = DynamicExternalUpstream {
            name: "DynamicExternalUpstream".to_string(),
            config: base_config(),
        };

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session
            .expect_key_get()
            .return_const(Some("unknown-region".to_string()));
        session
            .expect_record_outcome()
            .withf(|o, v| *o == DyeOutcome::Fallback && v == "on-no-match")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DynamicExternalUpstream");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }
}
