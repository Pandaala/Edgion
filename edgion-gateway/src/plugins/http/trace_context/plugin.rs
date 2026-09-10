//! TraceContext plugin — W3C trace-context propagation (propagation-only).
//!
//! - `trustInbound` false: discard inbound context; originate a root only when
//!   `generateWhenMissing` is true.
//! - well-formed inbound `traceparent`: pass it through unchanged, align x-trace-id.
//! - absent/malformed: strip partial/garbage headers, generate a root (if enabled).

use async_trait::async_trait;

use super::w3c;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::TraceContextConfig;

const TRACEPARENT: &str = "traceparent";
const TRACESTATE: &str = "tracestate";
const PLUGIN_NAME: &str = "TraceContext";
/// W3C-recommended upper bound on `tracestate`; oversized => dropped.
const MAX_TRACESTATE_BYTES: usize = 512;

fn combined_tracestate_len(value_lengths: impl Iterator<Item = usize>, value_count: usize) -> usize {
    value_lengths.fold(value_count.saturating_sub(1), usize::saturating_add)
}

pub struct TraceContext {
    generate_when_missing: bool,
    echo_to_client: bool,
    trust_inbound: bool,
    default_sampled: bool,
}

impl TraceContext {
    pub fn create(config: &TraceContextConfig) -> Box<dyn RequestFilter> {
        Box::new(TraceContext {
            generate_when_missing: config.generate_when_missing,
            echo_to_client: config.echo_to_client,
            trust_inbound: config.trust_inbound,
            default_sampled: config.default_sampled,
        })
    }

    /// Drop any inbound trace headers, then (if enabled) generate a fresh root,
    /// inject it upstream, align x-trace-id, and optionally echo to the client.
    fn reset_and_maybe_generate(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        let _ = session.remove_request_header(TRACEPARENT);
        let _ = session.remove_request_header(TRACESTATE);
        if !self.generate_when_missing {
            plugin_log.push("no valid traceparent; generation disabled");
            return;
        }
        let root = w3c::generate_root(self.default_sampled);
        let _ = session.set_request_header(TRACEPARENT, root.traceparent());
        let _ = session.set_trace_id(root.trace_id());
        if self.echo_to_client {
            let _ = session.set_response_header(TRACEPARENT, root.traceparent());
        }
        plugin_log.push("generated root traceparent");
    }

    fn enforce_tracestate_limit(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        let Some(values) = session.request_header_values(TRACESTATE) else {
            let _ = session.remove_request_header(TRACESTATE);
            return;
        };
        let combined_len = combined_tracestate_len(values.iter().map(String::len), values.len());
        if combined_len > MAX_TRACESTATE_BYTES {
            let _ = session.remove_request_header(TRACESTATE);
            plugin_log.push("tracestate dropped (oversized)");
        }
    }
}

#[async_trait]
impl RequestFilter for TraceContext {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Edge posture: ignore (untrusted) inbound trace context entirely.
        if !self.trust_inbound {
            self.reset_and_maybe_generate(session, plugin_log);
            return PluginRunningResult::GoodNext;
        }

        let inbound_values = session.request_header_values(TRACEPARENT);
        if let Some([inbound]) = inbound_values.as_deref() {
            if let Some(trace_id) = w3c::parse_trace_id(inbound) {
                // Pass-through: the original traceparent is already on the request.
                let _ = session.set_trace_id(trace_id);
                // Forward tracestate unchanged, but enforce a size cap.
                self.enforce_tracestate_limit(session, plugin_log);
                if self.echo_to_client {
                    let _ = session.set_response_header(TRACEPARENT, inbound);
                }
                plugin_log.push("passthrough traceparent");
                return PluginRunningResult::GoodNext;
            }
        }
        self.reset_and_maybe_generate(session, plugin_log);

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use mockall::Sequence;
    use std::sync::{Arc, Mutex};

    const VALID: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

    fn config() -> TraceContextConfig {
        TraceContextConfig::default()
    }

    async fn assert_reset(values: Option<Vec<String>>) {
        let mut config = config();
        config.generate_when_missing = false;
        let plugin = TraceContext::create(&config);
        let mut session = MockPluginSession::new();
        let mut sequence = Sequence::new();
        session
            .expect_request_header_values()
            .with(mockall::predicate::eq(TRACEPARENT))
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(move |_| values);
        session
            .expect_remove_request_header()
            .with(mockall::predicate::eq(TRACEPARENT))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        session
            .expect_remove_request_header()
            .with(mockall::predicate::eq(TRACESTATE))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        let mut log = PluginLog::new(PLUGIN_NAME);

        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "no valid traceparent; generation disabled");
    }

    #[test]
    fn name_is_static_across_instances() {
        let first = TraceContext::create(&config());
        let second = TraceContext::create(&config());

        assert_eq!(first.name(), PLUGIN_NAME);
        assert_eq!(second.name(), PLUGIN_NAME);
        assert!(std::ptr::eq(first.name(), second.name()));
    }

    #[tokio::test]
    async fn copied_flags_do_not_retain_caller_config() {
        let mut source = config();
        source.trust_inbound = false;
        source.generate_when_missing = false;
        let plugin = TraceContext::create(&source);
        source.trust_inbound = true;
        source.generate_when_missing = true;
        source.echo_to_client = true;
        source.default_sampled = true;
        assert!(source.trust_inbound);
        assert!(source.generate_when_missing);
        assert!(source.echo_to_client);
        assert!(source.default_sampled);

        let mut session = MockPluginSession::new();
        let mut sequence = Sequence::new();
        session
            .expect_remove_request_header()
            .with(mockall::predicate::eq(TRACEPARENT))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        session
            .expect_remove_request_header()
            .with(mockall::predicate::eq(TRACESTATE))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        let mut log = PluginLog::new(PLUGIN_NAME);

        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "no valid traceparent; generation disabled");
    }

    #[tokio::test]
    async fn one_valid_traceparent_is_passed_through_and_echoed() {
        let mut config = config();
        config.echo_to_client = true;
        let plugin = TraceContext::create(&config);
        let mut session = MockPluginSession::new();
        let mut sequence = Sequence::new();
        session
            .expect_request_header_values()
            .with(mockall::predicate::eq(TRACEPARENT))
            .times(1)
            .in_sequence(&mut sequence)
            .return_const(Some(vec![VALID.to_string()]));
        session
            .expect_set_trace_id()
            .with(mockall::predicate::eq(TRACE_ID))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        session
            .expect_request_header_values()
            .with(mockall::predicate::eq(TRACESTATE))
            .times(1)
            .in_sequence(&mut sequence)
            .return_const(Some(Vec::new()));
        session
            .expect_set_response_header()
            .withf(|name, value| name == TRACEPARENT && value == VALID)
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        let mut log = PluginLog::new(PLUGIN_NAME);

        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "passthrough traceparent");
    }

    #[tokio::test]
    async fn absent_repeated_invalid_byte_and_malformed_traceparent_reset() {
        assert_reset(Some(Vec::new())).await;
        assert_reset(Some(vec![VALID.to_string(), VALID.to_string()])).await;
        assert_reset(None).await;
        assert_reset(Some(vec![format!(" {VALID}")])).await;
    }

    #[tokio::test]
    async fn repeated_traceparent_is_removed_before_one_sampled_root_is_generated_and_echoed() {
        let mut config = config();
        config.default_sampled = true;
        config.echo_to_client = true;
        let plugin = TraceContext::create(&config);
        let generated = Arc::new(Mutex::new(None::<String>));
        let mut session = MockPluginSession::new();
        let mut sequence = Sequence::new();
        session
            .expect_request_header_values()
            .times(1)
            .in_sequence(&mut sequence)
            .return_const(Some(vec![VALID.to_string(), VALID.to_string()]));
        session
            .expect_remove_request_header()
            .with(mockall::predicate::eq(TRACEPARENT))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        session
            .expect_remove_request_header()
            .with(mockall::predicate::eq(TRACESTATE))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        let generated_for_header = Arc::clone(&generated);
        session
            .expect_set_request_header()
            .withf(|name, _| name == TRACEPARENT)
            .times(1)
            .in_sequence(&mut sequence)
            .returning(move |_, value| {
                assert_eq!(value.len(), 55);
                assert!(value.ends_with("-01"));
                assert!(w3c::parse_trace_id(value).is_some());
                *generated_for_header.lock().unwrap() = Some(value.to_string());
                Ok(())
            });
        let generated_for_trace_id = Arc::clone(&generated);
        session
            .expect_set_trace_id()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(move |trace_id| {
                let generated = generated_for_trace_id.lock().unwrap();
                assert_eq!(Some(trace_id), generated.as_deref().map(|value| &value[3..35]));
                Ok(())
            });
        let generated_for_echo = Arc::clone(&generated);
        session
            .expect_set_response_header()
            .withf(|name, _| name == TRACEPARENT)
            .times(1)
            .in_sequence(&mut sequence)
            .returning(move |_, value| {
                assert_eq!(Some(value), generated_for_echo.lock().unwrap().as_deref());
                Ok(())
            });
        let mut log = PluginLog::new(PLUGIN_NAME);

        let result = plugin.run_request(&mut session, &mut log).await;

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "generated root traceparent");
    }

    async fn run_tracestate_case(values: Option<Vec<String>>, remove: bool) -> String {
        let plugin = TraceContext::create(&config());
        let mut session = MockPluginSession::new();
        let mut sequence = Sequence::new();
        session
            .expect_request_header_values()
            .with(mockall::predicate::eq(TRACEPARENT))
            .times(1)
            .in_sequence(&mut sequence)
            .return_const(Some(vec![VALID.to_string()]));
        session
            .expect_set_trace_id()
            .with(mockall::predicate::eq(TRACE_ID))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        session
            .expect_request_header_values()
            .with(mockall::predicate::eq(TRACESTATE))
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(move |_| values);
        if remove {
            session
                .expect_remove_request_header()
                .with(mockall::predicate::eq(TRACESTATE))
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|_| Ok(()));
        }
        let mut log = PluginLog::new(PLUGIN_NAME);

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        log.render_for_test()
    }

    #[tokio::test]
    async fn tracestate_combined_boundaries_preserve_or_remove_all_lines() {
        let at_limit = run_tracestate_case(Some(vec!["a".repeat(255), "b".repeat(256)]), false).await;
        assert_eq!(at_limit, "passthrough traceparent");

        let over_limit = run_tracestate_case(Some(vec!["a".repeat(256), "b".repeat(256)]), true).await;
        assert_eq!(over_limit, "tracestate dropped (oversized)\npassthrough traceparent");
    }

    #[tokio::test]
    async fn invalid_utf8_tracestate_is_removed_without_extra_log() {
        let log = run_tracestate_case(None, true).await;
        assert_eq!(log, "passthrough traceparent");
    }

    #[test]
    fn tracestate_length_arithmetic_saturates() {
        assert_eq!(combined_tracestate_len([255, 256].into_iter(), 2), 512);
        assert_eq!(combined_tracestate_len([usize::MAX, 1].into_iter(), 2), usize::MAX);
    }
}
