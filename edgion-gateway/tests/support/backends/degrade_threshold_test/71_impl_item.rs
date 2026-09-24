#[cfg(test)]
impl Drop for DegradeTestState {
    fn drop(&mut self) {
        let store = get_degrade_threshold_store();
        store.services.clear();
        for (key, value) in self.overrides.drain(..) {
            store.services.insert(key, value);
        }
        GLOBAL_DEFAULT.store(self.default, Ordering::Relaxed);
        crate::backends::discovery::get_service_discovery_coordinator().refresh_all_degrade_thresholds();
    }
}
