#[cfg(test)]
impl StreamPluginStore {
    pub fn new() -> Self {
        Self {
            isolated: Some(Arc::new(crate::plugins::publication::PluginPublisher::new())),
        }
    }

    fn prepare_test_scope(&self, keys: HashSet<String>) {
        if self.isolated.is_none() {
            return;
        }
        use crate::plugins::publication::inputs::GatewayInput;
        use edgion_resources::{Gateway, GatewayClass};
        let class: GatewayClass = serde_json::from_value(serde_json::json!({
            "apiVersion":"gateway.networking.k8s.io/v1","kind":"GatewayClass","metadata":{"name":"stream-store-test"},
            "spec":{"controllerName":"edgion.io/gateway-controller"}
        }))
        .unwrap();
        let gateway: Gateway = crate::test_current_status::from_value(serde_json::json!({
            "apiVersion":"gateway.networking.k8s.io/v1","kind":"Gateway","metadata":{"name":"scope","namespace":"stream-store-test"},
            "spec":{"gatewayClassName":"stream-store-test","listeners":[{"name":"tcp","port":18089,"protocol":"TCP"}]}
        })).unwrap();
        let base = GatewayInput::from_gateway(&gateway);
        let mut gateways = HashMap::from([("stream-store-test/scope".into(), base.clone())]);
        for (index, key) in keys.into_iter().enumerate() {
            let mut input = base.clone();
            input.connection_key = Some(key);
            gateways.insert(format!("stream-store-test/attachment-{index}"), input);
        }
        self.publisher()
            .replace_gateway_classes(HashMap::from([("stream-store-test".into(), Arc::new(class))]));
        self.publisher().replace_gateways(gateways);
    }

    pub fn get(&self, key: &str) -> Option<Arc<CompiledStreamPlugins>> {
        self.publisher()
            .snapshot()
            .gateway("stream-store-test/scope")?
            .scope
            .stream(key)
    }
}
