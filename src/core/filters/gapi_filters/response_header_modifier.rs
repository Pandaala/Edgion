//! ResponseHeaderModifier filter implementation

use async_trait::async_trait;
use crate::types::resources::HTTPHeaderFilter;
use crate::types::filters::{FilterConf, FilterRunningResult, FilterRunningStage};
use crate::core::filters::traits::{Filter, FilterSession};
use crate::core::filters::filter_log::FilterLog;

pub struct ResponseHeaderModifierFilter {
    config: HTTPHeaderFilter,
}

impl ResponseHeaderModifierFilter {
    pub fn new(config: HTTPHeaderFilter) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Filter for ResponseHeaderModifierFilter {
    fn name(&self) -> &str {
        "ResponseHeaderModifier"
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
        vec![FilterRunningStage::UpstreamResponseFilter]
    }

    fn check_schema(&self, _conf: &FilterConf) {}
}

impl ResponseHeaderModifierFilter {
    fn modify_headers(&self, session: &mut dyn FilterSession) -> FilterRunningResult {
        // Set headers
        if let Some(headers) = &self.config.set {
            for h in headers {
                let _ = session.set_response_header(&h.name, &h.value);
            }
        }
        // Add headers
        if let Some(headers) = &self.config.add {
            for h in headers {
                let _ = session.append_response_header(&h.name, &h.value);
            }
        }
        // Remove headers - TODO: need remove_response_header in FilterSession
        FilterRunningResult::GoodNext
    }
}

