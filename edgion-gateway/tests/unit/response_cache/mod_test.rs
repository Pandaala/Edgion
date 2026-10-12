use super::storage::{Epoch, RequestState, Store};
use pingora_http::ResponseHeader;
use std::sync::Arc;

pub(super) fn response() -> ResponseHeader {
    let mut response = ResponseHeader::build(200, Some(8)).unwrap();
    response.insert_header("cache-control", "public, max-age=60").unwrap();
    response
        .insert_header("date", httpdate::fmt_http_date(std::time::SystemTime::now()))
        .unwrap();
    response.insert_header("content-type", "application/json").unwrap();
    response
}

pub(super) fn fixture(cap: usize) -> (Arc<Store>, Arc<Epoch>, Box<RequestState>) {
    let store = Store::new(2, cap, 8 * 1024 * 1024, 2 * 1024 * 1024);
    let epoch = store.epoch();
    let state = RequestState::arm(Arc::clone(&store), Arc::clone(&epoch), [7; 32]).unwrap();
    (store, epoch, state)
}

pub(crate) struct Harness {
    store: Arc<Store>,
    epoch: Arc<Epoch>,
}

impl Harness {
    pub(crate) fn new(cap: usize) -> Self {
        let (store, epoch, _) = fixture(cap);
        Self { store, epoch }
    }

    pub(crate) fn local_store(&self) -> &Arc<Store> {
        &self.store
    }

    pub(crate) fn totals(&self) -> (usize, usize) {
        let stats = self.store.stats();
        (stats.0, stats.4)
    }

    pub(crate) fn capture_publication(&mut self, ctx: &mut crate::EdgionHttpContext) {
        use std::collections::HashMap;
        let publisher = crate::plugins::publication::PluginPublisher::new();
        let source = crate::test_current_status::from_value::<edgion_resources::Gateway>(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
            "metadata": {"namespace": "cache-intent", "name": "gateway"},
            "spec": {"gatewayClassName": "cache-intent", "listeners": [{"name": "http", "port": 18080, "protocol": "HTTP"}]}
        })).unwrap();
        publisher.replace_gateways(HashMap::from([(
            "cache-intent/gateway".into(),
            crate::plugins::publication::inputs::GatewayInput::from_gateway(&source),
        )]));
        let mut view = publisher
            .snapshot()
            .http_listener(&edgion_resources::ListenerKey("0.0.0.0:18080/HTTP".into()));
        drop(publisher);
        let token = self.store.publication_epoch();
        self.epoch = Arc::clone(token.guard.as_ref().unwrap());
        Arc::get_mut(&mut view)
            .expect("local publication has no other owners")
            .cache_epoch = token;
        ctx.capture_listener_publication(view);
    }

    pub(crate) fn stamp_unique_publication(
        &mut self,
        view: &mut Arc<crate::plugins::publication::HttpListenerPublication>,
    ) {
        let token = self.store.publication_epoch();
        self.epoch = Arc::clone(token.guard.as_ref().unwrap());
        Arc::get_mut(view)
            .expect("owned fixture publication has no competing readers")
            .cache_epoch = token;
    }

    pub(crate) fn arm(&self, ctx: &mut crate::EdgionHttpContext) {
        ctx.response_cache = RequestState::arm(Arc::clone(&self.store), Arc::clone(&self.epoch), [7; 32]);
    }

    pub(crate) fn entry_count(&self) -> usize {
        self.store.stats().3
    }

    pub(crate) fn lookup_count(&self) -> u64 {
        self.store.lookup_count()
    }

    pub(crate) fn corrupt_tail(&self, tail: &[u8]) {
        self.store.corrupt_tail(tail);
    }

    pub(crate) fn pending_bytes(&self) -> usize {
        self.store.stats().1
    }

    pub(crate) fn response(&self) -> ResponseHeader {
        response()
    }

    pub(crate) fn fill(&self, bytes: &'static [u8]) {
        let mut state = RequestState::arm(Arc::clone(&self.store), Arc::clone(&self.epoch), [7; 32]).unwrap();
        let header = response();
        state.capture(&header, std::time::Instant::now());
        state.push(Some(&bytes::Bytes::from_static(bytes)), true);
        assert!(state.prepare_publication(Some(&header)));
        state.publish();
    }
}

#[test]
fn no_policy_means_no_cache_owner_or_allocation() {
    let context = crate::EdgionHttpContext::default();
    assert!(context.response_cache.is_none());
    let (_, allocations) = crate::test_allocation_counter::count_allocations(|| {
        let mut context = context;
        crate::response_cache::lifecycle::capture_body(&mut context, None, false);
        crate::response_cache::lifecycle::abandon(&mut context);
    });
    assert_eq!(allocations, 0);
}
