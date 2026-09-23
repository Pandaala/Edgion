//! ResponseHeaderModifier filter implementation

use super::header_modifier::HeaderModifierPlan;
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{PluginSession, UpstreamResponseFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::{GRPCHeaderFilter, HTTPHeaderFilter};

pub struct ResponseHeaderModifierFilter {
    plan: HeaderModifierPlan,
}

impl ResponseHeaderModifierFilter {
    pub fn new(config: HTTPHeaderFilter) -> Result<Self, &'static str> {
        config.validate_response()?;
        Ok(Self {
            plan: HeaderModifierPlan::from_http(config)?,
        })
    }

    /// Create from GRPCHeaderFilter (which has the same structure as HTTPHeaderFilter)
    pub fn new_from_grpc(config: GRPCHeaderFilter) -> Result<Self, &'static str> {
        config.validate_response()?;
        Ok(Self {
            plan: HeaderModifierPlan::from_grpc(config)?,
        })
    }
}

impl UpstreamResponseFilter for ResponseHeaderModifierFilter {
    fn name(&self) -> &str {
        "ResponseHeaderModifier"
    }

    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        self.modify_headers(session, log)
    }
}

impl ResponseHeaderModifierFilter {
    fn modify_headers(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // Set headers
        for header in self.plan.set() {
            if session
                .set_response_header_value(header.name(), header.value().clone())
                .is_err()
            {
                log.push_err("set resp hdr fail");
            }
        }
        // Add headers
        for header in self.plan.add() {
            if session
                .append_response_header_value(header.name(), header.value().clone())
                .is_err()
            {
                log.push_err("add resp hdr fail");
            }
        }
        // Remove headers
        for name in self.plan.remove() {
            if session.remove_response_header(name).is_err() {
                log.push_err("remove resp hdr fail");
            }
        }
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::HTTPHeader;
    use mockall::Sequence;
    use std::sync::Arc;

    use parking_lot::Mutex;

    fn header(name: &str, value: &str) -> HTTPHeader {
        HTTPHeader {
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    fn http_filter(
        set: Option<Vec<HTTPHeader>>,
        add: Option<Vec<HTTPHeader>>,
        remove: Option<Vec<String>>,
    ) -> HTTPHeaderFilter {
        HTTPHeaderFilter { set, add, remove }
    }

    fn grpc_filter(
        set: Option<Vec<HTTPHeader>>,
        add: Option<Vec<HTTPHeader>>,
        remove: Option<Vec<String>>,
    ) -> GRPCHeaderFilter {
        GRPCHeaderFilter { set, add, remove }
    }

    fn assert_add_appends_without_replacing(filter: ResponseHeaderModifierFilter) {
        let values = Arc::new(Mutex::new(vec!["upstream-a".to_string(), "upstream-b".to_string()]));
        let appended = Arc::clone(&values);
        let mut session = MockPluginSession::new();
        session.expect_set_response_header_value().never();
        session.expect_remove_response_header().never();
        session
            .expect_append_response_header_value()
            .withf(|name, value| name == "Vary" && value == "Origin")
            .times(1)
            .returning(move |_, value| {
                appended
                    .lock()
                    .push(value.to_str().expect("configured value remains text").to_string());
                Ok(())
            });
        let mut log = PluginLog::new("ResponseHeaderModifier");

        assert_eq!(
            filter.modify_headers(&mut session, &mut log),
            PluginRunningResult::GoodNext
        );
        assert_eq!(&*values.lock(), &["upstream-a", "upstream-b", "Origin"]);
        assert_eq!(log.render_for_test(), "");
    }

    #[test]
    fn http_add_appends_to_preexisting_field_lines_and_ignores_equivalent_duplicate() {
        assert_add_appends_without_replacing(
            ResponseHeaderModifierFilter::new(http_filter(
                None,
                Some(vec![header("Vary", "Origin"), header("vary", "ignored")]),
                None,
            ))
            .expect("valid header modifier"),
        );
    }

    #[test]
    fn grpc_add_appends_to_preexisting_field_lines_and_ignores_equivalent_duplicate() {
        assert_add_appends_without_replacing(
            ResponseHeaderModifierFilter::new_from_grpc(grpc_filter(
                None,
                Some(vec![header("Vary", "Origin"), header("vary", "ignored")]),
                None,
            ))
            .expect("valid header modifier"),
        );
    }

    #[test]
    fn operations_run_in_set_add_remove_order_for_distinct_names() {
        let filter = ResponseHeaderModifierFilter::new(http_filter(
            Some(vec![header("X-Set", "set")]),
            Some(vec![header("X-Add", "add")]),
            Some(vec!["X-Remove".to_string()]),
        ))
        .expect("valid header modifier");
        let mut session = MockPluginSession::new();
        let mut sequence = Sequence::new();
        session
            .expect_set_response_header_value()
            .withf(|name, value| name == "X-Set" && value == "set")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_append_response_header_value()
            .withf(|name, value| name == "X-Add" && value == "add")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_remove_response_header()
            .withf(|name| name == "X-Remove")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        let mut log = PluginLog::new("ResponseHeaderModifier");

        assert_eq!(
            filter.modify_headers(&mut session, &mut log),
            PluginRunningResult::GoodNext
        );
        assert_eq!(log.render_for_test(), "");
    }

    #[test]
    fn retained_entry_errors_keep_static_diagnostics() {
        let filter = ResponseHeaderModifierFilter::new(http_filter(
            Some(vec![header("X-Set", "set")]),
            Some(vec![header("X-Add", "add")]),
            Some(vec!["X-Remove".to_string()]),
        ))
        .expect("valid header modifier");
        let mut session = MockPluginSession::new();
        session
            .expect_set_response_header_value()
            .returning(|_, _| Err(Box::new(std::io::Error::other("test failure"))));
        session
            .expect_append_response_header_value()
            .returning(|_, _| Err(Box::new(std::io::Error::other("test failure"))));
        session
            .expect_remove_response_header()
            .returning(|_| Err(Box::new(std::io::Error::other("test failure"))));
        let mut log = PluginLog::new("ResponseHeaderModifier");

        assert_eq!(
            filter.modify_headers(&mut session, &mut log),
            PluginRunningResult::GoodNext
        );
        assert_eq!(
            log.render_for_test(),
            "err: set resp hdr fail\nerr: add resp hdr fail\nerr: remove resp hdr fail"
        );
    }

    #[test]
    fn invalid_equivalent_duplicates_do_not_mutate_or_log() {
        let filter = ResponseHeaderModifierFilter::new(http_filter(
            Some(vec![header("X-Set", "first"), header("x-set", "ignored")]),
            Some(vec![header("X-Add", "first"), header("x-add", "ignored")]),
            Some(vec!["X-Remove".to_string(), "x-remove".to_string()]),
        ))
        .expect("valid header modifier");
        let mut session = MockPluginSession::new();
        session
            .expect_set_response_header_value()
            .withf(|name, value| name == "X-Set" && value == "first")
            .times(1)
            .returning(|_, _| Ok(()));
        session
            .expect_append_response_header_value()
            .withf(|name, value| name == "X-Add" && value == "first")
            .times(1)
            .returning(|_, _| Ok(()));
        session
            .expect_remove_response_header()
            .withf(|name| name == "X-Remove")
            .times(1)
            .returning(|_| Ok(()));
        let mut log = PluginLog::new("ResponseHeaderModifier");

        assert_eq!(
            filter.modify_headers(&mut session, &mut log),
            PluginRunningResult::GoodNext
        );
        assert_eq!(log.render_for_test(), "");
    }
}
