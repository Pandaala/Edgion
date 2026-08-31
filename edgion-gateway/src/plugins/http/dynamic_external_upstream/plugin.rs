use async_trait::async_trait;

use crate::plugins::runtime::{DyeOutcome, PluginLog, PluginSession, RequestFilter};
use crate::{ExternalJumpPreset, SetJumpOutcome, UpstreamJumpTarget};
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
                                    body: Some("Routing key regex extraction failed".to_string()),
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
        let target = match self.config.lookup_domain(&routing_key) {
            Some(t) => t,
            None => {
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
                            body: Some("No matching domain for routing key".to_string()),
                        }
                    }
                };
            }
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
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::EdgionHttpContext;
    use edgion_resources::common::KeyGet;
    use edgion_resources::resources::edgion_plugins::plugin_configs::{DomainTarget, ExtUpstreamExtract};
    use mockall::Sequence;
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
            on_missing: UpstreamFallback::Fallback,
            on_no_match: UpstreamFallback::Fallback,
            compiled_regex: None,
        }
    }

    #[test]
    fn dynamic_external_upstream_name_is_static_across_instances() {
        let first = DynamicExternalUpstream::create(&base_config());
        let mut second_config = base_config();
        second_config.domain_map.insert(
            "eu".to_string(),
            DomainTarget {
                domain: "api-eu.example.com".to_string(),
                port: None,
                tls: false,
                override_host: None,
                sni: None,
            },
        );
        let second = DynamicExternalUpstream::create(&second_config);

        assert_eq!(first.name(), "DynamicExternalUpstream");
        assert_eq!(second.name(), "DynamicExternalUpstream");
    }

    #[tokio::test]
    async fn regex_capture_selects_matching_domain_without_copying_the_key() {
        let config = DynamicExternalUpstreamConfig {
            extract: Some(ExtUpstreamExtract {
                regex: r"^region=(.+)$".to_string(),
                group: 1,
            }),
            ..base_config()
        };
        let plugin = DynamicExternalUpstream::create(&config);

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session.expect_key_get().return_const(Some("region=us".to_string()));
        session
            .expect_set_upstream_jump()
            .withf(|target| {
                matches!(
                    target,
                    UpstreamJumpTarget::External(jump)
                        if jump.domain == "api-us.example.com"
                            && jump.port == 8443
                            && jump.use_tls
                            && jump.sni == "api-us.example.com"
                            && !jump.preserve_raw_path
                            && jump.upstream_read_timeout_override.is_none()
                )
            })
            .times(1)
            .return_const(SetJumpOutcome::Set);
        session
            .expect_record_outcome()
            .withf(|outcome, value| *outcome == DyeOutcome::Success && value == "api-us.example.com:8443")
            .times(1)
            .return_const(());

        let mut log = PluginLog::new("DynamicExternalUpstream");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    /// A successful selection (`SetJumpOutcome::Set`) must record `Success`
    /// with the resolved `"domain:port"` value — the value the deleted
    /// `apply_dye_headers` call used to pass.
    #[tokio::test]
    async fn deu_records_success_outcome_with_domain_port() {
        let mut config = base_config();
        config.domain_map.get_mut("us").unwrap().override_host = Some("origin.example.com".to_string());
        let plugin = DynamicExternalUpstream::create(&config);

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session.expect_key_get().return_const(Some("us".to_string()));
        let mut sequence = Sequence::new();
        session
            .expect_set_upstream_jump()
            .withf(|target| {
                matches!(
                    target,
                    UpstreamJumpTarget::External(jump)
                        if jump.domain == "api-us.example.com"
                            && jump.port == 8443
                            && jump.use_tls
                            && jump.sni == "api-us.example.com"
                            && !jump.preserve_raw_path
                            && jump.upstream_read_timeout_override.is_none()
                )
            })
            .times(1)
            .in_sequence(&mut sequence)
            .return_const(SetJumpOutcome::Set);
        session
            .expect_set_upstream_host()
            .withf(|host| host == "origin.example.com")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        session
            .expect_record_outcome()
            .withf(|o, v| *o == DyeOutcome::Success && v == "api-us.example.com:8443")
            .times(1)
            .in_sequence(&mut sequence)
            .return_const(());

        let mut log = PluginLog::new("DynamicExternalUpstream");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("upstream selected"));
    }

    #[tokio::test]
    async fn rejected_jump_has_no_host_or_dye_side_effects() {
        let mut config = base_config();
        config.domain_map.get_mut("us").unwrap().override_host = Some("origin.example.com".to_string());
        let plugin = DynamicExternalUpstream::create(&config);

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session.expect_key_get().return_const(Some("us".to_string()));
        session
            .expect_set_upstream_jump()
            .withf(|target| matches!(target, UpstreamJumpTarget::External(jump) if jump.domain == "api-us.example.com"))
            .times(1)
            .return_const(SetJumpOutcome::Rejected);
        // No Host or Dye expectations: either side effect fails the mock.

        let mut log = PluginLog::new("DynamicExternalUpstream");
        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert_eq!(
            serde_json::to_value(&log).unwrap()["log"],
            serde_json::json!(["jump locked, rejected"])
        );
    }

    /// Missing routing key with `on_missing: Reject` must record `Reject` with
    /// cause `on-missing` and return the 400 local reply.
    #[tokio::test]
    async fn deu_records_reject_outcome_on_missing_key() {
        let config = DynamicExternalUpstreamConfig {
            on_missing: UpstreamFallback::Reject,
            ..base_config()
        };
        let plugin = DynamicExternalUpstream::create(&config);

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

    /// `on_no_match: Fallback` (default) on a routing key with no domain entry must
    /// record `Fallback` with cause `on-no-match` and pass the request through.
    #[tokio::test]
    async fn deu_records_fallback_outcome_on_no_match() {
        let plugin = DynamicExternalUpstream::create(&base_config());

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
