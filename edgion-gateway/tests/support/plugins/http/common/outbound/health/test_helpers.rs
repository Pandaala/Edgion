// Test-only helpers for `OutboundHealth`, included into the health module so
// they can reach private state without widening production visibility.

impl OutboundHealth {
    /// A Service-pool health with injected endpoints. Its service key has no
    /// published section, so the injected snapshot is kept.
    pub(crate) fn with_endpoints_for_test(addrs: &[SocketAddr], passive: Option<&PassiveHealthCheck>) -> Arc<Self> {
        let health = Self::new(
            HealthTarget::Service {
                service_key: "outbound-health-test/absent".to_string(),
                port: Some(8080),
                scheme: "http",
            },
            passive,
        );
        let config = health.passive.as_ref().map(|p| p.resolved.clone());
        let dests = carry_over(&[], addrs.to_vec(), "http", config.as_ref());
        health.snapshot.store(Arc::new(DestSnapshot {
            section: None,
            dests: Arc::from(dests),
        }));
        health
    }

    /// Mark every current destination actively unhealthy.
    pub(crate) fn force_unhealthy_for_test(&self) {
        self.pool.active_down.store(true, Ordering::Release);
        for dest in self.current().dests.iter() {
            dest.active_down.store(true, Ordering::Release);
        }
    }

    /// Whether every current destination (and the pool) is fully usable.
    pub(crate) fn all_usable_for_test(&self) -> bool {
        let usable = |dest: &DestHealth| !dest.active_down.load(Ordering::Acquire) && !dest.state.is_open();
        usable(&self.pool) && self.current().dests.iter().all(|dest| usable(dest))
    }
}
