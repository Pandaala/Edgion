//! Class-specific metric interfaces. Fixed handles persist; dynamic handles are
//! renewable proxies and never retain an exporter accumulation epoch.
use super::metrics_catalog::{definition, validate, MetricClass, MetricType};
use metrics::{Counter, Gauge, Histogram, Key, Level, Metadata};
use std::sync::OnceLock;

/// A prepared metric identity does not match its declared class, kind or label domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetricDefinitionError;
impl std::fmt::Display for MetricDefinitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid metric definition or labels")
    }
}
impl std::error::Error for MetricDefinitionError {}

const METADATA: Metadata<'static> = Metadata::new(module_path!(), Level::INFO, None);
fn ready() -> bool {
    super::metrics_api::recorder_installed() || super::metrics_api::local_recorder_active()
}

macro_rules! wrapper {
    ($name:ident,$raw:ty,$kind:ident,$class:ident,$register:ident) => {
        pub struct $name {
            key: Option<Key>,
            handle: OnceLock<$raw>,
        }
        impl $name {
            /// Convenience event interfaces fail closed without affecting business execution.
            pub fn new(key: Key) -> Self {
                Self::try_new(key).unwrap_or_else(|_| Self {
                    key: None,
                    handle: OnceLock::new(),
                })
            }
            /// Prepare a descriptor without admitting a dynamic accumulation epoch.
            pub fn try_new(key: Key) -> Result<Self, MetricDefinitionError> {
                let valid = validate(&key, MetricType::$kind, MetricClass::$class)
                    && !(MetricClass::$class == MetricClass::Dynamic
                        && MetricType::$kind == MetricType::Gauge
                        && definition(key.name()).is_some_and(|def| def.contribution));
                if !valid {
                    return Err(MetricDefinitionError);
                }
                let result = Self {
                    key: Some(key),
                    handle: OnceLock::new(),
                };
                let _ = result.acquire();
                Ok(result)
            }
            fn acquire(&self) -> $raw {
                if let Some(handle) = self.handle.get() {
                    return handle.clone();
                }
                let Some(key) = &self.key else {
                    return <$raw>::noop();
                };
                let register = || metrics::with_recorder(|recorder| recorder.$register(key, &METADATA));
                // Pre-install calls must never freeze the global no-op recorder.
                if ready() {
                    self.handle.get_or_init(register).clone()
                } else {
                    register()
                }
            }
        }
        impl Clone for $name {
            fn clone(&self) -> Self {
                let handle = OnceLock::new();
                if let Some(current) = self.handle.get() {
                    let _ = handle.set(current.clone());
                }
                Self {
                    key: self.key.clone(),
                    handle,
                }
            }
        }
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    };
}
wrapper!(FixedCounter, Counter, Counter, Fixed, register_counter);
wrapper!(FixedGauge, Gauge, Gauge, Fixed, register_gauge);
wrapper!(DynamicCounter, Counter, Counter, Dynamic, register_counter);
wrapper!(DynamicGauge, Gauge, Gauge, Dynamic, register_gauge);
wrapper!(DynamicHistogram, Histogram, Histogram, Dynamic, register_histogram);
impl FixedCounter {
    #[inline]
    pub fn increment(&self, value: u64) {
        if let Some(handle) = self.handle.get() {
            handle.increment(value);
        } else {
            self.acquire().increment(value);
        }
    }
    #[inline]
    pub fn absolute(&self, value: u64) {
        if let Some(handle) = self.handle.get() {
            handle.absolute(value);
        } else {
            self.acquire().absolute(value);
        }
    }
}
impl FixedGauge {
    #[inline]
    pub fn increment(&self, value: f64) {
        if let Some(handle) = self.handle.get() {
            handle.increment(value);
        } else {
            self.acquire().increment(value);
        }
    }
    #[inline]
    pub fn decrement(&self, value: f64) {
        if let Some(handle) = self.handle.get() {
            handle.decrement(value);
        } else {
            self.acquire().decrement(value);
        }
    }
    #[inline]
    pub fn set(&self, value: f64) {
        if let Some(handle) = self.handle.get() {
            handle.set(value);
        } else {
            self.acquire().set(value);
        }
    }
}
impl DynamicCounter {
    #[inline]
    pub fn increment(&self, value: u64) {
        if let Some(handle) = self.handle.get() {
            handle.increment(value);
        } else {
            self.acquire().increment(value);
        }
    }
}
impl DynamicGauge {
    pub fn set(&self, value: f64) {
        self.acquire().set(value);
    }
}
impl DynamicHistogram {
    #[inline]
    pub fn record(&self, value: f64) {
        if let Some(handle) = self.handle.get() {
            handle.record(value);
        } else {
            self.acquire().record(value);
        }
    }
}

/// One authoritative owner's committed contribution. This token is deliberately
/// non-cloneable; overlapping resource generations acquire separate tokens.
pub struct GaugeContribution {
    key: Option<Key>,
    state: parking_lot::Mutex<ContributionState>,
}
#[derive(Default)]
struct ContributionState {
    committed: f64,
    owner_store: Option<std::sync::Arc<super::dynamic_metrics::DynamicStore>>,
    attached: Option<(
        std::sync::Arc<super::dynamic_metrics::DynamicStore>,
        std::sync::Arc<super::dynamic_metrics::Slot>,
    )>,
}
impl GaugeContribution {
    pub fn new(key: Key) -> Self {
        Self::try_new(key).unwrap_or_default()
    }
    pub fn try_new(key: Key) -> Result<Self, MetricDefinitionError> {
        if !(validate(&key, MetricType::Gauge, MetricClass::Dynamic)
            && definition(key.name()).is_some_and(|def| def.contribution))
        {
            return Err(MetricDefinitionError);
        }
        let owner_store = super::metrics_api::dynamic_store();
        if let Some(store) = &owner_store {
            store.owner_created();
        }
        Ok(Self {
            key: Some(key),
            state: parking_lot::Mutex::new(ContributionState {
                owner_store,
                ..Default::default()
            }),
        })
    }
    pub fn set(&self, value: f64) {
        self.change(|_| value, false);
    }
    pub fn increase(&self, value: f64) {
        self.change(|current| current + value, false);
    }
    pub fn decrease(&self, value: f64) {
        self.change(|current| current - value, false);
    }
    pub fn clear(&self) {
        self.change(|_| 0., true);
    }
    fn change(&self, update: impl FnOnce(f64) -> f64, conditional: bool) {
        let mut state = self.state.lock();
        let old = state.committed;
        let next = update(old);
        if !next.is_finite() || next < 0. {
            return;
        }
        state.committed = next;
        let Some(key) = &self.key else {
            return;
        };
        if let Some((store, slot)) = &state.attached {
            store.contribution(slot, next - old, false, false, conditional);
            return;
        }
        if conditional {
            if state.owner_store.is_none() && super::metrics_api::dynamic_store().is_none() {
                metrics::with_recorder(|recorder| recorder.register_gauge(key, &METADATA)).increment(next - old);
            }
            return;
        }
        let store = state.owner_store.clone().or_else(super::metrics_api::dynamic_store);
        if let Some(store) = store {
            if state.owner_store.is_none() {
                store.owner_created();
                state.owner_store = Some(store.clone());
            }
            if let Some(slot) = store.resolve(key) {
                // A prior rejection/pre-install write is part of this owner's
                // current contribution, not merely the most recent delta.
                store.contribution(&slot, next, true, false, false);
                state.attached = Some((store, slot));
            }
        } else {
            // Isolated conventional recorders retain their own gauge behavior;
            // never cache a pre-install no-op handle.
            metrics::with_recorder(|recorder| recorder.register_gauge(key, &METADATA)).increment(next - old);
        }
    }
}
impl Default for GaugeContribution {
    fn default() -> Self {
        Self {
            key: None,
            state: parking_lot::Mutex::new(ContributionState::default()),
        }
    }
}
impl std::fmt::Debug for GaugeContribution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GaugeContribution").finish_non_exhaustive()
    }
}
impl Drop for GaugeContribution {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        let bound_to_common = state.owner_store.is_some();
        if let Some(store) = state.owner_store.take() {
            store.owner_released();
        }
        if let Some((store, slot)) = state.attached.take() {
            store.contribution(&slot, -state.committed, false, true, true);
        } else if let Some(key) = &self.key {
            if !bound_to_common && super::metrics_api::dynamic_store().is_none() && state.committed != 0. {
                metrics::with_recorder(|recorder| recorder.register_gauge(key, &METADATA)).decrement(state.committed);
            }
        }
    }
}

pub use crate::{dynamic_counter, dynamic_gauge, dynamic_histogram, fixed_counter, fixed_gauge};

#[macro_export]
macro_rules! fixed_counter {
    ($name:expr $(,)?)=>{$crate::observe::metrics_facade::FixedCounter::new(::metrics::Key::from_name($name))};
    ($name:expr,$($key:expr=>$value:expr),+ $(,)?)=>{$crate::observe::metrics_facade::FixedCounter::new(::metrics::Key::from_parts($name,vec![$(::metrics::Label::new($key,$value)),+]))};
    ($name:expr,$labels:expr $(,)?)=>{$crate::observe::metrics_facade::FixedCounter::new(::metrics::Key::from_parts($name,::metrics::IntoLabels::into_labels($labels)))};
}
#[macro_export]
macro_rules! fixed_gauge {
    ($name:expr $(,)?)=>{$crate::observe::metrics_facade::FixedGauge::new(::metrics::Key::from_name($name))};
    ($name:expr,$($key:expr=>$value:expr),+ $(,)?)=>{$crate::observe::metrics_facade::FixedGauge::new(::metrics::Key::from_parts($name,vec![$(::metrics::Label::new($key,$value)),+]))};
    ($name:expr,$labels:expr $(,)?)=>{$crate::observe::metrics_facade::FixedGauge::new(::metrics::Key::from_parts($name,::metrics::IntoLabels::into_labels($labels)))};
}
#[macro_export]
macro_rules! dynamic_counter {
    ($name:expr $(,)?)=>{$crate::observe::metrics_facade::DynamicCounter::new(::metrics::Key::from_name($name))};
    ($name:expr,$($key:expr=>$value:expr),+ $(,)?)=>{$crate::observe::metrics_facade::DynamicCounter::new(::metrics::Key::from_parts($name,vec![$(::metrics::Label::new($key,$value)),+]))};
    ($name:expr,$labels:expr $(,)?)=>{$crate::observe::metrics_facade::DynamicCounter::new(::metrics::Key::from_parts($name,::metrics::IntoLabels::into_labels($labels)))};
}
#[macro_export]
macro_rules! dynamic_gauge {
    ($name:expr $(,)?)=>{$crate::observe::metrics_facade::DynamicGauge::new(::metrics::Key::from_name($name))};
    ($name:expr,$($key:expr=>$value:expr),+ $(,)?)=>{$crate::observe::metrics_facade::DynamicGauge::new(::metrics::Key::from_parts($name,vec![$(::metrics::Label::new($key,$value)),+]))};
    ($name:expr,$labels:expr $(,)?)=>{$crate::observe::metrics_facade::DynamicGauge::new(::metrics::Key::from_parts($name,::metrics::IntoLabels::into_labels($labels)))};
}
#[macro_export]
macro_rules! dynamic_histogram {
    ($name:expr $(,)?)=>{$crate::observe::metrics_facade::DynamicHistogram::new(::metrics::Key::from_name($name))};
    ($name:expr,$($key:expr=>$value:expr),+ $(,)?)=>{$crate::observe::metrics_facade::DynamicHistogram::new(::metrics::Key::from_parts($name,vec![$(::metrics::Label::new($key,$value)),+]))};
    ($name:expr,$labels:expr $(,)?)=>{$crate::observe::metrics_facade::DynamicHistogram::new(::metrics::Key::from_parts($name,::metrics::IntoLabels::into_labels($labels)))};
}

#[cfg(test)]
#[path = "../../tests/unit/observe/metrics_facade_test.rs"]
mod tests;
