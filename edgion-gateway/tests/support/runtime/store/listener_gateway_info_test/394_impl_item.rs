#[cfg(test)]
impl Drop for TestListenerIndexGuard {
    fn drop(&mut self) {
        TEST_INDEX_OVERRIDES.with(|overrides| {
            let mut overrides = overrides.borrow_mut();
            if let Some(previous) = self.previous.take() {
                overrides.insert(self.key.clone(), previous);
            } else {
                overrides.remove(&self.key);
            }
        });
    }
}
