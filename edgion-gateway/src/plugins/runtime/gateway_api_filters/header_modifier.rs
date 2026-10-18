//! Immutable preparation shared by Gateway API request and response header modifiers.

use crate::plugins::runtime::PreparedHeader;
use edgion_resources::resources::{GRPCHeaderFilter, HTTPHeader, HTTPHeaderFilter};
use std::collections::HashSet;

pub(crate) struct HeaderModifierPlan {
    set: Vec<PreparedHeader>,
    add: Vec<PreparedHeader>,
    remove: Vec<String>,
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
            prepared.push(
                PreparedHeader::new(header.name, &header.value)
                    .map_err(|_| "header modifier values must be valid HTTP header values")?,
            );
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
#[path = "../../../../tests/unit/plugins/runtime/gateway_api_filters/header_modifier_test.rs"]
mod tests;
