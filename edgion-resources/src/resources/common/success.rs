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
mod tests {
    use super::*;

    fn spec(yaml: &str) -> SuccessSpec {
        serde_yaml::from_str(yaml).unwrap()
    }

    /// Test-only convenience wrapper: `classify_detail(..) == Success` as a bool.
    /// `classify_success` (the former production wrapper of the same shape) was
    /// removed as dead code — it had no caller outside its own tests, and every
    /// real call site (the outbound retry loop) needs the finer-grained
    /// `SuccessOutcome` from `classify_detail` anyway.
    fn is_success(spec: Option<&SuccessSpec>, status: u16, body: &[u8], is_json: bool) -> bool {
        classify_detail(spec, status, body, is_json) == SuccessOutcome::Success
    }

    #[test]
    fn status_only_defaults_to_2xx_when_absent() {
        assert!(is_success(None, 200, b"", false));
        assert!(!is_success(None, 500, b"", false));
    }

    #[test]
    fn body_equals_number_is_strict() {
        let s = spec("statusCodes: [200]\nbody:\n  - pointer: /code\n    equals: 0\n");
        assert!(is_success(Some(&s), 200, br#"{"code": 0}"#, true));
        assert!(!is_success(Some(&s), 200, br#"{"code": "0"}"#, true)); // strict: 0 != "0"
        assert!(!is_success(Some(&s), 200, br#"{"code": -101}"#, true));
    }

    #[test]
    fn body_in_and_exists() {
        let s = spec("body:\n  - pointer: /status\n    in: [ok, accepted]\n  - pointer: /id\n    exists: true\n");
        assert!(is_success(Some(&s), 200, br#"{"status":"ok","id":"x"}"#, true));
        assert!(!is_success(Some(&s), 200, br#"{"status":"err","id":"x"}"#, true));
        assert!(!is_success(Some(&s), 200, br#"{"status":"ok"}"#, true)); // id missing
    }

    #[test]
    fn body_predicate_on_non_json_is_failure() {
        let s = spec("body:\n  - pointer: /code\n    equals: 0\n");
        assert!(!is_success(Some(&s), 200, b"not json", false));
        assert!(!is_success(Some(&s), 200, b"{bad", true)); // unparseable
    }

    #[test]
    fn status_and_body_are_anded() {
        let s = spec("statusCodes: [200]\nbody:\n  - pointer: /ok\n    equals: true\n");
        assert!(!is_success(Some(&s), 500, br#"{"ok": true}"#, true)); // status fails
        assert!(!is_success(Some(&s), 200, br#"{"ok": false}"#, true)); // body fails
        assert!(is_success(Some(&s), 200, br#"{"ok": true}"#, true));
    }

    /// `classify_detail` must distinguish StatusFail from BodyFail — this is the
    /// dimension the outbound retry loop needs so `retry_on_body_failure` cannot
    /// silently widen to cover a status the operator never listed in `retry_on_status`.
    #[test]
    fn classify_detail_distinguishes_status_fail_from_body_fail() {
        let s = spec("statusCodes: [200]\nbody:\n  - pointer: /code\n    equals: 0\n");
        assert_eq!(
            classify_detail(Some(&s), 404, br#"{"code": 0}"#, true),
            SuccessOutcome::StatusFail
        );
        assert_eq!(
            classify_detail(Some(&s), 200, br#"{"code": -101}"#, true),
            SuccessOutcome::BodyFail
        );
        assert_eq!(
            classify_detail(Some(&s), 200, br#"{"code": 0}"#, true),
            SuccessOutcome::Success
        );
    }

    /// With no `SuccessSpec` at all, a non-2xx status is still a StatusFail (not
    /// a BodyFail — there are no body predicates to fail).
    #[test]
    fn classify_detail_no_spec_non_2xx_is_status_fail() {
        assert_eq!(classify_detail(None, 500, b"", false), SuccessOutcome::StatusFail);
        assert_eq!(classify_detail(None, 200, b"", false), SuccessOutcome::Success);
    }

    /// A non-JSON body with body predicates configured is a BodyFail, not a
    /// StatusFail — the status already passed.
    #[test]
    fn classify_detail_non_json_with_body_predicates_is_body_fail() {
        let s = spec("body:\n  - pointer: /code\n    equals: 0\n");
        assert_eq!(
            classify_detail(Some(&s), 200, b"not json", false),
            SuccessOutcome::BodyFail
        );
    }

    // ── IMPORTANT 5: BodyPredicate arity enforcement ─────────────────────────

    /// Zero operators (just a bare `pointer`) must be rejected at deserialize time.
    #[test]
    fn body_predicate_zero_operators_rejected() {
        let err = serde_yaml::from_str::<BodyPredicate>("pointer: /code\n")
            .expect_err("a predicate with no operator must fail to parse");
        assert!(err.to_string().contains("exactly one"), "got: {err}");
    }

    /// CRITICAL / IMPORTANT 5 regression: `{pointer: /code, exists: true, equals: 0}`
    /// used to silently take `exists` (the first matching key in the externally
    /// tagged enum) and drop `equals: 0` — letting a `200 {"code":-101}` classify
    /// as success. Two operators on one entry must now be rejected outright.
    #[test]
    fn body_predicate_two_operators_rejected() {
        let err = serde_yaml::from_str::<BodyPredicate>("pointer: /code\nexists: true\nequals: 0\n")
            .expect_err("a predicate with two operators must fail to parse");
        assert!(err.to_string().contains("exactly one"), "got: {err}");
    }

    /// Exactly one operator parses fine and round-trips to the right variant.
    #[test]
    fn body_predicate_one_operator_accepted() {
        let p: BodyPredicate = serde_yaml::from_str("pointer: /code\nequals: 0\n").unwrap();
        assert_eq!(p.pointer, "/code");
        assert!(matches!(p.op, PredicateOp::Equals(v) if v == serde_json::json!(0)));

        let p: BodyPredicate = serde_yaml::from_str("pointer: /code\nnotEquals: 0\n").unwrap();
        assert!(matches!(p.op, PredicateOp::NotEquals(v) if v == serde_json::json!(0)));

        let p: BodyPredicate = serde_yaml::from_str("pointer: /code\nexists: false\n").unwrap();
        assert!(matches!(p.op, PredicateOp::Exists(false)));

        let p: BodyPredicate = serde_yaml::from_str("pointer: /code\nin: [1, 2]\n").unwrap();
        assert!(matches!(p.op, PredicateOp::In(v) if v.len() == 2));
    }

    // ── IMPORTANT 7: missing pointer semantics ────────────────────────────────

    /// CRITICAL / IMPORTANT 7 regression: `notEquals` on a MISSING pointer used to
    /// pass (`None != Some(v)` is vacuously true) — e.g. a `200 {"authenticated":
    /// false}` response with no `/error` key would satisfy
    /// `{pointer: /error, notEquals: true}` and be classified success. A missing
    /// pointer must now be a failure for `notEquals`.
    #[test]
    fn missing_pointer_with_not_equals_is_failure() {
        let s = spec("body:\n  - pointer: /error\n    notEquals: true\n");
        assert_eq!(
            classify_detail(Some(&s), 200, br#"{"authenticated": false}"#, true),
            SuccessOutcome::BodyFail
        );
    }

    /// `exists: false` is the one operator that may legitimately succeed on a
    /// missing pointer.
    #[test]
    fn missing_pointer_with_exists_false_is_success() {
        let s = spec("body:\n  - pointer: /error\n    exists: false\n");
        assert_eq!(
            classify_detail(Some(&s), 200, br#"{"authenticated": false}"#, true),
            SuccessOutcome::Success
        );
    }

    /// `equals` and `in` on a missing pointer were already correct (covered here
    /// for completeness alongside the `notEquals` regression above).
    #[test]
    fn missing_pointer_with_equals_and_in_is_failure() {
        let s = spec("body:\n  - pointer: /code\n    equals: 0\n");
        assert_eq!(
            classify_detail(Some(&s), 200, br#"{"other": 1}"#, true),
            SuccessOutcome::BodyFail
        );
        let s = spec("body:\n  - pointer: /code\n    in: [1, 2]\n");
        assert_eq!(
            classify_detail(Some(&s), 200, br#"{"other": 1}"#, true),
            SuccessOutcome::BodyFail
        );
    }

    // ── MINOR: empty `statusCodes` rejected ──────────────────────────────────

    /// An explicit `statusCodes: []` silently means "any 2xx" (same as omitting
    /// the field) — a Helm template rendering an empty list would widen an auth
    /// allow-list with no admission signal. Reject it.
    #[test]
    fn empty_status_codes_rejected() {
        let s = spec("statusCodes: []\n");
        let err = validate_success(&s).expect("empty statusCodes must be rejected");
        assert!(err.contains("statusCodes"), "got: {err}");
    }

    #[test]
    fn omitted_status_codes_accepted() {
        let s = spec("body:\n  - pointer: /code\n    equals: 0\n");
        assert!(validate_success(&s).is_none());
    }

    #[test]
    fn non_empty_status_codes_accepted() {
        let s = spec("statusCodes: [200, 201]\n");
        assert!(validate_success(&s).is_none());
    }
}
