//! `AccessLogExtern` declaration schema.
//!
//! An `AccessLogExtern` entry is **not a plugin**: it never executes in any
//! stage. It declares one access-log field, and is consumed when the access log
//! entry is serialized (`logging()`). See
//! `docs/superpowers/specs/2026-07-16-access-log-extern-plugin-design.md`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::common::log_labels::LOG_LABEL_MAX_KEY_LEN;

/// Where an `AccessLogExtern` field reads its value from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LogExtSource {
    /// A `metadata.labels` key on the matched HTTPRoute / GRPCRoute. Never masked.
    RouteLabel,
    /// A `metadata.annotations` key on the matched HTTPRoute / GRPCRoute. Never masked.
    RouteAnnotation,
    /// A request header. Masked unless whitelisted.
    Header,
    /// A query parameter from the **entry-time raw query**, frozen at request
    /// entry — not the post-rewrite URI. Masked unless whitelisted.
    Query,
    /// A request cookie. Masked unless whitelisted.
    Cookie,
    /// A response header. Masked unless whitelisted.
    RespHeader,
    /// A plugin-set context variable. Masked unless whitelisted.
    Ctx,
}

/// One declared access-log field. Exactly one source per field: template
/// composition is deliberately unsupported, because a composed string loses the
/// provenance that per-key masking needs. Compose in `CtxSet` and log the
/// resulting ctx var with `from: ctx` instead.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccessLogExternField {
    /// Field name inside the entry's `ext` block.
    pub key: String,
    /// Source type.
    pub from: LogExtSource,
    /// Key within the source.
    pub name: String,
}

/// Header names that may never be projected, whitelisted, or logged in clear.
///
/// These are not "sensitive by default" — they are un-nameable. The rationale is
/// blast radius versus reversibility: whitelisting `authorization` by mistake
/// puts every bearer token in the fleet into Elasticsearch, into backups, and
/// into every downstream warehouse, and it cannot be recalled. An operation with
/// that cost requires a code change and a review, not one `kubectl apply`.
///
/// `x-api-key` is here because it is Edgion's own KeyAuth default header.
const BLOCKED_HEADER_NAMES: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "api-key",
    "apikey",
    "x-auth-token",
    "authentication",
    "x-amz-security-token",
    "www-authenticate",
    "proxy-authenticate",
];

/// `ctx_map` keys that carry categorically-sensitive content — full JWT/OIDC
/// claims JSON (`store_claims_in_ctx`) and decrypted JWE plaintext
/// (`store_payload_in_ctx`). Downstream plugins read them in-memory; they must
/// never reach the log. Any new `ctx_map` key storing claims, decrypted
/// payloads, tokens or secrets belongs here.
pub const ACCESS_LOG_BLOCKED_CTX_KEYS: &[&str] = &["jwt_claims", "oidc_claims", "jwe_payload"];

/// Annotation prefixes a route author may not project. `routeAnnotation` values
/// are never masked, and a route carries annotations its author did not write:
/// `kubectl.kubernetes.io/last-applied-configuration` holds the whole route spec,
/// and `edgion.io/*` holds plugin wiring. Since a route author is a tenant rather
/// than the platform team, these stay un-nameable.
const BLOCKED_ANNOTATION_PREFIXES: &[&str] = &["kubectl.kubernetes.io/", "edgion.io/"];

/// Whether `name` is un-nameable for `source`. Header matching is
/// case-insensitive (HTTP semantics); ctx keys are case-sensitive.
pub fn is_blocked(source: LogExtSource, name: &str) -> bool {
    match source {
        LogExtSource::Header | LogExtSource::RespHeader => {
            BLOCKED_HEADER_NAMES.iter().any(|b| name.eq_ignore_ascii_case(b))
        }
        LogExtSource::Ctx => ACCESS_LOG_BLOCKED_CTX_KEYS.contains(&name),
        LogExtSource::RouteAnnotation => BLOCKED_ANNOTATION_PREFIXES.iter().any(|p| name.starts_with(p)),
        LogExtSource::RouteLabel | LogExtSource::Query | LogExtSource::Cookie => false,
    }
}

impl AccessLogExternField {
    /// Returns `Some(error)` when this declaration is invalid. Human-readable;
    /// surfaced through the `EdgionPlugins` `Accepted=False` status condition.
    pub fn validation_error(&self) -> Option<String> {
        if self.key.is_empty() {
            return Some("key must not be empty".to_string());
        }
        if self.name.is_empty() {
            return Some("name must not be empty".to_string());
        }
        if self.key.len() > LOG_LABEL_MAX_KEY_LEN {
            return Some(format!(
                "key must be at most {LOG_LABEL_MAX_KEY_LEN} bytes (got {})",
                self.key.len()
            ));
        }
        if self.name.len() > LOG_LABEL_MAX_KEY_LEN {
            return Some(format!(
                "name must be at most {LOG_LABEL_MAX_KEY_LEN} bytes (got {})",
                self.name.len()
            ));
        }
        if is_blocked(self.from, &self.name) {
            return Some(format!(
                "name '{}' is blocked for this source and can never be logged",
                self.name
            ));
        }
        None
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/access_log_extern_test.rs"]
mod tests;
