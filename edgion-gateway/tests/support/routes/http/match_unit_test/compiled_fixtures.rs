// Test fixtures own the unchanged rule before invoking the production compiler.
pub(crate) fn compile_http_fixture(
    rule: &HTTPRouteRule,
    namespace: &str,
) -> Result<HttpRuleRuntime, edgion_resources::RequestTransformViolation> {
    let mut route: HTTPRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "test-route", "namespace": namespace}, "spec": {}
    }))
    .expect("the fixture has a valid Route envelope");
    route.spec.rules = Some(vec![rule.clone()]);
    HttpRouteRuleUnit::compile_http_rule(&route, 0)
}

pub(crate) fn compile_http_backend_fixture(
    schema: HTTPBackendRef,
    namespace: &str,
    slot: usize,
    runtime: Arc<PluginRuntime>,
) -> CompiledBackend {
    let mut rule: HTTPRouteRule = serde_json::from_value(serde_json::json!({})).expect("empty rule fixture");
    let mut refs = (0..slot)
        .map(|index| {
            let mut placeholder = schema.clone();
            placeholder.name = format!("fixture-slot-{index}");
            placeholder
        })
        .collect::<Vec<_>>();
    refs.push(schema);
    rule.backend_refs = Some(refs);
    let mut backends = compile_http_fixture(&rule, namespace)
        .expect("valid owning fixture")
        .backends;
    let compiled = &mut Arc::get_mut(&mut backends).expect("new fixture is uniquely owned")[slot];
    // Existing runtime-wiring tests supply their own plugin; identity stays immutable.
    compiled.plugin_runtime = runtime;
    compiled.clone()
}
