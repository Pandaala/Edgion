//! Incremental request correlation identifiers. W3C propagation belongs to core.

use async_trait::async_trait;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::plugin_configs::{DefaultIdConfig, RequestIdConfig};

use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::request_trace_id::CompiledRequestIdRule;

pub struct RequestId {
    default_id: Option<DefaultIdConfig>,
    rules: Vec<CompiledRequestIdRule>,
}

impl RequestId {
    pub fn create(config: &RequestIdConfig) -> Option<Box<dyn RequestFilter>> {
        use edgion_resources::resources::edgion_plugins::PluginValidation;
        config.validation_error().map_or(Some(()), |_| None)?;
        let rules = config
            .ids
            .iter()
            .map(CompiledRequestIdRule::new)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        Some(Box::new(Self {
            default_id: config.default_id.clone(),
            rules,
        }))
    }
}

#[async_trait]
impl RequestFilter for RequestId {
    fn name(&self) -> &str {
        "RequestId"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if !session.apply_request_ids(self.default_id.as_ref(), &self.rules) {
            log.push_err("request ID capacity exceeded");
        }
        PluginRunningResult::GoodNext
    }
}
