//! Per-request resolve helpers for global plugin refs, Gateway-level plugin
//! keys, and rule-level ExtensionRefs.
//!
//! All helpers are pure: input refs/keys -> output Vec. They are called
//! from `pg_request_filter` once per request to populate the lazily allocated
//! plugin-runtime snapshot state.
//! Dangling references are silently dropped (global/gateway refs) or kept as a
//! `None` placeholder (ext refs); they are intentionally NOT logged here
//! because this is the data-plane hot path, where `tracing` is forbidden
//! (see `skills/03-coding/01-log-safety.md` Iron Rule 3 — a single
//! misconfigured ref would otherwise emit one warn per request, scaling
//! the log volume with QPS).

use std::sync::Arc;

use crate::ctx::{WafLogSource, WafSelection};
use crate::plugins::publication::PluginScope;
use crate::plugins::runtime::PluginRuntime;
#[cfg(test)]
use edgion_resources::resources::edgion_gateway_config::PluginReference;

/// Resolve the global references precomputed for this captured class scope.
pub(crate) fn resolve_global_plugins(scope: &PluginScope) -> Vec<Arc<PluginRuntime>> {
    resolve_gateway_plugins(scope, &scope.global_keys)
}

/// Resolve a rule's ExtensionRef keys (built by
/// `PluginRuntime::ext_ref_keys()`) to positional Vec of resolved runtimes.
///
/// `None` slots are dangling refs; position is preserved because
/// `ExtensionRefFilter.index` directly indexes into this Vec. Dangling refs
/// are not logged here — this runs on the data-plane hot path.
///
/// Takes the captured `&PluginScope` so all references share the same policy within a
/// request and share it across multiple resolve calls.
pub(crate) fn resolve_route_ext_refs(
    store: &PluginScope,
    keys: &[(String, String)],
) -> Vec<Option<Arc<PluginRuntime>>> {
    let mut out = Vec::with_capacity(keys.len());
    for (ns, name) in keys {
        let key = format!("{}/{}", ns, name);
        out.push(store.http(&key).map(|compiled| Arc::clone(&compiled.plugin_runtime)));
    }
    out
}

/// Resolve already-formed "namespace/name" store keys (from a Gateway's
/// `edgion.io/edgion-plugins` annotation) into plugin runtimes, preserving order and
/// dropping dangling refs (hot path: no logging).
pub(crate) fn resolve_gateway_plugins(store: &PluginScope, keys: &[String]) -> Vec<Arc<PluginRuntime>> {
    let mut out = Vec::with_capacity(keys.len());
    for key in keys {
        if let Some(rt) = store.http(key).map(|compiled| Arc::clone(&compiled.plugin_runtime)) {
            out.push(rt);
        }
    }
    out
}

#[derive(Clone, Copy)]
enum WafScope {
    Global = 0,
    Gateway = 1,
    Route = 2,
}

/// Select one request-wide WAF without allocating a candidate list.
///
/// Higher configured priority wins. Equal priority prefers the more specific
/// scope; equal priority and scope retains the first declaration.
pub(crate) fn select_waf_owner(
    global: &[Arc<PluginRuntime>],
    gateway: &[Arc<PluginRuntime>],
    route: &[Option<Arc<PluginRuntime>>],
) -> Option<WafSelection> {
    let mut winner = None;
    for (scope, runtime) in global
        .iter()
        .map(|runtime| (WafScope::Global, runtime.as_ref()))
        .chain(gateway.iter().map(|runtime| (WafScope::Gateway, runtime.as_ref())))
        .chain(
            route
                .iter()
                .filter_map(Option::as_deref)
                .map(|runtime| (WafScope::Route, runtime)),
        )
    {
        let Some(candidate) = runtime.waf_candidate() else {
            continue;
        };
        let replace = winner.as_ref().is_none_or(|(priority, selected_scope, _, _)| {
            candidate.priority() > *priority
                || (candidate.priority() == *priority && scope as u8 > *selected_scope as u8)
        });
        if replace {
            winner = Some((candidate.priority(), scope, candidate.mode(), candidate.owner()));
        }
    }
    winner.map(|(priority, scope, mode, owner)| {
        let source = match scope {
            WafScope::Global => WafLogSource::Global,
            WafScope::Gateway => WafLogSource::Gateway,
            WafScope::Route => WafLogSource::Route,
        };
        WafSelection::new(owner, source, priority, mode)
    })
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/global_plugins_resolve_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/global_plugins_resolve_test/gateway_plugins_resolve_tests.rs"]
mod gateway_plugins_resolve_tests;
