#[cfg(test)]
impl WasmRuntime {
    async fn backdate_oci_check_for_test(&self) {
        self.state.slot.refresh_due_ms.store(0, Ordering::Release);
    }
    async fn wait_for_oci_refresh_for_test(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.state.slot.refresh_in_flight.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("OCI refresh finishes within deadline");
    }
    fn backdate_load_retry_for_test(&self) {
        self.state.slot.load_retry_due_ms.store(0, Ordering::Release);
    }
    async fn wait_for_preload_for_test(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.state.slot.preload_in_flight.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("preload finishes within deadline");
    }
}
