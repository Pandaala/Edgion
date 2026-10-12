use super::*;
use crate::plugins::publication::inputs::GatewayInput;
use crate::plugins::publication::{PluginPublication, PluginPublisher, PluginScope};
use edgion_resources::resources::gateway_class::{GatewayClassSpec, ParametersReference};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use std::sync::LazyLock;

static TEST_MUTEX: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
static TEST_PUBLISHER: LazyLock<PluginPublisher> = LazyLock::new(PluginPublisher::new);
static RECONCILED: parking_lot::Mutex<Option<Arc<PluginPublication>>> = parking_lot::Mutex::new(None);

pub(super) fn global_publisher() -> &'static PluginPublisher {
    &TEST_PUBLISHER
}

// This witnesses callback ordering; real adaptive behavior has its own regressions.
pub(super) fn reconcile_all_adaptive_plugin_dependents() {
    *RECONCILED.lock() = Some(TEST_PUBLISHER.snapshot());
}

fn make_gateway_class(name: &str) -> GatewayClass {
    GatewayClass {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: GatewayClassSpec {
            current_status: None,
            controller_name: "test/controller".to_string(),
            description: None,
            parameters_ref: None,
        },
        status: None,
    }
}

fn setup() -> GatewayClassHandler {
    TEST_PUBLISHER.replace_gateway_classes(HashMap::new());
    TEST_PUBLISHER.replace_gateway_configs(HashMap::new());
    TEST_PUBLISHER.replace_gateways(
        ["foo", "bar", "baz", "ghost", "a", "b", "c"]
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let gateway = crate::test_current_status::from_value::<edgion_resources::Gateway>(serde_json::json!({
            "metadata": {"name": name, "namespace": "test"},
            "spec": {"gatewayClassName": name, "listeners": [{"name": "http", "port": 18080 + i, "protocol": "HTTP"}]}
        })).unwrap();
                (format!("test/{name}"), GatewayInput::from_gateway(&gateway))
            })
            .collect(),
    );
    *RECONCILED.lock() = None;
    GatewayClassHandler::new()
}

fn classes(names: &[&str]) -> HashMap<String, Arc<GatewayClass>> {
    names
        .iter()
        .map(|name| (name.to_string(), Arc::new(make_gateway_class(name))))
        .collect()
}

fn scope(name: &str) -> Arc<PluginScope> {
    TEST_PUBLISHER
        .snapshot()
        .gateway(&format!("test/{name}"))
        .unwrap()
        .scope
        .clone()
}

fn assert_reconciled_after_publication() {
    assert!(Arc::ptr_eq(
        RECONCILED.lock().as_ref().expect("callback must reconcile"),
        &TEST_PUBLISHER.snapshot()
    ));
}

#[test]
fn test_full_set_replaces_existing() {
    let _g = TEST_MUTEX.lock();
    let handler = setup();
    handler.full_set(&classes(&["foo", "bar"]));
    assert!(scope("foo").available && scope("bar").available);
    assert_reconciled_after_publication();
    let captured = scope("foo");
    handler.full_set(&classes(&["baz"]));
    assert!(!scope("foo").available && !scope("bar").available && scope("baz").available);
    assert!(captured.available, "admitted requests retain their captured scope");
    assert_reconciled_after_publication();
    handler.full_set(&HashMap::new());
    assert!(!scope("baz").available);
    assert_reconciled_after_publication();
    handler.full_set(&classes(&["foo"]));
    assert!(scope("foo").available);
    assert_reconciled_after_publication();
}

#[test]
fn test_add_deduplicates() {
    let _g = TEST_MUTEX.lock();
    let handler = setup();
    handler.partial_update(classes(&["foo"]), HashMap::new(), HashSet::new());
    let first = scope("foo");
    handler.partial_update(classes(&["foo"]), HashMap::new(), HashSet::new());
    assert!(
        Arc::ptr_eq(&first, &scope("foo")),
        "equivalent input retains the published scope"
    );
    assert_reconciled_after_publication();
}

#[test]
fn test_update_overwrites_existing() {
    let _g = TEST_MUTEX.lock();
    let handler = setup();
    handler.partial_update(classes(&["foo"]), HashMap::new(), HashSet::new());
    let captured = scope("foo");
    let mut changed = make_gateway_class("foo");
    changed.spec.parameters_ref = Some(
        serde_json::from_value::<ParametersReference>(serde_json::json!({
            "group": "edgion.io", "kind": "EdgionGatewayConfig", "name": "missing"
        }))
        .unwrap(),
    );
    handler.partial_update(
        HashMap::new(),
        HashMap::from([("foo".into(), Arc::new(changed))]),
        HashSet::new(),
    );
    assert!(
        !scope("foo").available,
        "new class binding must replace the old generation"
    );
    assert!(captured.available);
    assert_reconciled_after_publication();
    let config = Arc::new(
        serde_json::from_value::<edgion_resources::EdgionGatewayConfig>(serde_json::json!({
            "metadata": {"name": "bound"}, "spec": {}
        }))
        .unwrap(),
    );
    TEST_PUBLISHER.replace_gateway_configs(HashMap::from([("bound".into(), Arc::clone(&config))]));
    let mut repaired = make_gateway_class("foo");
    repaired.spec.parameters_ref = Some(
        serde_json::from_value(serde_json::json!({
            "group": "edgion.io", "kind": "EdgionGatewayConfig", "name": "bound"
        }))
        .unwrap(),
    );
    handler.partial_update(
        HashMap::new(),
        HashMap::from([("foo".into(), Arc::new(repaired))]),
        HashSet::new(),
    );
    let repaired_scope = scope("foo");
    assert!(repaired_scope.available);
    assert!(
        Arc::ptr_eq(repaired_scope.config.as_ref().unwrap(), &config),
        "scope retains the selected EGC Arc"
    );
    assert_reconciled_after_publication();
}

#[test]
fn test_update_miss_inserts() {
    let _g = TEST_MUTEX.lock();
    let handler = setup();
    assert!(!scope("ghost").available);
    handler.partial_update(HashMap::new(), classes(&["ghost"]), HashSet::new());
    assert!(scope("ghost").available);
    assert_reconciled_after_publication();
}

#[test]
fn test_remove_works() {
    let _g = TEST_MUTEX.lock();
    let handler = setup();
    handler.partial_update(classes(&["foo", "bar"]), HashMap::new(), HashSet::new());
    let captured = scope("foo");
    handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["foo".into()]));
    assert!(!scope("foo").available && scope("bar").available);
    assert!(captured.available);
    assert_reconciled_after_publication();
    handler.partial_update(classes(&["foo"]), HashMap::new(), HashSet::new());
    assert!(scope("foo").available && scope("bar").available);
    assert_reconciled_after_publication();
}

#[test]
fn test_remove_miss_does_not_panic() {
    let _g = TEST_MUTEX.lock();
    let handler = setup();
    handler.full_set(&classes(&["bar"]));
    let sibling = scope("bar");
    handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["nonexistent".into()]));
    assert!(
        Arc::ptr_eq(&sibling, &scope("bar")),
        "absent deletion preserves unrelated publication"
    );
    assert!(!scope("foo").available);
    assert_reconciled_after_publication();
}

#[test]
fn test_list_and_count() {
    let _g = TEST_MUTEX.lock();
    let handler = setup();
    handler.partial_update(classes(&["a", "b", "c"]), HashMap::new(), HashSet::new());
    assert!(["a", "b", "c"].into_iter().all(|name| scope(name).available));
    assert!(!scope("foo").available);
    assert_reconciled_after_publication();
}
