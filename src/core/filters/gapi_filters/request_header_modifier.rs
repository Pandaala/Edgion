//! RequestHeaderModifier filter implementation
//! 
//! This filter modifies request headers based on HTTPHeaderFilter configuration.

use async_trait::async_trait;
use crate::types::resources::HTTPHeaderFilter;
use crate::types::filters::{FilterConf, FilterRunningResult, FilterRunningStage};
use crate::core::filters::plugin_runtime::traits::{Filter, FilterSession};
use crate::core::filters::plugin_runtime::filter_log::FilterLog;

/// Filter that modifies request headers
pub struct RequestHeaderModifierFilter {
    config: HTTPHeaderFilter,
}

impl RequestHeaderModifierFilter {
    pub fn new(config: HTTPHeaderFilter) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Filter for RequestHeaderModifierFilter {
    fn name(&self) -> &str {
        "RequestHeaderModifier"
    }

    fn run_sync(
        &self,
        _stage: FilterRunningStage,
        session: &mut dyn FilterSession,
        _log: &mut FilterLog,
    ) -> FilterRunningResult {
        self.modify_headers(session)
    }

    async fn run_async(
        &self,
        _stage: FilterRunningStage,
        session: &mut dyn FilterSession,
        _log: &mut FilterLog,
    ) -> FilterRunningResult {
        self.modify_headers(session)
    }

    fn supports_sync(&self) -> bool {
        true
    }

    fn get_stages(&self) -> Vec<FilterRunningStage> {
        vec![FilterRunningStage::Request]
    }

    fn check_schema(&self, _conf: &FilterConf) {}
}

impl RequestHeaderModifierFilter {
    fn modify_headers(&self, session: &mut dyn FilterSession) -> FilterRunningResult {
        // Set headers - overwrite existing
        if let Some(headers) = &self.config.set {
            for h in headers {
                let _ = session.set_request_header(&h.name, &h.value);
            }
        }
        // Add headers - append to existing
        if let Some(headers) = &self.config.add {
            for h in headers {
                let _ = session.append_request_header(&h.name, &h.value);
            }
        }
        // Remove headers
        if let Some(names) = &self.config.remove {
            for name in names {
                let _ = session.remove_request_header(name);
            }
        }
        FilterRunningResult::GoodNext
    }
}

