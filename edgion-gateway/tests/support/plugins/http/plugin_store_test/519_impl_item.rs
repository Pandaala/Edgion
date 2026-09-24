#[cfg(test)]
impl PluginStore {
    pub fn new() -> Self {
        let store = Self {
            isolated: Some(Arc::new(crate::plugins::publication::PluginPublisher::new())),
        };
        store.prepare_test_scope(HashSet::new());
        store
    }

    pub(crate) fn prepare_test_scope(&self, keys: HashSet<String>) {
        if self.isolated.is_none() {
            return;
        }
        use crate::plugins::publication::inputs::GatewayInput;
        use edgion_resources::{EdgionGatewayConfig, Gateway, GatewayClass};
        let publisher = self.publisher();
        let mut config: EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
            "apiVersion":"edgion.io/v1", "kind":"EdgionGatewayConfig", "metadata":{"name":"plugin-store-test"},
            "spec":{"globalPluginsRef": keys.iter().map(|key| {
                let (namespace, name) = key.split_once('/').unwrap_or(("default",key));
                serde_json::json!({"namespace":namespace,"name":name})
            }).collect::<Vec<_>>()}
        }))
        .unwrap();
        let class: GatewayClass = serde_json::from_value(serde_json::json!({
            "apiVersion":"gateway.networking.k8s.io/v1","kind":"GatewayClass","metadata":{"name":"plugin-store-test"},
            "spec":{"controllerName":"edgion.io/gateway-controller","parametersRef":{"group":"edgion.io","kind":"EdgionGatewayConfig","name":"plugin-store-test"}}
        })).unwrap();
        let gateway: Gateway = crate::test_current_status::from_value(serde_json::json!({
            "apiVersion":"gateway.networking.k8s.io/v1","kind":"Gateway","metadata":{"name":"scope","namespace":"plugin-store-test"},
            "spec":{"gatewayClassName":"plugin-store-test","listeners":[{"name":"http","port":18089,"protocol":"HTTP"}]}
        })).unwrap();
        config.spec.plugin_policy = publisher
            .snapshot()
            .gateway("plugin-store-test/scope")
            .and_then(|gateway| {
                gateway
                    .scope
                    .config
                    .as_ref()
                    .and_then(|config| config.spec.plugin_policy.clone())
            });
        publisher.replace_gateway_configs(HashMap::from([("plugin-store-test".into(), Arc::new(config))]));
        publisher.replace_gateway_classes(HashMap::from([("plugin-store-test".into(), Arc::new(class))]));
        publisher.replace_gateways(HashMap::from([(
            "plugin-store-test/scope".into(),
            GatewayInput::from_gateway(&gateway),
        )]));
    }

    pub(crate) fn scope(&self) -> Arc<crate::plugins::publication::PluginScope> {
        self.publisher()
            .snapshot()
            .gateway("plugin-store-test/scope")
            .unwrap()
            .scope
            .clone()
    }

    pub(crate) fn set_test_policy(
        &self,
        policy: Option<edgion_resources::resources::plugin_policy::PluginPolicyConfig>,
    ) {
        let mut config = self.scope().config.as_ref().unwrap().as_ref().clone();
        config.spec.plugin_policy = policy;
        self.publisher()
            .replace_gateway_configs(HashMap::from([("plugin-store-test".into(), Arc::new(config))]));
    }

    pub fn with_plugin<R>(&self, key: &str, f: impl FnOnce(&CompiledPlugins) -> R) -> Option<R> {
        self.scope().http(key).map(|compiled| f(&compiled))
    }

    pub fn get_plugin_runtime(&self, key: &str) -> Option<Arc<PluginRuntime>> {
        self.with_plugin(key, |compiled| Arc::clone(&compiled.plugin_runtime))
    }

    pub fn replace_all(&self, plugins: HashMap<String, Arc<CompiledPlugins>>) {
        use edgion_common::conf_sync::traits::ConfHandler;
        self.full_set(
            &plugins
                .into_iter()
                .map(|(key, compiled)| (key, Arc::clone(&compiled.spec)))
                .collect(),
        );
    }

    pub(crate) fn apply_delta(
        &self,
        add: HashMap<String, Arc<CompiledPlugins>>,
        update: HashMap<String, Arc<CompiledPlugins>>,
        remove: &HashSet<String>,
    ) {
        use edgion_common::conf_sync::traits::ConfHandler;
        self.partial_update(
            add.into_iter()
                .map(|(key, compiled)| (key, Arc::clone(&compiled.spec)))
                .collect(),
            update
                .into_iter()
                .map(|(key, compiled)| (key, Arc::clone(&compiled.spec)))
                .collect(),
            remove.clone(),
        );
    }
}
