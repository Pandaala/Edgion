use super::*;
use edgion_resources::resources::gateway_class::GatewayClassSpec;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use std::collections::HashMap;

static TEST_MUTEX: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

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

fn clear_store() {
    GATEWAY_CLASS_STORE.write().clear();
}

#[test]
fn test_full_set_replaces_existing() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = GatewayClassHandler::new();

    let mut data = HashMap::new();
    data.insert("foo".to_string(), Arc::new(make_gateway_class("foo")));
    data.insert("bar".to_string(), Arc::new(make_gateway_class("bar")));
    handler.full_set(&data);
    assert_eq!(count_gateway_classes(), 2);

    let mut data2 = HashMap::new();
    data2.insert("baz".to_string(), Arc::new(make_gateway_class("baz")));
    handler.full_set(&data2);
    assert_eq!(count_gateway_classes(), 1);
}

#[test]
fn test_add_deduplicates() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = GatewayClassHandler::new();

    let mut add = HashMap::new();
    add.insert("foo".to_string(), Arc::new(make_gateway_class("foo")));
    handler.partial_update(add.clone(), HashMap::new(), std::collections::HashSet::new());
    handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

    assert_eq!(count_gateway_classes(), 1, "duplicate add must not create two entries");
}

#[test]
fn test_update_overwrites_existing() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = GatewayClassHandler::new();

    let mut add = HashMap::new();
    add.insert("foo".to_string(), Arc::new(make_gateway_class("foo")));
    handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

    let mut updated = make_gateway_class("foo");
    updated.spec.controller_name = "updated/controller".to_string();
    let mut upd = HashMap::new();
    upd.insert("foo".to_string(), Arc::new(updated));
    handler.partial_update(HashMap::new(), upd, std::collections::HashSet::new());

    let result = get_gateway_class_by_name("foo").expect("must exist after update");
    assert_eq!(result.spec.controller_name, "updated/controller");
    assert_eq!(count_gateway_classes(), 1);
}

#[test]
fn test_update_miss_inserts() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = GatewayClassHandler::new();

    let mut upd = HashMap::new();
    upd.insert("ghost".to_string(), Arc::new(make_gateway_class("ghost")));
    handler.partial_update(HashMap::new(), upd, std::collections::HashSet::new());

    assert_eq!(count_gateway_classes(), 1, "update miss must insert");
}

#[test]
fn test_remove_works() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = GatewayClassHandler::new();

    let mut add = HashMap::new();
    add.insert("foo".to_string(), Arc::new(make_gateway_class("foo")));
    add.insert("bar".to_string(), Arc::new(make_gateway_class("bar")));
    handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

    let mut remove = std::collections::HashSet::new();
    remove.insert("foo".to_string());
    handler.partial_update(HashMap::new(), HashMap::new(), remove);

    assert_eq!(count_gateway_classes(), 1);
    assert!(get_gateway_class_by_name("foo").is_none());
    assert!(get_gateway_class_by_name("bar").is_some());
}

#[test]
fn test_remove_miss_does_not_panic() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = GatewayClassHandler::new();

    let mut remove = std::collections::HashSet::new();
    remove.insert("nonexistent".to_string());
    handler.partial_update(HashMap::new(), HashMap::new(), remove);
    assert_eq!(count_gateway_classes(), 0);
}

#[test]
fn test_list_and_count() {
    let _g = TEST_MUTEX.lock();
    clear_store();
    let handler = GatewayClassHandler::new();

    let mut add = HashMap::new();
    add.insert("a".to_string(), Arc::new(make_gateway_class("a")));
    add.insert("b".to_string(), Arc::new(make_gateway_class("b")));
    add.insert("c".to_string(), Arc::new(make_gateway_class("c")));
    handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

    assert_eq!(count_gateway_classes(), 3);
    assert_eq!(list_gateway_classes().len(), 3);
}
