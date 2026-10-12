//! Access log entry definition

use crate::geoip::GeoLookup;
use crate::observe::access_log::log_ext::LogExt;
use crate::observe::RequestDiagnostic;
use crate::plugins::StageLogs;
use crate::routes::grpc::GrpcMatchInfo;
use crate::{ai::GenAiSummary, ctx::WafAccessLog, BackendContext, EdgionHttpContext, MatchInfo, RequestInfo};
use serde::Serialize;

/// Explicit per-slot outcome marker. "Not queried" (no database loaded),
/// "queried, no data" (a database is loaded but has no record for this
/// address), and "queried, lookup failed" (decode error or caught reader
/// panic) must not collapse into the same absence: downstream index
/// mappings and dashboards need a stable schema, and the three states drive
/// different operator conclusions.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum GeoSlotStatus {
    Unavailable,
    NotFound,
    LookupFailed,
    Found,
}

/// One resolved address's geo attributes for the access log.
///
/// `status` is always present; the dimension fields are present only when
/// `status == "found"` and the underlying `GeoInfo` field is `Some`.
///
/// Dimensions borrow the immutable facts retained by the request's lookup
/// slot. The slot must outlive this projection; serialization produces an
/// owned JSON string before that string is handed to an asynchronous sink.
/// Only the computed coordinate string needs separate ownership.
#[derive(Serialize)]
struct GeoSlotLog<'a> {
    status: GeoSlotStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    country: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    continent: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subdivision: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    city: Option<&'a str>,
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

impl<'a> GeoSlotLog<'a> {
    fn from_lookup(lookup: &'a GeoLookup) -> Self {
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
            GeoLookup::Failed => Self {
                status: GeoSlotStatus::LookupFailed,
                country: None,
                continent: None,
                subdivision: None,
                city: None,
                coord: None,
            },
            GeoLookup::Found(info) => Self {
                status: GeoSlotStatus::Found,
                country: info.country_iso.as_deref(),
                continent: info.continent.as_deref(),
                subdivision: info.subdivision.as_deref(),
                city: info.city.as_deref(),
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
pub struct GeoLogBlock<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    remote: Option<GeoSlotLog<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    direct_peer: Option<GeoSlotLog<'a>>,
}

impl<'a> GeoLogBlock<'a> {
    /// `None` when neither slot was resolved, so the `geo` key is omitted
    /// entirely rather than emitted empty.
    pub fn from_slots(remote: Option<&'a GeoLookup>, direct_peer: Option<&'a GeoLookup>) -> Option<Self> {
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

    /// Entry-time path and query. Populated only when
    /// `access_log_options.includeRawUri: true`. Excludes the scheme and
    /// authority of an HTTP/1 absolute-form request target.
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

    /// Final native WAF decision summary. This field is deliberately attached
    /// only by the terminal logging hook: intermediate/debug access-log views
    /// must not expose rule identifiers or the active protection posture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waf: Option<&'a WafAccessLog>,

    /// Resolved geo attributes for the client and direct-peer addresses.
    /// Present only when a plugin (or L4 predicate) resolved at least one geo
    /// slot during the request; absent otherwise. Building this entry never
    /// triggers a lookup, so an unresolved slot stays unresolved rather than
    /// being forced here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geo: Option<GeoLogBlock<'a>>,

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
            stage_logs: ctx.stage_logs(),
            conn_est: None,
            upstream_authority: ctx.upstream_authority.as_deref(),
            genai: ctx.ai_state().and_then(|state| state.summary.as_ref()),
            waf: None,
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
#[path = "../../../tests/unit/observe/access_log/entry_test.rs"]
mod tests;
