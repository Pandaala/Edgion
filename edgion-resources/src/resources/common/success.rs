//! Declarative success predicate for outbound HTTP responses.
//!
//! `SuccessSpec` combines an optional status-code allow-list with an optional
//! list of JSON-Pointer body predicates (ANDed). It exists so a backend that
//! returns 200 OK with an error in the body is still classified as a failure.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Success criteria for an outbound response.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SuccessSpec {
    /// Status codes considered success. Absent/empty → any 2xx.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_codes: Option<Vec<u16>>,
    /// JSON-Pointer body predicates, ANDed with the status check.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body: Vec<BodyPredicate>,
}

/// One JSON-Pointer body predicate. Exactly one operator per entry — enforced
/// by the custom `Deserialize` impl below (a `#[serde(flatten)]`-ed externally
/// tagged enum would otherwise silently take the FIRST matching key and drop
/// the rest, e.g. `{pointer: /code, exists: true, equals: 0}` would silently
/// become `exists: true` and let a `200 {"code":-101}` classify as success).
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BodyPredicate {
    /// RFC 6901 JSON Pointer, e.g. "/code" or "/data/status".
    pub pointer: String,
    #[serde(flatten)]
    pub op: PredicateOp,
}

/// Deserialization-time shape used only to enforce "exactly one operator key".
/// Mirrors `BodyPredicate` + `PredicateOp`'s field names but with every
/// operator optional, so arity can be counted before deciding which
/// `PredicateOp` variant to build.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawBodyPredicate {
    pointer: String,
    #[serde(default)]
    equals: Option<serde_json::Value>,
    #[serde(default)]
    not_equals: Option<serde_json::Value>,
    #[serde(default)]
    exists: Option<bool>,
    #[serde(default, rename = "in")]
    in_values: Option<Vec<serde_json::Value>>,
}

impl<'de> Deserialize<'de> for BodyPredicate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawBodyPredicate::deserialize(deserializer)?;
        let present = [
            raw.equals.is_some(),
            raw.not_equals.is_some(),
            raw.exists.is_some(),
            raw.in_values.is_some(),
        ]
        .iter()
        .filter(|present| **present)
        .count();
        if present != 1 {
            return Err(serde::de::Error::custom(format!(
                "success.body predicate at pointer '{}' must specify exactly one of \
                 equals/notEquals/exists/in, found {}",
                raw.pointer, present
            )));
        }
        let op = if let Some(v) = raw.equals {
            PredicateOp::Equals(v)
        } else if let Some(v) = raw.not_equals {
            PredicateOp::NotEquals(v)
        } else if let Some(b) = raw.exists {
            PredicateOp::Exists(b)
        } else if let Some(v) = raw.in_values {
            PredicateOp::In(v)
        } else {
            unreachable!("arity already checked above: exactly one branch must be Some")
        };
        Ok(BodyPredicate {
            pointer: raw.pointer,
            op,
        })
    }
}

/// The predicate operator. Values are compared as strict `serde_json::Value` — no coercion.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PredicateOp {
    Equals(serde_json::Value),
    NotEquals(serde_json::Value),
    Exists(bool),
    In(Vec<serde_json::Value>),
}

/// Validate a SuccessSpec: pointers non-empty, `in` non-empty. Returns first error.
pub fn validate_success(spec: &SuccessSpec) -> Option<String> {
    // An explicit but empty `statusCodes: []` silently means "any 2xx" — the same
    // as omitting the field — which would let a Helm template that renders an
    // empty list widen an auth allow-list without anyone noticing. Reject it.
    if let Some(codes) = spec.status_codes.as_ref() {
        if codes.is_empty() {
            return Some(
                "success.statusCodes must not be an empty list; omit the field entirely to allow any 2xx".to_string(),
            );
        }
    }
    for p in &spec.body {
        if p.pointer.is_empty() {
            return Some("success.body[].pointer must not be empty".to_string());
        }
        if let PredicateOp::In(v) = &p.op {
            if v.is_empty() {
                return Some("success.body[].in must not be empty".to_string());
            }
        }
    }
    None
}

/// Why a response was (not) classified a success.
///
/// Distinguishing the two failure dimensions lets a caller (e.g. the outbound
/// retry loop) react to `retry_on_body_failure` without accidentally widening
/// it to cover a status the operator did not list in `retry_on_status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuccessOutcome {
    /// Status accepted and all body predicates passed.
    Success,
    /// The status code was not accepted; body predicates were not evaluated.
    StatusFail,
    /// Status accepted, but a body predicate failed (includes a non-JSON or
    /// unparseable body when body predicates are configured).
    BodyFail,
}

/// Classify a response, reporting WHICH dimension failed.
///
/// `is_json` is derived by the caller from the response Content-Type. When any
/// `body` predicate is present and the body is not valid JSON, the response is
/// a `BodyFail`.
pub fn classify_detail(spec: Option<&SuccessSpec>, status: u16, body: &[u8], is_json: bool) -> SuccessOutcome {
    let Some(spec) = spec else {
        return if (200..300).contains(&status) {
            SuccessOutcome::Success
        } else {
            SuccessOutcome::StatusFail
        };
    };
    // Status check.
    let status_ok = match spec.status_codes.as_ref() {
        Some(codes) if !codes.is_empty() => codes.contains(&status),
        _ => (200..300).contains(&status),
    };
    if !status_ok {
        return SuccessOutcome::StatusFail;
    }
    if spec.body.is_empty() {
        return SuccessOutcome::Success;
    }
    // Body predicates require valid JSON.
    if !is_json {
        return SuccessOutcome::BodyFail;
    }
    let json: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return SuccessOutcome::BodyFail,
    };
    for p in &spec.body {
        let found = json.pointer(&p.pointer);
        // A missing pointer is a failure for every operator except `Exists` — you
        // cannot assert equals/notEquals/in about a field that isn't there.
        // `Equals`/`In` already fail naturally on `None` (`None == Some(v)` and
        // `is_some_and` are both false); `NotEquals` needs an explicit check,
        // since `None != Some(v)` is otherwise vacuously true and would let a
        // missing field satisfy a `notEquals` predicate.
        let ok = match &p.op {
            PredicateOp::Exists(want) => found.is_some() == *want,
            PredicateOp::Equals(v) => found == Some(v),
            PredicateOp::NotEquals(v) => found.is_some_and(|f| f != v),
            PredicateOp::In(vs) => found.is_some_and(|f| vs.contains(f)),
        };
        if !ok {
            return SuccessOutcome::BodyFail;
        }
    }
    SuccessOutcome::Success
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/success_test.rs"]
mod tests;
