#[cfg(test)]
impl TestTickTaskGuard {
    fn new(state: Arc<TickCalloutState>) -> Self {
        state.active_tick_tasks.fetch_add(1, Ordering::AcqRel);
        Self { state }
    }
}
