//! Per-request resolve helpers for rule-level ExtensionRefs and request-wide
//! WAF owner selection.
//!
//! Global plugin references and Gateway-level plugin keys are not resolved
//! here: they are prepared once per publication
//! (`PluginScope::resolved_globals`, `GatewayScope::resolved_http`) and the
//! request path only clones those slices.
//! Dangling ExtensionRefs are kept as a `None` placeholder; they are
//! intentionally NOT logged here because this is the data-plane hot path,
//! where `tracing` is forbidden (see `skills/03-coding/01-log-safety.md`
//! Iron Rule 3 — a single misconfigured ref would otherwise emit one warn per
//! request, scaling the log volume with QPS).

use std::sync::Arc;

use crate::ctx::{WafLogSource, WafSelection};
use crate::plugins::publication::PluginScope;
use crate::plugins::runtime::PluginRuntime;
use crate::plugins::runtime::WafCandidate;

/// Resolve a rule's ExtensionRef keys (built by
/// `PluginRuntime::ext_ref_keys()`) to positional Vec of resolved runtimes.
///
/// `None` slots are dangling refs; position is preserved because
/// `ExtensionRefFilter.index` directly indexes into this Vec. Dangling refs
/// are not logged here — this runs on the data-plane hot path.
///
/// Takes the captured `&PluginScope` so all references share the same policy within a
/// request and share it across multiple resolve calls.
pub(crate) fn resolve_route_ext_refs(store: &PluginScope, keys: &[Option<String>]) -> Vec<Option<Arc<PluginRuntime>>> {
    resolve_route_ext_refs_with(keys, |key| store.http_runtime(key))
}

pub(crate) fn resolve_route_ext_refs_with(
    keys: &[Option<String>],
    mut lookup: impl FnMut(&str) -> Option<Arc<PluginRuntime>>,
) -> Vec<Option<Arc<PluginRuntime>>> {
    keys.iter().map(|key| key.as_deref().and_then(&mut lookup)).collect()
}

#[derive(Clone, Copy)]
enum WafScope {
    Global = 0,
    Gateway = 1,
    Route = 2,
}

/// Select one request-wide WAF without allocating a candidate list.
///
/// The more specific scope wins. Within one scope, higher configured priority
/// wins; equal priority retains the first declaration.
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
            scope as u8 > *selected_scope as u8
                || (scope as u8 == *selected_scope as u8 && candidate.priority() > *priority)
        });
        if replace {
            winner = Some((candidate.priority(), scope, candidate.mode(), candidate));
        }
    }
    winner.map(|(priority, scope, mode, candidate)| {
        let source = match scope {
            WafScope::Global => WafLogSource::Global,
            WafScope::Gateway => WafLogSource::Gateway,
            WafScope::Route => WafLogSource::Route,
        };
        WafSelection::new(candidate.owner(), source, priority, mode)
    })
}

/// One level's WAF candidate: highest priority, ties keep the first declaration.
#[inline]
fn level_waf_candidate(refs: &[Option<Arc<PluginRuntime>>]) -> Option<&WafCandidate> {
    let mut winner: Option<&WafCandidate> = None;
    for candidate in refs
        .iter()
        .filter_map(Option::as_deref)
        .filter_map(PluginRuntime::waf_candidate)
    {
        if winner.is_none_or(|selected| candidate.priority() > selected.priority()) {
            winner = Some(candidate);
        }
    }
    winner
}

/// Select one request-wide WAF for a delegated chain without allocating.
///
/// `levels` yields each level's resolved route ExtensionRefs, root first and
/// the leaf last. Priority is compared only within one level. Walking root to
/// leaf, the outermost level whose candidate declares `parent` precedence
/// wins; otherwise the deepest level with a candidate wins. Any Route-scope
/// winner beats Gateway and Global exactly as in [`select_waf_owner`], which
/// handles the request when no level declares a WAF.
pub(crate) fn select_waf_owner_chain<'a>(
    global: &[Arc<PluginRuntime>],
    gateway: &[Arc<PluginRuntime>],
    levels: impl Iterator<Item = &'a [Option<Arc<PluginRuntime>>]>,
) -> Option<WafSelection> {
    let mut route_winner: Option<&WafCandidate> = None;
    for refs in levels {
        if let Some(candidate) = level_waf_candidate(refs) {
            route_winner = Some(candidate);
            if candidate.takes_parent_precedence() {
                break;
            }
        }
    }
    match route_winner {
        Some(candidate) => Some(WafSelection::new(
            candidate.owner(),
            WafLogSource::Route,
            candidate.priority(),
            candidate.mode(),
        )),
        None => select_waf_owner(global, gateway, &[]),
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/global_plugins_resolve_test.rs"]
mod tests;
