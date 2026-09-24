//! Request Restriction plugin configuration
//!
//! This plugin restricts access based on request attributes and identity,
//! evaluated through the shared [`Condition`] enum.
//!
//! ## Model (DNF: OR-of-AND)
//!
//! - `mode` is **required** and states what a match means: `Deny` (match =>
//!   reject, unmatched passes) or `Allow` (match => pass, unmatched is
//!   rejected). There is no default: omitting it is a validation error, so the
//!   polarity is always written out and can never silently flip.
//! - `conditions` explicitly selects a top-level `allOf` or `anyOf`.
//! - An `anyOf` alternative is either one direct Condition or one nested
//!   `allOf` group. Evaluation follows configuration order and short-circuits.
//!
//! A condition is a shared [`Condition`] predicate. Predicates match request
//! attributes (header/cookie/query/path/method) and, additionally,
//! post-authentication identity: mTLS client-cert subject / SAN / CN
//! (`mtlsMatch`) and verified JWT claims (`jwtClaimMatch`). Inline IP sets
//! (`directPeerIpMatch` / `remoteIpMatch` with `ips` and `ipRefs`) are
//! accepted. It is still a local allow/deny matcher, not a full RBAC
//! orchestration layer.
//!
//! ## Configuration Examples
//!
//! ### Deny: block bots OR /admin paths
//! ```yaml
//! requestRestriction:
//!   mode: Deny
//!   conditions:
//!     anyOf:
//!       - type: keyMatch
//!         key:
//!           type: header
//!           name: User-Agent
//!         regex: ["(?i).*Bot.*"]
//!       - type: keyMatch
//!         key:
//!           type: path
//!         values: ["/admin"]
//!   message: "Access denied"
//! ```
//!
//! ### Allow: allow only read-only methods
//! ```yaml
//! requestRestriction:
//!   mode: Allow
//!   conditions:
//!     allOf:
//!       - type: keyMatch
//!         key:
//!           type: method
//!         values: ["GET", "HEAD", "OPTIONS"]
//!   status: 405
//! ```
//!
//! ### AND within a group: deny only DELETE to /admin
//! ```yaml
//! requestRestriction:
//!   mode: Deny
//!   conditions:
//!     allOf:
//!       - type: keyMatch
//!         key:
//!           type: method
//!         values: ["DELETE"]
//!       - type: keyMatch
//!         key:
//!           type: path
//!         values: ["/admin"]
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::conditions::{ConditionSet, ConditionValidationProfile};

/// What a matching rule group means. Required — there is no default, so the
/// polarity is always stated explicitly in the configuration.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum RestrictionMode {
    /// Any matching group => reject; no group matches => pass.
    Deny,
    /// Any matching group => pass; no group matches => reject.
    Allow,
}

/// Request Restriction plugin configuration.
///
/// Bounded-DNF model: `conditions` explicitly selects `allOf` or `anyOf`. The required
/// `mode` states what a match means. A condition is a shared [`Condition`]
/// predicate (no per-condition allow/deny). Predicates match request
/// attributes and post-authentication identity (mTLS subject/SAN/CN, verified
/// JWT claims); it remains a local allow/deny matcher, not a full RBAC
/// orchestration layer.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestRestrictionConfig {
    /// Polarity — **required**. `Deny`: a matching group rejects. `Allow`: a
    /// matching group passes and anything unmatched is rejected.
    ///
    /// Modelled as `Option` rather than a serde-required field so that a missing
    /// value is reported by `check_fields` through the normal plugin-validation
    /// path (a readable `Accepted=False` reason) instead of failing the whole
    /// resource's deserialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<RestrictionMode>,

    /// Explicit bounded Boolean expression evaluated in configuration order.
    pub conditions: ConditionSet,

    /// HTTP status code for rejection (default: 403).
    #[serde(default = "default_status")]
    #[schemars(range(min = 200, max = 599))]
    pub status: u16,

    /// Custom rejection message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn default_status() -> u16 {
    403
}

impl Default for RequestRestrictionConfig {
    fn default() -> Self {
        Self {
            mode: None,
            conditions: ConditionSet::default(),
            status: default_status(),
            message: None,
        }
    }
}

impl RequestRestrictionConfig {
    /// Validate configuration and compile matchers.
    ///
    /// Returns `Err(message)` if any condition is malformed; on success every
    /// condition has its compiled matchers (HashSet, regex, IP radix) populated.
    pub fn validate(&mut self) -> Result<(), String> {
        self.validate_and_compile()
    }

    /// Same errors as [`Self::validate_and_compile`], without requiring `&mut self`.
    pub fn check_fields(&self) -> Option<String> {
        self.clone().validate_and_compile().err()
    }

    /// Whether a matching group means "allow".
    ///
    /// `check_fields` rejects a missing `mode`, and the gateway fails closed on
    /// an invalid config before evaluating any request, so the unvalidated
    /// `None` case is unreachable. It resolves to allow-polarity, so unmatched
    /// traffic is rejected rather than admitted.
    pub fn allow_mode(&self) -> bool {
        debug_assert!(
            self.mode.is_some(),
            "allow_mode called on an unvalidated config with no mode"
        );
        !matches!(self.mode, Some(RestrictionMode::Deny))
    }

    /// True if any condition is HMAC with `validateRequestBody`.
    pub fn validates_request_body(&self) -> bool {
        self.conditions.validates_request_body()
    }

    /// Validate and compile all conditions across all groups.
    fn validate_and_compile(&mut self) -> Result<(), String> {
        if self.mode.is_none() {
            return Err("mode must be specified (Deny | Allow)".to_string());
        }
        if !crate::http_status::is_valid_final_http_status(self.status) {
            return Err(format!("Invalid status code: {}", self.status));
        }
        self.conditions
            .compile()
            .map_err(|errors| format!("invalid condition regex: {}", errors[0]))?;
        self.conditions
            .validate(ConditionValidationProfile::RequestAsync)
            .map_err(|errors| errors.join("; "))?;
        Ok(())
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for RequestRestrictionConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/request_restriction_test.rs"]
mod tests;
