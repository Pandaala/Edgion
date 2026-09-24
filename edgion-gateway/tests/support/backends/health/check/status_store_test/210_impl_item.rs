#[cfg(test)]
impl HealthStatusStore {
    fn service_count(&self) -> usize {
        self.services.load().len()
    }

    fn unhealthy_count(&self, service_key: &str) -> usize {
        self.services
            .load()
            .get(service_key)
            .map(|svc| svc.unhealthy_backends.load().len())
            .unwrap_or(0)
    }
}
