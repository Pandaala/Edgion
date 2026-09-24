//! Typed URL allow-list data for RequestAccessPolicy bindings.

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::plugin_configs::{
    UrlAccessCandidate, MAX_REFERENCED_IP_RULES, MAX_REFERENCED_URL_RULES,
};

/// Data-only URL candidates imported into one RequestAccessPolicy URL rule.
///
/// Secret and IP references are binding-time placeholders. The referring
/// EdgionPlugins Controller pass resolves them; this resource never embeds
/// resolved Secret material and cannot recursively reference another URL list.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestAccessUrlAllowListData {
    pub items: Vec<UrlAccessCandidate>,
}

impl RequestAccessUrlAllowListData {
    pub fn validate(&self) -> Result<(), String> {
        if self.items.len() > MAX_REFERENCED_URL_RULES {
            return Err(format!(
                "RequestAccessUrlAllowList items cannot exceed {MAX_REFERENCED_URL_RULES}"
            ));
        }
        let ip_rules = self.items.iter().map(UrlAccessCandidate::ip_rule_count).sum::<usize>();
        if ip_rules > MAX_REFERENCED_IP_RULES {
            return Err(format!(
                "RequestAccessUrlAllowList IP match conditions cannot exceed {MAX_REFERENCED_IP_RULES}"
            ));
        }
        let mut names = HashSet::new();
        for (index, item) in self.items.iter().enumerate() {
            if !names.insert(item.name.clone()) {
                return Err(format!("duplicate URL allow item name '{}'", item.name));
            }
            let mut item = item.clone();
            item.validate_and_init(true)
                .map_err(|error| format!("items[{index}]: {error}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_config_data/request_access_url_allow_list_test.rs"]
mod tests;
