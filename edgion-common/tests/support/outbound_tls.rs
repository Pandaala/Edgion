use super::*;

static OUTBOUND_TLS_TEST_MUTEX: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Serializes tests that resolve the process-wide outbound TLS default and
/// gives each test a clean default without leaking mutations to the next test.
pub(crate) struct OutboundTlsTestGuard {
    _lock: parking_lot::MutexGuard<'static, ()>,
    previous: OutboundTlsConfig,
}

impl Drop for OutboundTlsTestGuard {
    fn drop(&mut self) {
        set_outbound_global_default(std::mem::take(&mut self.previous));
    }
}

pub(crate) fn outbound_tls_test_guard() -> OutboundTlsTestGuard {
    let lock = OUTBOUND_TLS_TEST_MUTEX.lock();
    let previous = outbound_global_default();
    set_outbound_global_default(OutboundTlsConfig::default());
    OutboundTlsTestGuard { _lock: lock, previous }
}
