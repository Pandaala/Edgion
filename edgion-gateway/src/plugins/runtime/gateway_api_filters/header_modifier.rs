//! Immutable preparation shared by Gateway API request and response header modifiers.

use edgion_resources::resources::{GRPCHeaderFilter, HTTPHeader, HTTPHeaderFilter};
use http::HeaderValue;
use std::collections::HashSet;

pub(crate) struct HeaderModifierPlan {
    set: Vec<PreparedHeader>,
    add: Vec<PreparedHeader>,
    remove: Vec<String>,
}

pub(crate) struct PreparedHeader {
    name: String,
    value: HeaderValue,
}

impl PreparedHeader {
    #[inline]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub(crate) fn value(&self) -> &HeaderValue {
        &self.value
    }
}

impl HeaderModifierPlan {
    pub(super) fn from_http(config: HTTPHeaderFilter) -> Result<Self, &'static str> {
        config.validate()?;
        Self::from_parts(config.set, config.add, config.remove)
    }

    pub(super) fn from_grpc(config: GRPCHeaderFilter) -> Result<Self, &'static str> {
        config.validate()?;
        Self::from_parts(config.set, config.add, config.remove)
    }

    fn from_parts(
        set: Option<Vec<HTTPHeader>>,
        add: Option<Vec<HTTPHeader>>,
        remove: Option<Vec<String>>,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            set: prepare_headers(set)?,
            add: prepare_headers(add)?,
            remove: retain_first_names(remove),
        })
    }

    #[inline]
    pub(crate) fn set(&self) -> &[PreparedHeader] {
        &self.set
    }

    #[inline]
    pub(crate) fn add(&self) -> &[PreparedHeader] {
        &self.add
    }

    #[inline]
    pub(crate) fn remove(&self) -> &[String] {
        &self.remove
    }
}

fn prepare_headers(headers: Option<Vec<HTTPHeader>>) -> Result<Vec<PreparedHeader>, &'static str> {
    let Some(headers) = headers else {
        return Ok(Vec::new());
    };
    let mut seen = HashSet::with_capacity(headers.len());
    let mut prepared = Vec::with_capacity(headers.len());
    for header in headers {
        if seen.insert(header.name.to_ascii_lowercase()) {
            let value = HeaderValue::from_bytes(header.value.as_bytes())
                .map_err(|_| "header modifier values must be valid HTTP header values")?;
            // A freshly allocated HeaderValue promotes its Bytes storage on the
            // first clone. Perform that one-time allocation during publication
            // so every request-path clone only increments the shared reference.
            let shared_value = value.clone();
            drop(value);
            prepared.push(PreparedHeader {
                name: header.name,
                value: shared_value,
            });
        }
    }
    Ok(prepared)
}

fn retain_first_names(names: Option<Vec<String>>) -> Vec<String> {
    let Some(names) = names else {
        return Vec::new();
    };
    let mut seen = HashSet::with_capacity(names.len());
    names
        .into_iter()
        .filter(|name| seen.insert(name.to_ascii_lowercase()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(name: &str, value: &str) -> HTTPHeader {
        HTTPHeader {
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    fn assert_prepared_plan(plan: &HeaderModifierPlan) {
        assert_eq!(plan.set().len(), 2);
        assert_eq!(plan.set()[0].name(), "X-Set");
        assert_eq!(plan.set()[0].value(), "first-set");
        assert_eq!(plan.set()[1].name(), "X-Set-Other");
        assert_eq!(plan.set()[1].value(), "other-set");

        assert_eq!(plan.add().len(), 2);
        assert_eq!(plan.add()[0].name(), "X-Add");
        assert_eq!(plan.add()[0].value(), "first-add");
        assert_eq!(plan.add()[1].name(), "X-Add-Other");
        assert_eq!(plan.add()[1].value(), "other-add");

        assert_eq!(plan.remove(), &["X-Remove", "X-Remove-Other"]);
    }

    fn set_entries() -> Vec<HTTPHeader> {
        vec![
            header("X-Set", "first-set"),
            header("x-set", "ignored-set"),
            header("X-Set-Other", "other-set"),
        ]
    }

    fn add_entries() -> Vec<HTTPHeader> {
        vec![
            header("X-Add", "first-add"),
            header("x-add", "ignored-add"),
            header("X-Add-Other", "other-add"),
        ]
    }

    fn remove_entries() -> Vec<String> {
        vec![
            "X-Remove".to_string(),
            "x-remove".to_string(),
            "X-Remove-Other".to_string(),
        ]
    }

    #[test]
    fn http_and_grpc_plans_keep_first_equivalent_entry_and_stable_order() {
        let http = HeaderModifierPlan::from_http(HTTPHeaderFilter {
            set: Some(set_entries()),
            add: Some(add_entries()),
            remove: Some(remove_entries()),
        })
        .expect("valid header modifier");
        let grpc = HeaderModifierPlan::from_grpc(GRPCHeaderFilter {
            set: Some(set_entries()),
            add: Some(add_entries()),
            remove: Some(remove_entries()),
        })
        .expect("valid header modifier");

        assert_prepared_plan(&http);
        assert_prepared_plan(&grpc);
    }

    #[test]
    fn absent_and_empty_lists_prepare_empty_slices() {
        let absent = HeaderModifierPlan::from_http(HTTPHeaderFilter {
            set: None,
            add: None,
            remove: None,
        })
        .expect("valid header modifier");
        let empty = HeaderModifierPlan::from_grpc(GRPCHeaderFilter {
            set: Some(Vec::new()),
            add: Some(Vec::new()),
            remove: Some(Vec::new()),
        })
        .expect("valid header modifier");

        assert!(absent.set().is_empty());
        assert!(absent.add().is_empty());
        assert!(absent.remove().is_empty());
        assert!(empty.set().is_empty());
        assert!(empty.add().is_empty());
        assert!(empty.remove().is_empty());
    }

    #[test]
    fn cross_action_same_name_is_rejected_before_preparation() {
        let plan = HeaderModifierPlan::from_http(HTTPHeaderFilter {
            set: Some(vec![header("X-Same", "set")]),
            add: Some(vec![header("x-same", "add")]),
            remove: Some(vec!["X-SAME".to_string()]),
        });

        assert!(plan.is_err());
    }

    #[test]
    fn plan_slice_accessors_do_not_allocate() {
        let plan = HeaderModifierPlan::from_http(HTTPHeaderFilter {
            set: Some(vec![header("X-Set", "set")]),
            add: Some(vec![header("X-Add", "add")]),
            remove: Some(vec!["X-Remove".to_string()]),
        })
        .expect("valid header modifier");

        let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            std::hint::black_box((plan.set(), plan.add(), plan.remove()))
        });

        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }

    #[test]
    fn prepared_value_clones_share_bytes_without_allocating() {
        let plan = HeaderModifierPlan::from_http(HTTPHeaderFilter {
            set: Some(vec![header("X-Set", &"value".repeat(128))]),
            add: None,
            remove: None,
        })
        .expect("valid header modifier");

        let (value, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            std::hint::black_box(plan.set()[0].value().clone())
        });

        assert_eq!(value, "value".repeat(128));
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }
}
