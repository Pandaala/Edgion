# LB Panic Threshold Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an Envoy/Istio-style load-balancing panic threshold so a service whose backends are *all* marked unhealthy (active health check + circuit breaker) routes to all backends instead of returning a hard 503.

**Architecture:** Per-request, before selecting, count how many backends pass the combined health predicate over the selector's own backend list; if the survivor fraction is below a configurable threshold, select with a no-op health filter (panic mode). Threshold resolves as `edgion.io/panic-threshold` annotation → `EdgionGatewayConfig.loadBalancing.panicThreshold` global default → built-in `0` (disabled). Panic is an independent final safety valve layered over `is_backend_healthy`; the circuit breaker's own `maxEjectionPercent` is unchanged.

**Tech Stack:** Rust, Pingora, `dashmap`, `arc-swap`, `metrics` crate, `serde`/`serde_yaml`, `k8s-openapi`, `kube` (CustomResource).

**Spec:** `docs/superpowers/specs/2026-06-07-lb-panic-threshold-design.md`

---

## File Structure

- **Create** `src/core/gateway/backends/panic_threshold.rs` — per-service threshold store (`DashMap<String, u8>`), process-global default `AtomicU8`, annotation parser, and the pure `resolve_panic_threshold` / `panic_floor` helpers. One responsibility: "what is the effective panic threshold and the survivor floor".
- **Modify** `src/core/gateway/backends/mod.rs` — declare the new module and re-export its public API.
- **Modify** `src/types/constants/annotations.rs` — add the `PANIC_THRESHOLD` annotation constant.
- **Modify** `src/core/gateway/backends/discovery/services/conf_handler_impl.rs` — parse the annotation and feed the store on full/partial/remove.
- **Modify** `src/types/resources/edgion_gateway_config.rs` — add `loadBalancing.panicThreshold` to the CRD spec.
- **Modify** `src/core/gateway/config/edgion_gateway/conf_handler_impl.rs` — push the global default into the atomic on apply.
- **Modify** `src/core/gateway/observe/metrics/registry.rs` — add the `lb_panic_mode` gauge metric.
- **Modify** `src/core/gateway/lb/compiled_selector.rs` — add `CompiledSelector::panic_should_engage` (counts over the selector's own backend list).
- **Modify** `src/core/gateway/backends/selector.rs` — split `is_backend_healthy` into a quiet predicate + metric wrapper; add `panic_engaged`; wire panic into `pick` and `select_roundrobin_backend_key`.
- **Modify** docs: `docs/en/...` annotations + EdgionGatewayConfig references and `skills/02-features/05-annotations/`.
- **Create** an integration scenario under `examples/test/.../integration/` + `skills/05-testing`.

---

## Task 1: Per-service threshold store + global default + pure helpers

**Files:**
- Create: `src/core/gateway/backends/panic_threshold.rs`
- Modify: `src/core/gateway/backends/mod.rs`

- [ ] **Step 1: Write the failing test**

Append to the new file `src/core/gateway/backends/panic_threshold.rs`:

```rust
//! Load-balancing panic threshold: per-service override store, process-global
//! default, and the pure resolution / floor math.
//!
//! Panic mode engages when the fraction of backends passing the combined
//! health predicate drops below the effective threshold percent. Threshold
//! resolves as: `edgion.io/panic-threshold` annotation (per service) → the
//! `EdgionGatewayConfig.loadBalancing.panicThreshold` global default → 0.
//! A threshold of 0 disables panic mode (opt-in; matches Istio's k8s default).

use crate::types::constants::annotations::edgion::PANIC_THRESHOLD;
use dashmap::DashMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LazyLock};

static PANIC_THRESHOLD_STORE: LazyLock<Arc<PanicThresholdStore>> =
    LazyLock::new(|| Arc::new(PanicThresholdStore::new()));

/// Process-global default panic threshold percent, set from EdgionGatewayConfig.
/// Initialized to 0 (panic disabled) so the feature is strictly opt-in.
static GLOBAL_DEFAULT: AtomicU8 = AtomicU8::new(0);

pub fn get_panic_threshold_store() -> Arc<PanicThresholdStore> {
    PANIC_THRESHOLD_STORE.clone()
}

/// Per-service panic-threshold overrides parsed from the Service annotation.
pub struct PanicThresholdStore {
    services: DashMap<String, u8>,
}

impl PanicThresholdStore {
    pub fn new() -> Self {
        Self {
            services: DashMap::new(),
        }
    }

    /// Set or clear the override for a service. `None` removes any override so
    /// the service falls back to the global default.
    pub fn set_service_threshold(&self, service_key: &str, threshold: Option<u8>) {
        match threshold {
            Some(v) => {
                self.services.insert(service_key.to_string(), v);
            }
            None => {
                self.services.remove(service_key);
            }
        }
    }

    pub fn remove_service(&self, service_key: &str) {
        self.services.remove(service_key);
    }

    fn get_override(&self, service_key: &str) -> Option<u8> {
        self.services.get(service_key).map(|v| *v)
    }
}

impl Default for PanicThresholdStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Set the process-global default panic threshold (called when EdgionGatewayConfig
/// is applied). Values > 100 are invalid and treated as 0 (disabled).
pub fn configure_lb_panic_default(threshold: u8) {
    let effective = if threshold > 100 {
        tracing::warn!(threshold, "Invalid global panicThreshold (>100), disabling panic mode");
        0
    } else {
        threshold
    };
    GLOBAL_DEFAULT.store(effective, Ordering::Relaxed);
    tracing::info!(threshold = effective, "LB global panic threshold updated");
}

pub fn get_lb_panic_default() -> u8 {
    GLOBAL_DEFAULT.load(Ordering::Relaxed)
}

/// Resolve the effective threshold for a service: per-service override, else the
/// process-global default.
pub fn resolve_panic_threshold(service_key: &str) -> u8 {
    get_panic_threshold_store()
        .get_override(service_key)
        .unwrap_or_else(get_lb_panic_default)
}

/// Survivor floor: the minimum number of healthy backends required to *avoid*
/// panic. `ceil(threshold * total / 100)`. Panic engages when survivors < floor.
/// `threshold` is assumed already validated to 0..=100; `total` >= 1.
pub fn panic_floor(threshold: u8, total: usize) -> usize {
    (threshold as usize * total).div_ceil(100)
}

/// Parse the `edgion.io/panic-threshold` annotation: a bare integer percent
/// string in 0..=100. Anything else (non-integer, > 100) is logged and ignored
/// so the service falls back to the global default.
pub fn parse_panic_threshold_annotation(meta: &ObjectMeta) -> Option<u8> {
    let raw = meta.annotations.as_ref()?.get(PANIC_THRESHOLD)?;
    match raw.trim().parse::<u8>() {
        Ok(v) if v <= 100 => Some(v),
        _ => {
            tracing::warn!(
                resource = %meta.name.as_deref().unwrap_or(""),
                value = %raw,
                "Invalid edgion.io/panic-threshold annotation (want integer 0-100), ignoring"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn meta_with(value: Option<&str>) -> ObjectMeta {
        let mut ann = BTreeMap::new();
        if let Some(v) = value {
            ann.insert(PANIC_THRESHOLD.to_string(), v.to_string());
        }
        ObjectMeta {
            name: Some("svc".to_string()),
            annotations: if ann.is_empty() { None } else { Some(ann) },
            ..Default::default()
        }
    }

    #[test]
    fn floor_is_ceil() {
        assert_eq!(panic_floor(50, 10), 5); // 50% of 10
        assert_eq!(panic_floor(50, 3), 2); // ceil(1.5)
        assert_eq!(panic_floor(100, 10), 10); // require all healthy
        assert_eq!(panic_floor(1, 1), 1);
    }

    #[test]
    fn annotation_parse_valid_and_invalid() {
        assert_eq!(parse_panic_threshold_annotation(&meta_with(Some("50"))), Some(50));
        assert_eq!(parse_panic_threshold_annotation(&meta_with(Some("0"))), Some(0));
        assert_eq!(parse_panic_threshold_annotation(&meta_with(Some("100"))), Some(100));
        assert_eq!(parse_panic_threshold_annotation(&meta_with(Some("101"))), None);
        assert_eq!(parse_panic_threshold_annotation(&meta_with(Some("50%"))), None);
        assert_eq!(parse_panic_threshold_annotation(&meta_with(Some("abc"))), None);
        assert_eq!(parse_panic_threshold_annotation(&meta_with(None)), None);
    }

    #[test]
    fn resolve_prefers_override_then_global() {
        let store = get_panic_threshold_store();
        configure_lb_panic_default(30);
        store.set_service_threshold("default/a", Some(70));
        assert_eq!(resolve_panic_threshold("default/a"), 70);
        assert_eq!(resolve_panic_threshold("default/no-override"), 30);
        store.set_service_threshold("default/a", None);
        assert_eq!(resolve_panic_threshold("default/a"), 30);
        configure_lb_panic_default(0); // reset global for other tests
    }

    #[test]
    fn global_default_clamps_invalid() {
        configure_lb_panic_default(200);
        assert_eq!(get_lb_panic_default(), 0);
        configure_lb_panic_default(0);
    }
}
```

Then register the module — add to `src/core/gateway/backends/mod.rs` after the existing `pub mod ...` lines (keep alphabetical-ish ordering near `pub mod policy;`):

```rust
pub mod panic_threshold;
```

and add to the re-export block near the other `pub use` lines:

```rust
pub use panic_threshold::{
    configure_lb_panic_default, get_lb_panic_default, get_panic_threshold_store, panic_floor,
    parse_panic_threshold_annotation, resolve_panic_threshold,
};
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p edgion panic_threshold:: 2>&1 | tail -20`
Expected: FAIL to **compile** — `PANIC_THRESHOLD` is not yet defined in `annotations.rs` (added in Step 3). This is the expected red.

- [ ] **Step 3: Add the annotation constant**

Modify `src/types/constants/annotations.rs` — inside `pub mod edgion`, after the `CIRCUIT_BREAKER` constant:

```rust
    /// Per-service load-balancing panic threshold (percent, 0-100).
    /// When the fraction of healthy backends drops below this percent, the
    /// gateway ignores health filtering and load-balances across all backends.
    /// Value: integer string. 0 (default) disables panic mode.
    pub const PANIC_THRESHOLD: &str = "edgion.io/panic-threshold";
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p edgion panic_threshold:: 2>&1 | tail -20`
Expected: PASS (4 tests). If `div_ceil` is unavailable, the MSRV is too old — replace with `(threshold as usize * total + 99) / 100`.

Note: `resolve_prefers_override_then_global` and `global_default_clamps_invalid` (and Task 3's
test) mutate the shared process-global `AtomicU8`. They reset it to `0` at the end, but if these
specific tests prove flaky under parallel execution, run them with `-- --test-threads=1` or gate
the global-default assertions behind a shared `std::sync::Mutex` test lock. Per-service-override
tests are unaffected (they don't depend on the global).

- [ ] **Step 5: Commit**

```bash
git add src/core/gateway/backends/panic_threshold.rs src/core/gateway/backends/mod.rs src/types/constants/annotations.rs
git commit -m "feat(lb): panic threshold store, global default, annotation parse"
```

---

## Task 2: Wire the annotation into the service config handler

**Files:**
- Modify: `src/core/gateway/backends/discovery/services/conf_handler_impl.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `conf_handler_impl.rs` (it already imports `super::*` and has `create_test_service`):

```rust
    #[test]
    fn test_panic_threshold_annotation_plumbed_and_removed() {
        use crate::core::gateway::backends::panic_threshold::{
            get_panic_threshold_store, resolve_panic_threshold, configure_lb_panic_default,
        };
        configure_lb_panic_default(0);
        let store = ServiceStore::new();

        let mut svc = create_test_service("default", "pt-svc");
        svc.metadata.annotations = Some(std::collections::BTreeMap::from([(
            "edgion.io/panic-threshold".to_string(),
            "40".to_string(),
        )]));

        let mut data = HashMap::new();
        data.insert("default/pt-svc".to_string(), Arc::new(svc));
        store.full_set(&data);
        assert_eq!(resolve_panic_threshold("default/pt-svc"), 40);

        // Removal clears the override (falls back to global default 0).
        store.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["default/pt-svc".to_string()]));
        assert_eq!(resolve_panic_threshold("default/pt-svc"), 0);
        get_panic_threshold_store().remove_service("default/pt-svc");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p edgion test_panic_threshold_annotation_plumbed_and_removed 2>&1 | tail -20`
Expected: FAIL — `resolve_panic_threshold` returns `0` (annotation not parsed yet).

- [ ] **Step 3: Implement the plumbing**

In `conf_handler_impl.rs`, extend the imports at the top:

```rust
use crate::core::gateway::backends::panic_threshold::{get_panic_threshold_store, parse_panic_threshold_annotation};
```

In `full_set`, add the store handle near the others:

```rust
        let panic_store = get_panic_threshold_store();
```

In `full_set`'s stale-key cleanup loop, after `cb_store.remove_service(&old_key);`:

```rust
                panic_store.remove_service(&old_key);
```

In `full_set`'s upsert loop, after `cb_store.set_service_config(key, cb_config);`:

```rust
            panic_store.set_service_threshold(key, parse_panic_threshold_annotation(&service.metadata));
```

In `partial_update`, add `let panic_store = get_panic_threshold_store();` near the other store handles. In the add/update loop, after `cb_store.set_service_config(key, cb_config);`:

```rust
            panic_store.set_service_threshold(key, parse_panic_threshold_annotation(&service.metadata));
```

In `partial_update`'s remove loop, after `cb_store.remove_service(key);`:

```rust
            panic_store.remove_service(key);
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p edgion test_panic_threshold_annotation_plumbed_and_removed 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/core/gateway/backends/discovery/services/conf_handler_impl.rs
git commit -m "feat(lb): plumb edgion.io/panic-threshold annotation into service handler"
```

---

## Task 3: Global default via EdgionGatewayConfig.loadBalancing.panicThreshold

**Files:**
- Modify: `src/types/resources/edgion_gateway_config.rs`
- Modify: `src/core/gateway/config/edgion_gateway/conf_handler_impl.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module of `src/core/gateway/config/edgion_gateway/conf_handler_impl.rs`:

```rust
    #[test]
    fn test_load_balancing_panic_threshold_applied() {
        use crate::core::gateway::backends::panic_threshold::get_lb_panic_default;
        use crate::types::resources::edgion_gateway_config::LoadBalancingConfig;

        let handler = EdgionGatewayConfigHandler::new();
        let mut cfg = make_test_config("gw-pt"); // existing test helper; see note
        cfg.spec.load_balancing = Some(LoadBalancingConfig { panic_threshold: 50 });

        let mut data = HashMap::new();
        data.insert("gw-pt".to_string(), Arc::new(cfg));
        handler.full_set(&data);

        assert_eq!(get_lb_panic_default(), 50);
        crate::core::gateway::backends::panic_threshold::configure_lb_panic_default(0);
    }
```

Note: if no `make_test_config` helper exists, build the config inline using the same pattern the surrounding tests use (look at the existing `max_retries: 99` test around line 204). The only fields that matter here are `spec.load_balancing`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p edgion test_load_balancing_panic_threshold_applied 2>&1 | tail -20`
Expected: FAIL to compile — `LoadBalancingConfig` / `load_balancing` field do not exist yet.

- [ ] **Step 3: Add the CRD field**

In `src/types/resources/edgion_gateway_config.rs`, add a field to `EdgionGatewayConfigSpec` (after `tcp_timeout`):

```rust
    /// Load-balancing-wide settings (panic threshold, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_balancing: Option<LoadBalancingConfig>,
```

And add the struct (near `HttpTimeout`, keep the camelCase + JsonSchema pattern):

```rust
/// Gateway-wide load-balancing configuration.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoadBalancingConfig {
    /// Panic threshold percent (0-100). When the fraction of healthy backends
    /// for a service drops below this percent, the gateway ignores health
    /// filtering and load-balances across all backends. 0 (default) disables
    /// panic mode. Per-service `edgion.io/panic-threshold` overrides this.
    #[serde(default)]
    pub panic_threshold: u8,
}
```

- [ ] **Step 4: Wire the apply path**

In `src/core/gateway/config/edgion_gateway/conf_handler_impl.rs`, add an apply helper next to `apply_link_sys_config`:

```rust
fn apply_load_balancing_config(config: &EdgionGatewayConfig) {
    if let Some(lb) = &config.spec.load_balancing {
        crate::core::gateway::backends::panic_threshold::configure_lb_panic_default(lb.panic_threshold);
    }
}
```

Then call it everywhere `apply_link_sys_config` is called (the three sites at lines ~63, ~83, ~96). For example, immediately after each `apply_link_sys_config(config);` / `apply_link_sys_config(&config);`:

```rust
            apply_load_balancing_config(config);
```

(match the borrow form — `config` vs `&config` — used at each existing call site).

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p edgion test_load_balancing_panic_threshold_applied 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/types/resources/edgion_gateway_config.rs src/core/gateway/config/edgion_gateway/conf_handler_impl.rs
git commit -m "feat(lb): EdgionGatewayConfig.loadBalancing.panicThreshold global default"
```

---

## Task 4: Split `is_backend_healthy` into quiet predicate + metric wrapper

**Files:**
- Modify: `src/core/gateway/backends/selector.rs`

- [ ] **Step 1: Write the failing test**

Add a test module entry to `selector.rs` (create a `#[cfg(test)] mod tests` block at the end if none exists). Because `is_backend_healthy_quiet` and `is_backend_healthy` both depend on global health/CB stores, the unit test asserts **boolean parity** for a service with no HC and no CB config (both must return `true`):

```rust
#[cfg(test)]
mod panic_tests {
    use super::*;
    use crate::core::gateway::backends::health::get_health_status_store;
    use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
    use pingora_load_balancing::Backend;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn backend(i: u8) -> Backend {
        Backend {
            addr: PSocketAddr::Inet(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, i)), 80)),
            weight: 1,
            ext: Default::default(),
        }
    }

    #[test]
    fn quiet_and_metric_predicates_agree_with_no_config() {
        let hs = get_health_status_store();
        let b = backend(1);
        // No HC/CB config for this service key → both predicates return true.
        assert!(is_backend_healthy_quiet("default/unconfigured-svc", &b, &hs));
        assert!(is_backend_healthy("default/unconfigured-svc", &b, &hs));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p edgion panic_tests::quiet_and_metric_predicates_agree_with_no_config 2>&1 | tail -20`
Expected: FAIL to compile — `is_backend_healthy_quiet` does not exist yet.

- [ ] **Step 3: Implement the split**

Replace the existing `is_backend_healthy` (selector.rs:36-55) with a quiet predicate plus a metric-recording wrapper:

```rust
/// Side-effect-free health predicate: active health check + circuit-breaker
/// ejection, with NO metric recording. Used by the panic-threshold counting
/// pass so it does not double-count circuit-breaker rejections.
#[inline]
pub(crate) fn is_backend_healthy_quiet(
    service_key: &str,
    backend: &pingora_load_balancing::Backend,
    health_store: &health::check::HealthStatusStore,
) -> bool {
    if health_store.has_service(service_key)
        && !health_store.is_healthy(service_key, health::check::backend_hash(backend))
    {
        return false;
    }
    !get_circuit_breaker_store().is_ejected(service_key, health::check::backend_hash(backend))
}

/// Health predicate for the selection walk. Same logic as
/// [`is_backend_healthy_quiet`] but records a circuit-breaker rejection metric
/// when a backend is rejected for being ejected (preserves existing behavior).
#[inline]
pub(crate) fn is_backend_healthy(
    service_key: &str,
    backend: &pingora_load_balancing::Backend,
    health_store: &health::check::HealthStatusStore,
) -> bool {
    // 1. Active health check filter
    if health_store.has_service(service_key)
        && !health_store.is_healthy(service_key, health::check::backend_hash(backend))
    {
        return false;
    }
    // 2. Circuit breaker filter (records the rejection metric)
    if get_circuit_breaker_store().is_ejected(service_key, health::check::backend_hash(backend)) {
        crate::core::gateway::observe::metrics::record_cb_rejection(service_key);
        return false;
    }
    true
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p edgion panic_tests::quiet_and_metric_predicates_agree_with_no_config 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/core/gateway/backends/selector.rs
git commit -m "refactor(lb): split is_backend_healthy into quiet predicate + metric wrapper"
```

---

## Task 5: `CompiledSelector::panic_should_engage`

**Files:**
- Modify: `src/core/gateway/lb/compiled_selector.rs`

- [ ] **Step 1: Write the failing test**

Add to (or create) the `#[cfg(test)] mod tests` block in `compiled_selector.rs`. Build a selector over a few backends and assert the survivor-floor decision for each policy:

```rust
#[cfg(test)]
mod panic_should_engage_tests {
    use super::*;
    use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
    use pingora_load_balancing::Backend;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn backends(n: u8) -> Vec<Arc<EdgionBackend>> {
        (0..n)
            .map(|i| {
                Arc::new(EdgionBackend::new(Backend {
                    addr: PSocketAddr::Inet(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, i)), 80)),
                    weight: 1,
                    ext: Default::default(),
                }))
            })
            .collect()
    }

    fn rr_selector(n: u8) -> CompiledSelector {
        CompiledSelector::RoundRobin(RrCore::build(Arc::new(backends(n))))
    }

    #[test]
    fn engages_when_survivors_below_floor() {
        let sel = rr_selector(10);
        // floor for 50% of 10 = 5; only 4 healthy → panic.
        let mut healthy = 0;
        let engaged = sel.panic_should_engage(5, |_| {
            healthy += 1;
            healthy <= 4
        });
        assert!(engaged);
    }

    #[test]
    fn does_not_engage_when_survivors_reach_floor() {
        let sel = rr_selector(10);
        // 6 healthy ≥ floor 5 → no panic; early-exit after the 5th survivor.
        let engaged = sel.panic_should_engage(5, |_| true);
        assert!(!engaged);
    }

    #[test]
    fn floor_zero_never_engages() {
        let sel = rr_selector(3);
        assert!(!sel.panic_should_engage(0, |_| false));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p edgion panic_should_engage_tests:: 2>&1 | tail -20`
Expected: FAIL to compile — `panic_should_engage` / `backends_slice` not defined.

- [ ] **Step 3: Implement the method**

In `compiled_selector.rs`, inside `impl CompiledSelector` (next to `backend_count`), add:

```rust
    /// Borrow this selector's own backend list. Used by the panic-threshold
    /// counting pass so the count and the selection walk operate on the exact
    /// same list (no `replace_backends` TOCTOU skew).
    fn backends_slice(&self) -> &[Arc<EdgionBackend>] {
        match self {
            CompiledSelector::RoundRobin(c) => &c.backends,
            CompiledSelector::ConsistentHash(c) => &c.backends,
            CompiledSelector::LeastConn(c) => &c.backends,
            CompiledSelector::Ewma(c) => &c.backends,
        }
    }

    /// Decide whether panic mode should engage: count how many backends pass
    /// the (side-effect-free) `quiet` predicate, returning `true` when fewer
    /// than `floor` survive. Early-exits as soon as `floor` survivors are seen,
    /// so the common healthy case scans at most `floor` backends. `floor == 0`
    /// disables panic (returns `false`).
    pub fn panic_should_engage(&self, floor: usize, quiet: impl Fn(&EdgionBackend) -> bool) -> bool {
        if floor == 0 {
            return false;
        }
        let mut survivors = 0usize;
        for b in self.backends_slice() {
            if quiet(b) {
                survivors += 1;
                if survivors >= floor {
                    return false;
                }
            }
        }
        true
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p edgion panic_should_engage_tests:: 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/core/gateway/lb/compiled_selector.rs
git commit -m "feat(lb): CompiledSelector::panic_should_engage over own backend list"
```

---

## Task 6: `lb_panic_mode` gauge metric

**Files:**
- Modify: `src/core/gateway/observe/metrics/registry.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module (or a new `#[cfg(test)] mod` at the bottom) of `registry.rs`:

```rust
#[cfg(test)]
mod lb_panic_metric_tests {
    use super::*;

    #[test]
    fn record_lb_panic_mode_does_not_panic() {
        // Smoke test: the gauge set path is callable for engaged/cleared.
        record_lb_panic_mode("default/svc", true);
        record_lb_panic_mode("default/svc", false);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p edgion lb_panic_metric_tests:: 2>&1 | tail -20`
Expected: FAIL to compile — `record_lb_panic_mode` undefined.

- [ ] **Step 3: Implement the metric**

Add the name constant in the `names` module (next to the `CB_*` constants near line 44):

```rust
    // Load-balancing panic mode: 1 = engaged, 0 = normal (per service)
    pub const LB_PANIC_MODE: &str = "edgion_lb_panic_mode";
```

Add the recorder near `record_cb_rejection`:

```rust
/// Set the panic-mode gauge for a service: 1.0 when panic mode is engaged
/// (health filtering bypassed), 0.0 when selecting normally. Called from the
/// selection path; cheap (atomic gauge set), no logging — Iron Rule 3 safe.
pub fn record_lb_panic_mode(service_key: &str, engaged: bool) {
    gauge!(names::LB_PANIC_MODE, "service" => service_key.to_string())
        .set(if engaged { 1.0 } else { 0.0 });
}
```

Ensure `gauge` is imported in this file (the `record_cb_ejection` function already uses `gauge!`, so it is).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p edgion lb_panic_metric_tests:: 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/core/gateway/observe/metrics/registry.rs
git commit -m "feat(metrics): add edgion_lb_panic_mode gauge"
```

---

## Task 7: Integrate panic decision into the selection path

**Files:**
- Modify: `src/core/gateway/backends/selector.rs`

- [ ] **Step 1: Write the failing test**

Add these tests to the `panic_tests` module created in Task 4. They drive the full path via the public `select_roundrobin_backend_key`, using the health store to force all-unhealthy:

```rust
    use crate::core::gateway::backends::panic_threshold::{
        configure_lb_panic_default, get_panic_threshold_store,
    };
    use crate::core::gateway::lb::compiled_selector;

    fn seed_cell(service_key: &str, n: u8) {
        let cell = compiled_selector::get_or_create_cell(service_key);
        let bs: Vec<crate::core::gateway::lb::EdgionBackend> = (0..n)
            .map(|i| {
                crate::core::gateway::lb::EdgionBackend::new(backend(i + 1))
            })
            .collect();
        cell.replace_backends(bs);
    }

    fn mark_all_unhealthy(service_key: &str, n: u8) {
        let hs = get_health_status_store();
        let hashes: Vec<u64> = (0..n)
            .map(|i| health::check::backend_hash(&backend(i + 1)))
            .collect();
        hs.register_service(service_key, hashes.clone());
        // Drive each backend below the unhealthy threshold.
        for h in hashes {
            for _ in 0..5 {
                hs.record_check(service_key, h, false, 1, 1);
            }
        }
    }

    #[test]
    fn panic_off_all_unhealthy_returns_none() {
        let svc = "default/panic-off";
        configure_lb_panic_default(0);
        get_panic_threshold_store().set_service_threshold(svc, None);
        seed_cell(svc, 4);
        mark_all_unhealthy(svc, 4);
        assert!(select_roundrobin_backend_key(svc).is_none());
    }

    #[test]
    fn panic_on_all_unhealthy_selects_backend() {
        let svc = "default/panic-on";
        get_panic_threshold_store().set_service_threshold(svc, Some(50));
        seed_cell(svc, 4);
        mark_all_unhealthy(svc, 4);
        assert!(select_roundrobin_backend_key(svc).is_some());
        get_panic_threshold_store().set_service_threshold(svc, None);
    }
```

Note: confirm the `HealthStatusStore::record_check(service_key, hash, healthy, healthy_threshold, unhealthy_threshold)` signature against `status_store.rs` and adjust the call if it differs; the intent is "record enough failures to mark the backend unhealthy".

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p edgion panic_tests::panic_on_all_unhealthy_selects_backend 2>&1 | tail -20`
Expected: FAIL — currently both panic-on and panic-off return `None` (no panic logic), so the `panic_on...` assertion `is_some()` fails.

- [ ] **Step 3: Implement panic integration**

In `selector.rs`, add the decision helper (after `is_backend_healthy`):

```rust
/// Decide whether panic mode should engage for this selection. Gated so the
/// O(n) survivor count only runs for services that actually have health-check
/// or circuit-breaker state (otherwise every backend passes and panic can never
/// trigger). Records the `lb_panic_mode` gauge when it evaluates.
fn panic_engaged(
    service_key: &str,
    selector: &compiled_selector::CompiledSelector,
    health_store: &health::check::HealthStatusStore,
) -> bool {
    let cb_store = get_circuit_breaker_store();
    if !health_store.has_service(service_key) && !cb_store.has_service(service_key) {
        return false;
    }
    let threshold = crate::core::gateway::backends::panic_threshold::resolve_panic_threshold(service_key);
    if threshold == 0 {
        return false;
    }
    let total = selector.backend_count();
    if total == 0 {
        return false;
    }
    let floor = crate::core::gateway::backends::panic_threshold::panic_floor(threshold, total);
    let engaged = selector.panic_should_engage(floor, |b| {
        is_backend_healthy_quiet(service_key, b.inner(), health_store)
    });
    crate::core::gateway::observe::metrics::record_lb_panic_mode(service_key, engaged);
    engaged
}
```

Update `select_roundrobin_backend_key` (selector.rs:73-81) to use it:

```rust
pub fn select_roundrobin_backend_key(service_key: &str) -> Option<Arc<EdgionBackend>> {
    let cell = compiled_selector::get_or_create_cell(service_key);
    if !hydrate_cell_if_empty(&cell, service_key) {
        return None;
    }
    let selector = cell.get_or_build(PolicyKind::RoundRobin);
    let health_store = get_health_status_store();
    if panic_engaged(service_key, &selector, &health_store) {
        return selector.select(None, |_| true);
    }
    selector.select(None, |b| is_backend_healthy(service_key, b.inner(), &health_store))
}
```

Update `pick` (selector.rs:200-205) similarly — replace the final `selector.select(...).ok_or(...)`:

```rust
    let health_store = get_health_status_store();
    let result = if panic_engaged(service_key, &selector, &health_store) {
        selector.select(hash_key.as_deref(), |_| true)
    } else {
        selector.select(hash_key.as_deref(), |b| {
            is_backend_healthy(service_key, b.inner(), &health_store)
        })
    };
    result.ok_or(not_found.for_kind(kind, hash_key_used))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p edgion panic_tests:: 2>&1 | tail -20`
Expected: PASS (panic-on selects a backend; panic-off returns None).

- [ ] **Step 5: Run the broader LB test suite for regressions**

Run: `cargo test -p edgion lb:: backends::selector:: 2>&1 | tail -30`
Expected: PASS — existing selection behavior unchanged when no panic threshold is configured.

- [ ] **Step 6: Commit**

```bash
git add src/core/gateway/backends/selector.rs
git commit -m "feat(lb): engage panic mode in pick + select_roundrobin_backend_key"
```

---

## Task 8 (best-effort, optional): off-path panic-mode edge log

**Scope note:** The `lb_panic_mode` gauge is the source of truth. This task adds a
human-readable `tracing::warn` on engage/disengage transitions, emitted off the
request path from the health-check probe loop. It only covers services with an
active health-check loop (CB-only panic services get the gauge but no edge log —
documented limitation; do not add `tracing::` on the request path to close it).
Skip this task if you want the MVP without the log.

**Files:**
- Modify: `src/core/gateway/backends/panic_threshold.rs` (add a readable `AtomicBool` per service)
- Modify: `src/core/gateway/backends/selector.rs` (`panic_engaged` updates the flag)
- Modify: `src/core/gateway/backends/health/check/manager.rs` (loop reads the flag, logs edges)

- [ ] **Step 1: Write the failing test**

Add to `panic_threshold.rs` tests:

```rust
    #[test]
    fn panic_active_flag_roundtrips() {
        let store = get_panic_threshold_store();
        store.set_panic_active("default/flag-svc", true);
        assert!(store.is_panic_active("default/flag-svc"));
        store.set_panic_active("default/flag-svc", false);
        assert!(!store.is_panic_active("default/flag-svc"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p edgion panic_active_flag_roundtrips 2>&1 | tail -20`
Expected: FAIL to compile — `set_panic_active` / `is_panic_active` undefined.

- [ ] **Step 3: Implement the readable flag**

In `PanicThresholdStore`, add a second map and methods:

```rust
    // add to the struct:
    //   active: DashMap<String, bool>,
    // and initialize `active: DashMap::new()` in `new()`.

    pub fn set_panic_active(&self, service_key: &str, active: bool) {
        self.active.insert(service_key.to_string(), active);
    }

    pub fn is_panic_active(&self, service_key: &str) -> bool {
        self.active.get(service_key).map(|v| *v).unwrap_or(false)
    }
```

Also clear it in `remove_service`:

```rust
        self.active.remove(service_key);
```

In `selector.rs` `panic_engaged`, after computing `engaged` and recording the gauge:

```rust
    get_panic_threshold_store().set_panic_active(service_key, engaged);
```

(add `get_panic_threshold_store` to the existing `panic_threshold::{...}` use path).

- [ ] **Step 4: Implement the edge log in the HC loop**

In `health_check_loop` (`manager.rs`), before the `loop {`, add:

```rust
    let mut last_panic = false;
```

Inside the loop, after `health_store.register_service(&service_key, hashes);`:

```rust
        let now_panic = crate::core::gateway::backends::panic_threshold::get_panic_threshold_store()
            .is_panic_active(&service_key);
        if now_panic != last_panic {
            if now_panic {
                tracing::warn!(service = %service_key, "LB panic mode engaged: routing to all backends");
            } else {
                tracing::warn!(service = %service_key, "LB panic mode cleared");
            }
            last_panic = now_panic;
        }
```

- [ ] **Step 5: Run test + compile**

Run: `cargo test -p edgion panic_active_flag_roundtrips 2>&1 | tail -20`
Expected: PASS.
Run: `cargo check -p edgion --all-targets 2>&1 | tail -10`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/core/gateway/backends/panic_threshold.rs src/core/gateway/backends/selector.rs src/core/gateway/backends/health/check/manager.rs
git commit -m "feat(lb): best-effort off-path panic-mode edge log (active-HC services)"
```

---

## Task 9: Documentation (strong opt-in guidance — required)

**Files:**
- Modify: `skills/02-features/05-annotations/00-annotations-overview.md` (add `edgion.io/panic-threshold`)
- Modify: the EdgionGatewayConfig reference under `skills/02-features/02-config/` (add `loadBalancing.panicThreshold`)
- Modify: `docs/en/` annotation + gateway-config references if present (mirror the skills change)

- [ ] **Step 1: Document the annotation and global config**

Add an `edgion.io/panic-threshold` entry (value format: integer 0-100; 0 = disabled/default; per-service overrides global) and a `loadBalancing.panicThreshold` entry. Cross-link from the circuit-breaker and health-check sections so operators configuring either gate discover the panic safety valve.

- [ ] **Step 2: Add the opt-in guidance block**

Include this guidance verbatim near the annotation docs:

> **When to enable panic threshold.** Default is `0` (disabled), matching Istio's
> Kubernetes default, because a 50% threshold misfires on small pools — a 2-pod
> service losing 1 pod hits 50% and would route back to the bad pod. Enable it
> (e.g. `50`) for services with **larger** backend pools (≈ ≥ 5 endpoints) where
> losing many backends at once more likely means a systemic problem (a
> misconfigured health-check path, a network partition) than genuine failure.
> The canonical case: a renamed health-check endpoint marks every backend
> unhealthy and, without panic mode, takes the whole service offline with 503s.

- [ ] **Step 3: Run the docs consistency check**

Run: `make check-agent-docs 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add skills/ docs/
git commit -m "docs(lb): panic threshold annotation + global config + opt-in guidance"
```

---

## Task 10: Integration test — misconfigured health-check stays up under panic

**Files:**
- Create/modify: an integration scenario under `examples/test/.../integration/` (follow `skills/05-testing/01-integration-testing.md`)
- Modify: `skills/05-testing/` scenario index if required

- [ ] **Step 1: Author the scenario**

Define a Service with a backend pool (≥ 4 pods) and a health-check annotation whose
path does not exist (so all backends go unhealthy), plus
`edgion.io/panic-threshold: "50"`. Assert that requests still reach a backend
(2xx / connection established) rather than 503. Add a sibling case with no panic
annotation (default 0) asserting the pre-existing `not_found`/503 behavior.

- [ ] **Step 2: Run the targeted integration test**

Run: `./examples/test/scripts/integration/run_integration.sh --no-prepare -r Service -i <panic-item>`
Expected: PASS — panic-on service serves traffic; panic-off service returns the historical error.

- [ ] **Step 3: Commit**

```bash
git add examples/ skills/
git commit -m "test(lb): integration scenario for panic-threshold all-unhealthy fallback"
```

---

## Final verification

- [ ] `cargo fmt --all`
- [ ] `cargo check --all-targets` — clean
- [ ] `cargo clippy --all-targets` — clean
- [ ] `cargo test -p edgion panic 2>&1 | tail -30` — all panic-related unit tests pass
- [ ] `make check-agent-docs` — clean
- [ ] Update `tasks/todo/lb-01-no-panic-threshold.md` → move/mark done per the LB issues workflow (delete after fix, per project memory).
