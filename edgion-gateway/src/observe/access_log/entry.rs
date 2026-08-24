//! Access log entry definition

use crate::geoip::GeoLookup;
use crate::observe::access_log::log_ext::LogExt;
use crate::observe::RequestDiagnostic;
use crate::plugins::StageLogs;
use crate::routes::grpc::GrpcMatchInfo;
use crate::{ai::GenAiSummary, BackendContext, EdgionHttpContext, MatchInfo, RequestInfo};
use serde::Serialize;

/// Explicit per-slot outcome marker. "Not queried" (no database loaded) and
/// "queried, no data" (a database is loaded but has no record for this
/// address) must not collapse into the same absence: downstream index
/// mappings and dashboards need a stable schema, and the two states drive
/// different operator conclusions ("the control is not running" vs. "this
/// client is in a private range").
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum GeoSlotStatus {
    Unavailable,
    NotFound,
    Found,
}

/// One resolved address's geo attributes for the access log.
///
/// `status` is always present; the dimension fields are present only when
/// `status == "found"` and the underlying `GeoInfo` field is `Some`.
///
/// Fields are owned `String`s, not `&str` borrows off the `GeoLookup`
/// argument: `GeoLookup::Found` carries an `Arc<GeoInfo>`, and cloning its
/// (at most four, short) string fields out is cheap — well short of the
/// "newly-allocated geo struct" the request-body/response-buffering
/// invariants warn against, and consistent with `request_uri` on this same
/// struct, which is likewise an owned `Option<String>` computed per request.
/// Owning also keeps `from_slots` usable with a short-lived `&GeoLookup`
/// (e.g. one built from a temporary in a test) rather than requiring the
/// caller's `GeoLookup` value to outlive the returned block.
#[derive(Serialize)]
struct GeoSlotLog {
    status: GeoSlotStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    continent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subdivision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    city: Option<String>,
    /// A single `"lat,lon"` string rather than two `f64` fields: the masking
    /// pipeline operates on strings, and this reuses the same `masked`
    /// serializer `direct_peer_ip`/`remote_ip` get. Present only when
    /// Present when the record carried both coordinates. Always masked through
    /// the process-global sensitive mode, like `direct_peer_ip` and `remote_ip`.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "edgion_resources::sensitive::masked_opt::serialize"
    )]
    coord: Option<String>,
}

impl GeoSlotLog {
    fn from_lookup(lookup: &GeoLookup) -> Self {
        match lookup {
            GeoLookup::Unavailable => Self {
                status: GeoSlotStatus::Unavailable,
                country: None,
                continent: None,
                subdivision: None,
                city: None,
                coord: None,
            },
            GeoLookup::NotFound => Self {
                status: GeoSlotStatus::NotFound,
                country: None,
                continent: None,
                subdivision: None,
                city: None,
                coord: None,
            },
            GeoLookup::Found(info) => Self {
                status: GeoSlotStatus::Found,
                country: info.country_iso.clone(),
                continent: info.continent.clone(),
                subdivision: info.subdivision.clone(),
                city: info.city.clone(),
                coord: match (info.latitude, info.longitude) {
                    (Some(lat), Some(lon)) => Some(format!("{lat},{lon}")),
                    _ => None,
                },
            },
        }
    }
}

/// Resolved geo attributes, reporting only the slots a plugin actually
/// resolved during the request.
///
/// Reading the slots must NEVER force a lookup: the geo slots are lazy by
/// design so a route with no geo rule pays nothing, and building an access-log
/// entry is not a reason to query the database. A slot nobody consulted is
/// simply absent from the output.
#[derive(Serialize)]
pub struct GeoLogBlock {
    #[serde(skip_serializing_if = "Option::is_none")]
    remote: Option<GeoSlotLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    direct_peer: Option<GeoSlotLog>,
}

impl GeoLogBlock {
    /// `None` when neither slot was resolved, so the `geo` key is omitted
    /// entirely rather than emitted empty.
    pub fn from_slots(remote: Option<&GeoLookup>, direct_peer: Option<&GeoLookup>) -> Option<Self> {
        if remote.is_none() && direct_peer.is_none() {
            return None;
        }
        Some(Self {
            remote: remote.map(GeoSlotLog::from_lookup),
            direct_peer: direct_peer.map(GeoSlotLog::from_lookup),
        })
    }
}

fn is_empty<T>(slice: &&[T]) -> bool {
    slice.is_empty()
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Unified match info that covers both HTTP and gRPC routes in access logs.
#[derive(Serialize)]
#[serde(untagged)]
pub enum RouteMatchInfo<'a> {
    Http(&'a MatchInfo),
    Grpc(&'a GrpcMatchInfo),
}

#[derive(Serialize)]
pub struct AccessLogEntry<'a> {
    #[serde(rename = "ts")]
    pub timestamp: i64,

    pub request_info: &'a RequestInfo,

    /// Entry-time raw URI (raw path + raw query). Populated only when
    /// `[access_log_options] include_raw_uri = true`. Carries the exact URI
    /// the client sent, before any path normalization.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "edgion_resources::sensitive::masked_opt::serialize"
    )]
    pub request_uri: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_info: Option<RouteMatchInfo<'a>>,

    #[serde(skip_serializing_if = "is_empty")]
    pub diagnostics: &'a [RequestDiagnostic],

    #[serde(skip_serializing_if = "is_false")]
    pub diagnostics_full: bool,

    pub backend_context: Option<&'a BackendContext>,

    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub stage_logs: &'a [StageLogs],

    #[serde(skip_serializing_if = "Option::is_none")]
    pub conn_est: Option<bool>,

    /// Composed upstream authority (Host + SNI) when the target Service carries an
    /// `upstreamAuthority` policy; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_authority: Option<&'a str>,

    /// Final AI usage and routing summary. Model-level detail belongs here,
    /// not in Prometheus labels. The type cannot carry request/response text or
    /// provider credentials.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genai: Option<&'a GenAiSummary>,

    /// Resolved geo attributes for the client and direct-peer addresses.
    /// Present only when a plugin (or L4 predicate) resolved at least one geo
    /// slot during the request; absent otherwise. Building this entry never
    /// triggers a lookup, so an unresolved slot stays unresolved rather than
    /// being forced here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geo: Option<GeoLogBlock>,

    /// Operator-declared fields (`AccessLogExtern`), nested one level deep.
    ///
    /// Nested rather than flattened: the top-level namespace holds reserved
    /// access-log fields such as `diagnostics`; nesting prevents an operator
    /// field from creating duplicate JSON keys under a last-wins parser.
    #[serde(skip_serializing_if = "LogExt::is_empty")]
    pub ext: LogExt,
}

impl<'a> AccessLogEntry<'a> {
    #[inline]
    pub fn from_context(ctx: &'a EdgionHttpContext) -> Self {
        let match_info = if let Some(ru) = ctx.route_unit.as_ref() {
            Some(RouteMatchInfo::Http(&ru.matched_info))
        } else {
            ctx.grpc_route_unit
                .as_ref()
                .map(|gru| RouteMatchInfo::Grpc(&gru.matched_info))
        };

        let request_uri = if crate::cli::config::get_access_log_options().include_raw_uri
            && !ctx.request_info.entry_raw_path.is_empty()
        {
            Some(ctx.request_uri())
        } else {
            None
        };

        // `.get()`, never the accessors: the accessors are `get_or_init` and
        // would make every request pay a lookup just to build its log line.
        let (remote_geo, direct_peer_geo) = ctx.request_info.resolved_geo();
        let geo = GeoLogBlock::from_slots(remote_geo, direct_peer_geo);

        Self {
            timestamp: chrono::Utc::now().timestamp_millis(),
            request_info: &ctx.request_info,
            request_uri,
            match_info,
            diagnostics: ctx.diagnostics.entries(),
            diagnostics_full: ctx.diagnostics.is_full(),
            backend_context: ctx.backend_context.as_ref(),
            stage_logs: &ctx.stage_logs,
            conn_est: None,
            upstream_authority: ctx.upstream_authority.as_deref(),
            genai: ctx.ai_state().and_then(|state| state.summary.as_ref()),
            geo,
            ext: LogExt::default(),
        }
    }

    /// Set connection established flag
    #[inline]
    pub fn set_conn_est(&mut self) {
        self.conn_est = Some(true);
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| access_log_fallback(chrono::Utc::now().timestamp_millis()))
    }
}

fn access_log_fallback(timestamp: i64) -> String {
    format!(r#"{{"ts":{timestamp},"diagnostics":[{{"kind":"error","code":"AccessLogSerializeFailed"}}]}}"#)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geoip::GeoInfo;
    use edgion_resources::{EdgionErrorCode, EdgionInfoCode};

    #[test]
    fn empty_diagnostics_and_full_marker_are_omitted() {
        let json = AccessLogEntry::from_context(&EdgionHttpContext::new()).to_json();
        assert!(!json.contains(r#""diagnostics""#), "got: {json}");
        assert!(!json.contains(r#""diagnostics_full""#), "got: {json}");
    }

    #[test]
    fn diagnostic_overflow_emits_the_full_marker_and_only_kind_and_code() {
        let mut ctx = EdgionHttpContext::new();
        for _ in 0..=crate::observe::request_diagnostics::MAX_REQUEST_DIAGNOSTICS {
            ctx.record_error(EdgionErrorCode::Unknown);
        }

        let value: serde_json::Value = serde_json::from_str(&AccessLogEntry::from_context(&ctx).to_json()).unwrap();
        let diagnostics = value["diagnostics"].as_array().unwrap();
        assert_eq!(
            diagnostics.len(),
            crate::observe::request_diagnostics::MAX_REQUEST_DIAGNOSTICS
        );
        assert_eq!(value["diagnostics_full"], true);
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic.as_object().is_some_and(|object| {
                object.len() == 2
                    && object.get("kind") == Some(&serde_json::json!("error"))
                    && object.get("code") == Some(&serde_json::json!("Unknown"))
            })
        }));
    }

    #[test]
    fn serialization_fallback_uses_the_diagnostic_schema() {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&access_log_fallback(123)).unwrap(),
            serde_json::json!({
                "ts": 123,
                "diagnostics": [{"kind": "error", "code": "AccessLogSerializeFailed"}]
            })
        );
    }

    #[test]
    fn geo_block_is_omitted_when_no_geo_lookup_ran() {
        let ctx = EdgionHttpContext::default();
        let entry = AccessLogEntry::from_context(&ctx);
        assert!(!entry.to_json().contains("\"geo\""));
    }

    /// A slot nobody resolved is ABSENT, which is what distinguishes it from a
    /// slot that was resolved and found nothing. Collapsing the two would make
    /// "no plugin asked" indistinguishable from "asked, database had no record".
    #[test]
    fn unresolved_slot_is_absent_while_resolved_slots_report_their_status() {
        let block =
            GeoLogBlock::from_slots(Some(&GeoLookup::NotFound), None).expect("one resolved slot must produce a block");
        let json = serde_json::to_string(&block).unwrap();
        assert!(
            json.contains("not_found"),
            "resolved slot must report its status: {json}"
        );
        assert!(
            !json.contains("direct_peer"),
            "unresolved slot must be absent entirely: {json}"
        );
    }

    /// Unavailable (no database at all) and NotFound (database loaded, no
    /// record) stay distinct in the log, as they do in `GeoLookup`.
    #[test]
    fn unavailable_and_not_found_remain_distinguishable() {
        let block = GeoLogBlock::from_slots(Some(&GeoLookup::Unavailable), Some(&GeoLookup::NotFound))
            .expect("resolved slots must produce a block");
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("unavailable"));
        assert!(json.contains("not_found"));
    }

    /// Building an access-log entry must never force a lookup, so a request
    /// whose plugins never consulted geo emits no `geo` key at all.
    #[test]
    fn no_resolved_slot_yields_no_block() {
        assert!(GeoLogBlock::from_slots(None, None).is_none());
        assert!(GeoLogBlock::from_slots(None, None).is_none());
    }

    #[test]
    fn coordinates_are_emitted_masked_when_the_record_carries_them() {
        let with_coords = std::sync::Arc::new(GeoInfo {
            country_iso: Some("CN".into()),
            latitude: Some(39.9),
            longitude: Some(116.4),
            ..Default::default()
        });
        let block = GeoLogBlock::from_slots(Some(&GeoLookup::Found(with_coords)), None).unwrap();
        let json = serde_json::to_string(&block).unwrap();
        assert!(
            json.contains("coord"),
            "a record carrying coordinates must emit coord: {json}"
        );
        // Whether the value renders in clear is governed by the process-global
        // `SENSITIVE_MODE`, which is Off in unit tests — so this asserts the field
        // is emitted, not its masked rendering. The masking itself belongs to the
        // shared `masked` serializer and is covered where that mode is exercised.

        // A record with no coordinates emits no `coord` key at all — there is no
        // switch to turn them off, so absence must mean "the record had none".
        let without = std::sync::Arc::new(GeoInfo {
            country_iso: Some("CN".into()),
            ..Default::default()
        });
        let block = GeoLogBlock::from_slots(Some(&GeoLookup::Found(without)), None).unwrap();
        assert!(!serde_json::to_string(&block).unwrap().contains("coord"));
    }

    #[test]
    fn access_log_includes_acme_information_diagnostic() {
        let mut ctx = EdgionHttpContext::new();
        ctx.record_info(EdgionInfoCode::AcmeChallengeServed);

        let json = AccessLogEntry::from_context(&ctx).to_json();

        assert!(
            json.contains(r#""diagnostics":[{"kind":"info","code":"AcmeChallengeServed"}]"#),
            "access log must include ACME local response status, got: {json}"
        );
        assert!(
            !json.contains(r#""kind":"error""#),
            "ACME local response should not be reported as an error, got: {json}"
        );
    }

    #[test]
    fn ctx_map_never_reaches_the_log_without_a_declaration() {
        // The annotation allowlist is gone: ctx vars reach the log only through
        // an AccessLogExtern `from: ctx` declaration, which populates `ext`.
        let mut ctx = EdgionHttpContext::new();
        ctx.set_ctx_var("user_tier".to_string(), "gold".to_string());
        ctx.set_ctx_var("jwt_claims".to_string(), r#"{"sub":"alice"}"#.to_string());

        let json = AccessLogEntry::from_context(&ctx).to_json();

        assert!(!json.contains("user_tier") && !json.contains("gold"), "got: {json}");
        assert!(!json.contains("jwt_claims") && !json.contains("alice"), "got: {json}");
        assert!(!json.contains(r#""ext""#), "empty ext must be omitted, got: {json}");
    }

    #[test]
    fn upstream_authority_is_logged_only_when_present() {
        let mut ctx = EdgionHttpContext::new();
        assert!(
            !AccessLogEntry::from_context(&ctx)
                .to_json()
                .contains("upstream_authority"),
            "absent authority must be omitted from the access log"
        );

        ctx.upstream_authority = Some("tenant-a.sandbox.internal".to_string());
        let json = AccessLogEntry::from_context(&ctx).to_json();
        assert!(json.contains("tenant-a.sandbox.internal"), "got: {json}");
    }
}
