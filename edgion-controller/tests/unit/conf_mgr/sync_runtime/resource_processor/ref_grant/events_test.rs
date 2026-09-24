use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestListener {
    call_count: AtomicUsize,
}

#[async_trait::async_trait]
impl RevalidationListener for TestListener {
    async fn on_reference_grant_changed(&self, _event: &ReferenceGrantChangedEvent) {
        self.call_count.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn test_dispatcher_registers_and_dispatches() {
    let dispatcher = ReferenceGrantEventDispatcher::new();
    let listener = Arc::new(TestListener {
        call_count: AtomicUsize::new(0),
    });

    dispatcher.register_listener(listener.clone());

    let event = ReferenceGrantChangedEvent {
        affected_namespaces: HashSet::new(),
    };

    dispatcher.dispatch(&event).await;

    assert_eq!(listener.call_count.load(Ordering::SeqCst), 1);
}
