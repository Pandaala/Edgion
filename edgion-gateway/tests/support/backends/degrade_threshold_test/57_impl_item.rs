#[cfg(test)]
impl DegradeTestState {
    fn capture() -> Self {
        Self {
            default: get_lb_degrade_default(),
            overrides: get_degrade_threshold_store()
                .services
                .iter()
                .map(|entry| (entry.key().clone(), *entry.value()))
                .collect(),
        }
    }
}
