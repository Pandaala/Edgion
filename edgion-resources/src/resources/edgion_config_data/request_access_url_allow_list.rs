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
mod tests {
    use super::*;
    use crate::resources::edgion_plugins::plugin_configs::{
        AccessPathMatch, AccessPathMatchType, Condition, ConditionSet, IpAccessMatch,
    };

    fn candidate(name: &str, hosts: Vec<&str>, match_type: AccessPathMatchType, path: &str) -> UrlAccessCandidate {
        UrlAccessCandidate {
            name: name.to_string(),
            hosts: hosts.into_iter().map(str::to_string).collect(),
            paths: vec![AccessPathMatch {
                match_type,
                value: path.to_string(),
            }],
            conditions: None,
        }
    }

    #[test]
    fn validates_named_url_items() {
        let data = RequestAccessUrlAllowListData {
            items: vec![UrlAccessCandidate {
                name: "vendor".to_string(),
                hosts: vec!["callbacks.example.com".to_string()],
                paths: vec![AccessPathMatch {
                    match_type: AccessPathMatchType::PathPrefix,
                    value: "/callbacks/".to_string(),
                }],
                conditions: None,
            }],
        };
        assert!(data.validate().is_ok());
    }

    #[test]
    fn allows_hostless_data_for_consumer_side_validation() {
        let data = RequestAccessUrlAllowListData {
            items: vec![UrlAccessCandidate {
                name: "all-hosts".to_string(),
                hosts: vec![],
                paths: vec![AccessPathMatch {
                    match_type: AccessPathMatchType::Exact,
                    value: "/health".to_string(),
                }],
                conditions: None,
            }],
        };
        assert!(data.validate().is_ok());
    }

    #[test]
    fn freezes_empty_list_as_valid_data_with_no_candidates() {
        assert!(RequestAccessUrlAllowListData { items: vec![] }.validate().is_ok());
    }

    #[test]
    fn rejects_duplicate_item_names() {
        let data = RequestAccessUrlAllowListData {
            items: vec![
                candidate("vendor", vec!["a.example.com"], AccessPathMatchType::Exact, "/a"),
                candidate("vendor", vec!["b.example.com"], AccessPathMatchType::Exact, "/b"),
            ],
        };
        assert!(data.validate().unwrap_err().contains("duplicate URL allow item name"));
    }

    #[test]
    fn validates_all_path_kinds_and_nested_ip_conditions() {
        let mut exact = candidate(
            "exact",
            vec!["callbacks.example.com"],
            AccessPathMatchType::Exact,
            "/exact",
        );
        exact.conditions = Some(ConditionSet::all_of(vec![Condition::DirectPeerIpMatch(
            IpAccessMatch {
                ips: vec!["127.0.0.0/8".to_string()],
                ..Default::default()
            },
        )]));
        let mut prefix = candidate(
            "prefix",
            vec!["callbacks.example.com"],
            AccessPathMatchType::PathPrefix,
            "/callbacks/:vendor",
        );
        prefix.conditions = Some(ConditionSet::all_of(vec![Condition::RemoteIpMatch(IpAccessMatch {
            ip_refs: vec![crate::resources::edgion_config_data::EdgionConfigDataRef {
                name: "vendor-networks".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        })]));
        let regex = candidate(
            "regex",
            vec!["callbacks.example.com"],
            AccessPathMatchType::RegularExpression,
            r"^/events/[0-9]+$",
        );
        assert!(RequestAccessUrlAllowListData {
            items: vec![exact, prefix, regex]
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn rejects_malformed_host_path_and_regex() {
        for item in [
            candidate("wildcard", vec!["*.example.com"], AccessPathMatchType::Exact, "/ok"),
            candidate("query", vec!["a.example.com"], AccessPathMatchType::Exact, "/a?x=1"),
            candidate(
                "regex",
                vec!["a.example.com"],
                AccessPathMatchType::RegularExpression,
                "(",
            ),
        ] {
            assert!(RequestAccessUrlAllowListData { items: vec![item] }.validate().is_err());
        }
    }
}
