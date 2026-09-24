#[cfg(test)]
impl Drop for TestTickTaskGuard {
    fn drop(&mut self) {
        self.state.active_tick_tasks.fetch_sub(1, Ordering::AcqRel);
    }
}
