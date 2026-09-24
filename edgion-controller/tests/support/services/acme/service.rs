use super::*;

/// Test-only serialization lock for the process-global `ACME_SERVICE`.
///
/// `test_timer_generation_isolation` installs a real handle into `ACME_SERVICE`
/// and asserts nothing else enqueues into its receiver within a time window.
/// Any concurrently running test that drives `EdgionAcmeHandler::after_publish` reaches
/// `notify_resource_changed` -> `get_sender()`, which clones whatever handle is
/// currently installed and sends into it — landing a stray message in the
/// isolation test's receiver and tripping its assertion. Every test that touches
/// the global (installs a handle, or reads it via the notify path) must hold this
/// lock so they run serially. Held across `.await`, hence `tokio::sync::Mutex`.
#[cfg(test)]
pub(crate) static ACME_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Captures real notification sends without starting a worker or performing IO.
/// Callers must hold `ACME_TEST_MUTEX` for the capture's entire lifetime.
#[cfg(test)]
pub(crate) struct NotificationCapture {
    rx: mpsc::Receiver<AcmeCommand>,
    pub(super) generation: u64,
}

#[cfg(test)]
impl NotificationCapture {
    pub(crate) fn install() -> Self {
        let (tx, rx) = mpsc::channel(64);
        let generation = ACME_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let mut service = ACME_SERVICE.lock();
        assert!(service.is_none(), "notification capture requires an idle service");
        *service = Some(AcmeServiceHandle {
            tx,
            generation,
            cancel: CancellationToken::new(),
        });
        Self { rx, generation }
    }

    pub(crate) fn drain(&mut self) -> Vec<String> {
        let mut keys = Vec::new();
        while let Ok(command) = self.rx.try_recv() {
            match command {
                AcmeCommand::ResourceChanged { key } => keys.push(key),
                command => panic!("unexpected ACME command: {command:?}"),
            }
        }
        keys
    }
}

#[cfg(test)]
impl Drop for NotificationCapture {
    fn drop(&mut self) {
        let mut service = ACME_SERVICE.lock();
        if service
            .as_ref()
            .is_some_and(|handle| handle.generation == self.generation)
        {
            service.take();
        }
    }
}
