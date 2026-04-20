//! `edgion_controller_resource_abnormal` gauge.
//!
//! See `docs/superpowers/specs/2026-04-20-controller-resource-abnormal-metrics-design.md`
//! for the full design. Short version:
//!
//! * A background task rebuilds a fresh `Vec<Entry>` every `tick_secs` by
//!   reading every registered `RawStore`, serializing each resource to JSON,
//!   recursively scanning for `"conditions": [...]` arrays, and classifying
//!   into `Err` / `Warn` based on condition type.
//! * The snapshot is published atomically via `ArcSwap<Arc<Vec<Entry>>>`.
//! * `/metrics` is read-only; it loads the current snapshot and appends
//!   a text block to the common Prometheus handler output.
//! * No incremental state, no per-resource hooks — every tick fully
//!   replaces the Vec, so recovery and deletion are automatic.
//!
//! Cardinality: one series per abnormal resource at most. Hard safety cap
//! of 10 000 entries per snapshot (see [`SNAPSHOT_ENTRY_HARD_CAP`]).

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::OnceLock;

pub const METRIC_NAME: &str = "edgion_controller_resource_abnormal";
pub const METRIC_HELP: &str =
    "Resource currently has at least one False status condition. type=err for Accepted/ResolvedRefs/Programmed failures (or Conflicted/ListenersNotValid=True), type=warn for Ready:False or PartiallyInvalid:True.";

/// Safety ceiling: if a snapshot would exceed this, it is truncated and
/// a single `tracing::error!` is emitted. Hard-coded — not a config knob.
pub const SNAPSHOT_ENTRY_HARD_CAP: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Kind {
    Err,
    Warn,
}

impl Kind {
    #[inline]
    pub fn as_label(self) -> &'static str {
        match self {
            Kind::Err => "err",
            Kind::Warn => "warn",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    /// K8s `.kind` field (PascalCase singular), e.g. `HTTPRoute`.
    pub kind: &'static str,
    pub namespace: String,
    pub name: String,
    pub ty: Kind,
}

/// Renderer closure signature: pushes `Entry` values for one kind's store.
pub type Renderer = Box<dyn Fn(&mut Vec<Entry>) + Send + Sync>;

/// Global current snapshot. Read lock-free on every `/metrics` scrape;
/// replaced atomically at the end of each background tick.
pub fn snapshot_slot() -> &'static ArcSwap<Vec<Entry>> {
    static SNAPSHOT: OnceLock<ArcSwap<Vec<Entry>>> = OnceLock::new();
    SNAPSHOT.get_or_init(|| ArcSwap::from_pointee(Vec::new()))
}

/// Registered per-kind renderers. Append-only during controller startup;
/// each tick's loop body briefly locks to iterate.
pub fn renderers_slot() -> &'static Mutex<Vec<Renderer>> {
    static RENDERERS: OnceLock<Mutex<Vec<Renderer>>> = OnceLock::new();
    RENDERERS.get_or_init(|| Mutex::new(Vec::new()))
}
