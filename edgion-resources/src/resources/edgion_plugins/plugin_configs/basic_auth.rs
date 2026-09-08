//! Basic Auth plugin configuration

use std::collections::{HashMap, HashSet};

use crate::resources::common::group_name::default_group_name;
use crate::resources::edgion_plugins::auth_realm::validate_auth_realm;
use crate::resources::gateway::SecretObjectReference;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A named group of Secret references whose users all carry this group's
/// name in `ResolvedBasicAuthUser.group`. Used for audit / tenant
/// attribution in PluginLog.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BasicAuthSecretGroup {
    /// Group name; charset = Kubernetes label-value
    /// ([`crate::gwapi_types::GROUP_NAME_PATTERN`], <=63 chars).
    /// Surfaces in PluginLog as `group=<name>`.
    #[schemars(
        length(max = crate::gwapi_types::GROUP_NAME_MAX_LEN),
        pattern(crate::gwapi_types::GROUP_NAME_PATTERN)
    )]
    pub name: String,

    /// Human-readable description (not used by runtime).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Secret references for this group. Must be non-empty at preparse.
    /// Each Secret is `type: kubernetes.io/basic-auth` with `username` +
    /// `password` fields (1 Secret = 1 user).
    pub secret_refs: Vec<SecretObjectReference>,
}

/// Resolved BasicAuth user (controller-populated). The `password` field
/// holds either plaintext or htpasswd-format hash as stored in the Secret;
/// gateway's `load_users()` pre-hashes plaintext to `{SHA}<base64>` at
/// plugin construction.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedBasicAuthUser {
    /// Password value as stored in Secret (plaintext or pre-hashed).
    pub password: String,

    /// Group this user originated from. Always set by controller in
    /// current versions; `#[serde(default)]` provides a fallback so that
    /// Gateway-new / Controller-old rolling upgrades don't break
    /// deserialization.
    #[serde(default = "default_group_name")]
    pub group: String,
}

impl std::fmt::Debug for ResolvedBasicAuthUser {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ResolvedBasicAuthUser([redacted])")
    }
}

/// Basic Auth plugin configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BasicAuthConfig {
    /// Named groups of Secret references. Optional so anonymous-only
    /// deployments remain valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_groups: Option<Vec<BasicAuthSecretGroup>>,

    /// Hide Authorization header from upstream
    #[serde(default)]
    pub hide_credentials: bool,

    /// Delay in milliseconds before returning an authentication failure response.
    /// Increases the time cost for brute-force / credential-stuffing attacks.
    /// Default: 0 (no delay).
    #[serde(default)]
    pub auth_failure_delay_ms: u64,

    /// Realm for WWW-Authenticate header
    #[serde(default = "default_realm")]
    pub realm: String,

    /// Anonymous consumer username (allows unauthenticated access)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anonymous: Option<String>,

    /// Resolved users from Secret refs (controller populated).
    /// Key: username, value: ResolvedBasicAuthUser (password + group).
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_users: Option<HashMap<String, ResolvedBasicAuthUser>>,
}

fn default_realm() -> String {
    "API Gateway".to_string()
}

impl Default for BasicAuthConfig {
    fn default() -> Self {
        Self {
            secret_groups: None,
            hide_credentials: false,
            auth_failure_delay_ms: 0,
            realm: default_realm(),
            anonymous: None,
            resolved_users: None,
        }
    }
}

impl BasicAuthConfig {
    pub fn detect_validation_error(&self) -> Option<String> {
        if let Err(error) = validate_auth_realm(&self.realm) {
            return Some(error.to_string());
        }
        if self.realm.trim().is_empty() {
            return Some("realm cannot be empty".to_string());
        }

        // Validate secret_groups structure (cross-group username uniqueness
        // is checked at controller resolve time, not here).
        if let Some(groups) = &self.secret_groups {
            if groups.is_empty() {
                return Some("secretGroups cannot be empty when present".to_string());
            }
            let mut seen: HashSet<&str> = HashSet::new();
            for g in groups {
                if let Err(e) = crate::resources::common::group_name::validate_group_name(&g.name) {
                    return Some(format!("secretGroups: {}", e));
                }
                if !seen.insert(g.name.as_str()) {
                    return Some(format!("duplicate group name '{}'", g.name));
                }
                if g.secret_refs.is_empty() {
                    return Some(format!("group '{}' has no secretRefs", g.name));
                }
            }
        }

        // Final gate: anonymous or groups required
        let has_secret_groups = self.secret_groups.as_ref().is_some_and(|v| !v.is_empty());
        let has_resolved = self.resolved_users.as_ref().is_some_and(|v| !v.is_empty());
        if self.anonymous.is_none() && !has_secret_groups && !has_resolved {
            return Some("secretGroups (or resolvedUsers) is required when anonymous is not set".to_string());
        }

        None
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for BasicAuthConfig {
    fn validation_error(&self) -> Option<String> {
        self.detect_validation_error()
    }
}

#[cfg(test)]
mod tests {
    use super::BasicAuthConfig;
    use std::collections::HashMap;

    #[test]
    fn resolved_password_debug_redaction_preserves_transport() {
        for password in ["private-password", "{SHA}private-hash"] {
            let config = BasicAuthConfig {
                resolved_users: Some(HashMap::from([(
                    "alice".into(),
                    super::ResolvedBasicAuthUser {
                        password: password.into(),
                        group: "private-group".into(),
                    },
                )])),
                ..Default::default()
            };
            for debug in [format!("{config:?}"), format!("{config:#?}")] {
                assert!(debug.contains("[redacted]"));
                assert!(!debug.contains(password));
                assert!(!debug.contains("private-group"));
            }
            let wire = serde_json::to_string(&config).unwrap();
            let decoded: BasicAuthConfig = serde_json::from_str(&wire).unwrap();
            assert_eq!(decoded.resolved_users.unwrap()["alice"].password, password);
            let _guard = crate::redact::AdminRedactGuard::new();
            let admin = serde_json::to_value(&config).unwrap();
            assert_eq!(admin["resolvedUsers"], "[redacted]");
        }
    }

    #[test]
    fn test_detect_validation_error_requires_creds_or_anonymous() {
        let cfg = BasicAuthConfig::default();
        assert!(cfg
            .detect_validation_error()
            .is_some_and(|e| e.contains("secretGroups")));
    }

    #[test]
    fn test_default_auth_failure_delay_ms_is_zero() {
        let cfg = BasicAuthConfig::default();
        assert_eq!(cfg.auth_failure_delay_ms, 0);
    }

    #[test]
    fn test_detect_validation_error_allows_resolved_users() {
        use super::ResolvedBasicAuthUser;
        let cfg = BasicAuthConfig {
            resolved_users: Some(HashMap::from([(
                "alice".to_string(),
                ResolvedBasicAuthUser {
                    password: "pass".to_string(),
                    group: "default".to_string(),
                },
            )])),
            ..Default::default()
        };
        assert!(cfg.detect_validation_error().is_none());
    }

    use crate::resources::edgion_plugins::plugin_configs::BasicAuthSecretGroup;
    use crate::resources::gateway::SecretObjectReference;

    fn make_sor(name: &str) -> SecretObjectReference {
        SecretObjectReference {
            group: None,
            kind: None,
            name: name.to_string(),
            namespace: None,
        }
    }

    #[test]
    fn test_validate_secret_groups_empty() {
        let cfg = BasicAuthConfig {
            secret_groups: Some(vec![]),
            anonymous: None,
            ..Default::default()
        };
        assert_eq!(
            cfg.detect_validation_error().unwrap(),
            "secretGroups cannot be empty when present"
        );
    }

    #[test]
    fn test_validate_secret_groups_duplicate_name() {
        let cfg = BasicAuthConfig {
            secret_groups: Some(vec![
                BasicAuthSecretGroup {
                    name: "a".into(),
                    description: None,
                    secret_refs: vec![make_sor("s1")],
                },
                BasicAuthSecretGroup {
                    name: "a".into(),
                    description: None,
                    secret_refs: vec![make_sor("s2")],
                },
            ]),
            anonymous: None,
            ..Default::default()
        };
        assert!(cfg
            .detect_validation_error()
            .unwrap()
            .contains("duplicate group name 'a'"));
    }

    #[test]
    fn test_validate_secret_groups_invalid_name() {
        let cfg = BasicAuthConfig {
            secret_groups: Some(vec![BasicAuthSecretGroup {
                name: "-bad".into(),
                description: None,
                secret_refs: vec![make_sor("s")],
            }]),
            anonymous: None,
            ..Default::default()
        };
        assert!(cfg
            .detect_validation_error()
            .unwrap()
            .contains("must start with alphanumeric"));
    }

    #[test]
    fn test_validate_secret_groups_empty_refs() {
        let cfg = BasicAuthConfig {
            secret_groups: Some(vec![BasicAuthSecretGroup {
                name: "a".into(),
                description: None,
                secret_refs: vec![],
            }]),
            anonymous: None,
            ..Default::default()
        };
        assert!(cfg
            .detect_validation_error()
            .unwrap()
            .contains("group 'a' has no secretRefs"));
    }

    #[test]
    fn test_dual_group_yaml_deserializes() {
        let yaml = r#"
secretGroups:
  - name: partner-a
    secretRefs:
      - name: secret-a
        namespace: edgion-default
  - name: partner-b
    secretRefs:
      - name: secret-b
        namespace: edgion-default
realm: "Dynamic Partner API"
"#;
        let cfg: BasicAuthConfig = serde_yaml::from_str(yaml).unwrap();
        let groups = cfg.secret_groups.unwrap();
        assert_eq!(groups.len(), 2, "should parse 2 groups, got {}", groups.len());
        assert_eq!(groups[0].name, "partner-a");
        assert_eq!(groups[1].name, "partner-b");
    }
}
