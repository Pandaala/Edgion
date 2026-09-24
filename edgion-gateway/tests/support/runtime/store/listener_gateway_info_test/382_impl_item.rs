#[cfg(test)]
impl TestListenerIndexGuard {
    pub(crate) fn install(key: ListenerKey, index: Arc<GatewayInfoIndex>) -> Self {
        let previous = TEST_INDEX_OVERRIDES.with(|overrides| overrides.borrow_mut().insert(key.clone(), index));
        Self {
            key,
            previous,
            _thread_bound: std::marker::PhantomData,
        }
    }
}
