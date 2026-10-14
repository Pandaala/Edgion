//! Fleet-wide registry of `unknown_fields` findings for `EdgionPlugins`,
//! `EdgionStreamPlugins`, `EdgionConfigData`, and `LinkSys`, read live by
//! `GET /api/v1/diagnostics/unknown-fields`
//! (`api/diagnostics_handlers.rs`).
//!
//! `unknown_fields` is `#[serde(skip)]` on the resource specs (deliberately;
//! see `skills/01-architecture/01-controller/02-admin-api.md`),
//! so it is invisible through `AdminState::center_list_resources`, which
//! re-serializes the typed object to JSON. This module reads the typed
//! `RawStore` directly instead, mirroring
//! `observe::status_metrics::register_status_renderer` closely enough that
//! the two are registered side by side:
//!
//! - Same three call sites (`conf_center/{file_system,etcd,kubernetes}/resource_controller.rs`),
//!   right next to the existing `register_status_renderer` call, each already
//!   holding the `Arc<dyn RawStore<K>>` that call needs.
//! - Same re-registration contract: keyed by `kind`, so a conf_center reload
//!   (`FileSystemController` / `EtcdController` / `KubernetesController`
//!   reconstruction) REPLACES the prior closure rather than accumulating a
//!   duplicate — see `status_metrics.rs:103` for the fuller rationale, pinned
//!   here by `re_registering_same_kind_replaces_old_source` below.
//!
//! Unlike `status_metrics`, there is no background snapshot loop: this is a
//! live, on-demand query (`collect_unknown_fields`), read directly by the
//! diagnostics handler on each request. The fleet is expected to be small
//! enough, and the call infrequent enough (an operator or CI asking a
//! point-in-time question), that a full live scan is cheap and avoids a
//! second background task plus its own staleness window.
//!
//! `K` is generic at the three call sites, so the concrete supported type is
//! reached via `Any` downcast — the same idiom
//! `conf_mgr::conf_center::common::populate_unknown_fields_non_fatal` uses at
//! the analogous generic seam (FileSystem/etcd loader).

use parking_lot::Mutex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

use crate::conf_mgr::sync_runtime::resource_processor::raw_store::RawStore;

/// One resource's unknown-field findings, ready for the diagnostics response.
#[derive(Debug, Clone, Serialize)]
pub struct UnknownFieldsEntry {
    pub kind: &'static str,
    pub namespace: String,
    pub name: String,
    pub paths: Vec<String>,
}

/// Per-kind findings source closure signature: scans one kind's store and
/// pushes an entry for every resource carrying at least one finding.
type Source = Box<dyn Fn(&mut BoundedCollector) + Send + Sync>;

/// Maximum number of concrete paths exposed for one resource. The final
/// element is a deterministic marker when additional paths were omitted.
pub(crate) const MAX_PATHS_PER_RESOURCE: usize = 20;
pub(crate) const MAX_UNKNOWN_FIELD_PATH_BYTES: usize = 512;

pub(crate) fn bounded_unknown_field_path(path: &str) -> String {
    if path.len() <= MAX_UNKNOWN_FIELD_PATH_BYTES {
        return path.to_string();
    }
    // Reserve ample room for the ASCII marker; keeping this fixed avoids a
    // length-dependent loop while still guaranteeing the final value is
    // below the byte cap for every usize width.
    let mut end = MAX_UNKNOWN_FIELD_PATH_BYTES.saturating_sub(32).min(path.len());
    while !path.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...(+{} bytes)", &path[..end], path.len() - end)
}

fn bounded_resource_identity(identity: &str) -> String {
    if identity.len() <= MAX_UNKNOWN_FIELD_PATH_BYTES {
        return identity.to_string();
    }

    let digest = hex::encode(Sha256::digest(identity.as_bytes()));
    let marker = format!("...({} bytes; sha256:{digest})", identity.len());
    let mut end = MAX_UNKNOWN_FIELD_PATH_BYTES
        .saturating_sub(marker.len())
        .min(identity.len());
    while !identity.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &identity[..end], marker)
}

pub(crate) fn bounded_unknown_field_paths(paths: &[String]) -> Vec<String> {
    let shown_len = paths.len().min(MAX_PATHS_PER_RESOURCE);
    let mut shown: Vec<String> = paths[..shown_len]
        .iter()
        .map(|path| bounded_unknown_field_path(path))
        .collect();
    if paths.len() > shown_len {
        shown.push(format!("(+{} more)", paths.len() - shown_len));
    }
    shown
}

struct BoundedCollector {
    cap: usize,
    total: usize,
    by_kind: BTreeMap<&'static str, usize>,
    items: Vec<UnknownFieldsEntry>,
}

impl BoundedCollector {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            total: 0,
            by_kind: BTreeMap::new(),
            items: Vec::with_capacity(cap.min(1024)),
        }
    }

    fn push(&mut self, entry: UnknownFieldsEntry) {
        self.total += 1;
        *self.by_kind.entry(entry.kind).or_default() += 1;
        if self.cap == 0 {
            return;
        }
        let key = (entry.kind, entry.namespace.as_str(), entry.name.as_str());
        let index = self
            .items
            .binary_search_by(|existing| (existing.kind, existing.namespace.as_str(), existing.name.as_str()).cmp(&key))
            .unwrap_or_else(|index| index);
        if self.items.len() < self.cap {
            self.items.insert(index, entry);
        } else if index < self.cap {
            self.items.insert(index, entry);
            self.items.pop();
        }
    }
}

/// Registered per-kind sources, keyed by kind name so a second registration
/// for the same kind replaces (never appends to) the first.
fn sources_slot() -> &'static Mutex<HashMap<&'static str, Source>> {
    static SOURCES: OnceLock<Mutex<HashMap<&'static str, Source>>> = OnceLock::new();
    SOURCES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register the per-kind unknown-fields source. Called from the same three
/// resource_controller call sites as `status_metrics::register_status_renderer`,
/// right after the `RawStore` is wired.
///
/// A no-op for every kind except `EdgionPlugins`, `EdgionStreamPlugins`, and
/// `EdgionConfigData`, and `LinkSys` — the resources carrying an `unknown_fields` field — so the other
/// ~20 kinds pay only a string compare and never occupy a registry slot.
pub(crate) fn register_unknown_fields_source<K>(kind: &'static str, store: Arc<dyn RawStore<K>>)
where
    K: Send + Sync + 'static,
{
    let source: Source = match kind {
        "EdgionPlugins" => Box::new(move |out: &mut BoundedCollector| {
            for obj in store.snapshot() {
                let any: Arc<dyn Any + Send + Sync> = obj;
                let Ok(plugins) = any.downcast::<edgion_resources::EdgionPlugins>() else {
                    continue;
                };
                if !plugins.spec.unknown_fields.is_empty() {
                    out.push(UnknownFieldsEntry {
                        kind: "EdgionPlugins",
                        namespace: bounded_resource_identity(plugins.metadata.namespace.as_deref().unwrap_or_default()),
                        name: bounded_resource_identity(plugins.metadata.name.as_deref().unwrap_or_default()),
                        paths: bounded_unknown_field_paths(&plugins.spec.unknown_fields),
                    });
                }
            }
        }),
        "EdgionStreamPlugins" => Box::new(move |out: &mut BoundedCollector| {
            for obj in store.snapshot() {
                let any: Arc<dyn Any + Send + Sync> = obj;
                let Ok(plugins) = any.downcast::<edgion_resources::EdgionStreamPlugins>() else {
                    continue;
                };
                if !plugins.spec.unknown_fields.is_empty() {
                    out.push(UnknownFieldsEntry {
                        kind: "EdgionStreamPlugins",
                        namespace: bounded_resource_identity(plugins.metadata.namespace.as_deref().unwrap_or_default()),
                        name: bounded_resource_identity(plugins.metadata.name.as_deref().unwrap_or_default()),
                        paths: bounded_unknown_field_paths(&plugins.spec.unknown_fields),
                    });
                }
            }
        }),
        "EdgionConfigData" => Box::new(move |out: &mut BoundedCollector| {
            for obj in store.snapshot() {
                let any: Arc<dyn Any + Send + Sync> = obj;
                let Ok(config_data) = any.downcast::<edgion_resources::EdgionConfigData>() else {
                    continue;
                };
                if !config_data.spec.unknown_fields.is_empty() {
                    out.push(UnknownFieldsEntry {
                        kind: "EdgionConfigData",
                        namespace: bounded_resource_identity(
                            config_data.metadata.namespace.as_deref().unwrap_or_default(),
                        ),
                        name: bounded_resource_identity(config_data.metadata.name.as_deref().unwrap_or_default()),
                        paths: bounded_unknown_field_paths(&config_data.spec.unknown_fields),
                    });
                }
            }
        }),
        "LinkSys" => Box::new(move |out: &mut BoundedCollector| {
            for obj in store.snapshot() {
                let any: Arc<dyn Any + Send + Sync> = obj;
                let Ok(link_sys) = any.downcast::<edgion_resources::LinkSys>() else {
                    continue;
                };
                if !link_sys.spec.unknown_fields.is_empty() {
                    out.push(UnknownFieldsEntry {
                        kind: "LinkSys",
                        namespace: bounded_resource_identity(
                            link_sys.metadata.namespace.as_deref().unwrap_or_default(),
                        ),
                        name: bounded_resource_identity(link_sys.metadata.name.as_deref().unwrap_or_default()),
                        paths: bounded_unknown_field_paths(&link_sys.spec.unknown_fields),
                    });
                }
            }
        }),
        _ => return,
    };
    // Keyed by `kind`: a reload-time second call for the same kind drops the
    // prior closure (and with it the Arc to the old RawStore), matching
    // `register_status_renderer`'s re-registration contract.
    sources_slot().lock().insert(kind, source);
}

/// Run every registered source, collect findings, sort for determinism, and
/// cap the result. Returns `(total_before_cap, truncated, items)`.
///
/// Sorting mirrors the walker's own determinism rule: output must not depend on
/// `HashMap` iteration order or on `RawStore::snapshot`'s unspecified order.
pub(crate) fn collect_unknown_fields(cap: usize) -> (usize, bool, Vec<UnknownFieldsEntry>) {
    let mut collector = BoundedCollector::new(cap);
    {
        let sources = sources_slot().lock();
        for source in sources.values() {
            source(&mut collector);
        }
    }
    let truncated = collector.total > cap;
    (collector.total, truncated, collector.items)
}

// ---------------------------------------------------------------------
// `edgion_controller_unknown_config_fields` gauge.
//
// Deliberately built on the same bounded collector and live
// `spec.unknown_fields` sources as `collect_unknown_fields` above, NOT on
// `status_metrics::classify()`.
// `edgion_controller_resource_abnormal` is derived from resource STATUS
// CONDITIONS, which per the "Controller metrics" section of
// `docs/zh-CN/ops-guide/plugin-policy.md` makes that gauge Kubernetes-mode only:
// in FileSystem/etcd
// mode the scanner reads pre-processed objects that never carry status
// conditions at all. Our findings live on `spec.unknown_fields`, which
// the registered sources read straight from the typed `RawStore` in ALL
// THREE conf_center modes. Routing this metric through `classify()` would
// silently produce zero coverage in FileSystem/etcd mode — see
// `status_metrics.rs`'s `classify()` doc comment and the pin test
// `unknown_config_fields_true_produces_no_series`, both of which this
// metric must NOT disturb: it is a separate gauge, on purpose.
//
// Like `edgion_controller_resource_abnormal`, there is no background loop:
// both diagnostics and `/metrics` perform the same cheap live scan on demand.

pub(crate) const METRIC_NAME: &str = "edgion_controller_unknown_config_fields";
pub(crate) const METRIC_HELP: &str = "Resources whose submitted document contains a key the EdgionPlugins, EdgionStreamPlugins, EdgionConfigData, or LinkSys schema does not recognize, aggregated by resource kind. Detailed findings are available from GET /api/v1/diagnostics/unknown-fields. Advisory only; does not affect Accepted or block sync.";

type SampledGauges = [Option<edgion_common::observe::metrics_facade::FixedGauge>; 4];
static SAMPLED_GAUGES: std::sync::OnceLock<parking_lot::Mutex<SampledGauges>> = std::sync::OnceLock::new();
fn sampled_gauges() -> &'static parking_lot::Mutex<SampledGauges> {
    SAMPLED_GAUGES.get_or_init(|| parking_lot::Mutex::new(std::array::from_fn(|_| None)))
}

/// Sample the four closed kinds before Common renders. Untouched kinds remain
/// absent; an initialized kind publishes zero after recovery or source removal.
pub(crate) fn sample_unknown_config_fields_metric() {
    let mut collector = BoundedCollector::new(0);
    {
        let sources = sources_slot().lock();
        for source in sources.values() {
            source(&mut collector);
        }
    }
    use edgion_common::observe::metrics_catalog::{definition, LabelDomain};
    let def = definition(METRIC_NAME).expect("unknown-fields fixed metric definition");
    let LabelDomain::Closed(kinds) = def.labels[0].domain else {
        return;
    };
    let mut gauges = sampled_gauges().lock();
    for (index, kind) in kinds.iter().enumerate() {
        let count = collector.by_kind.get(kind).copied().unwrap_or(0);
        if count > 0 && gauges[index].is_none() {
            metrics::describe_gauge!(METRIC_NAME, METRIC_HELP);
            gauges[index] = Some(edgion_common::fixed_gauge!(METRIC_NAME,"kind"=>*kind));
        }
        if let Some(gauge) = &gauges[index] {
            gauge.set(count as f64);
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/observe/unknown_fields_registry_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/support/observe/unknown_fields_registry.rs"]
mod test_support;
#[cfg(test)]
use test_support::*;
