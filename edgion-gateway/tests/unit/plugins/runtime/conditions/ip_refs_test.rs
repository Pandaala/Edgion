use super::*;
use crate::plugins::runtime::edgion_config_data::EdgionConfigDataStore;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::edgion_plugins::conditions::Condition;
use serde_json::json;

fn owner(namespace: &str, target_namespace: &str) -> EdgionPlugins {
    serde_json::from_value(json!({
        "metadata":{"name":"policy","namespace":namespace},
        "spec":{"requestPlugins":[{"type":"Mock","config":{"status":418},
            "conditions":{"run":{"allOf":[{"type":"remoteIpMatch","ipRefs":[{"name":"networks","namespace":target_namespace}]}]}}
        }]}
    })).unwrap()
}
fn predicate(owner: &EdgionPlugins) -> Arc<BoundIpPredicate> {
    let compiler = IpConditionCompiler::new(owner).unwrap();
    compiler.required()[0].clone()
}
fn publish(store: &EdgionConfigDataStore, namespace: &str, cidr: &str, visibility: &str) {
    let data = serde_json::from_value(json!({
        "metadata":{"name":"networks","namespace":namespace},
        "spec":{"visibility":visibility,"data":{"type":"IpList","config":{"items":[{"name":"main","cidrs":[cidr]}]}}}
    }))
    .unwrap();
    store.full_set(&HashMap::from([(format!("{namespace}/networks"), Arc::new(data))]));
}
fn session(store: &EdgionConfigDataStore) -> MockPluginSession {
    let view = Arc::new(store.capture_snapshot());
    let mut session = MockPluginSession::new();
    let availability = view.clone();
    session
        .expect_ip_list_availability()
        .returning(move |key| availability.ip_list_availability(key));
    session
        .expect_ip_list_contains()
        .returning(move |key, ip| view.ip_list_contains(key, ip));
    session
}

#[test]
fn actual_owner_live_membership_and_missing_recovery_keep_the_same_predicate() {
    let owner = owner("default", "default");
    let predicate = predicate(&owner);
    let store = EdgionConfigDataStore::new();
    let ip = Some("192.0.2.1".parse().unwrap());
    assert_eq!(predicate.evaluate(&session(&store), ip, true), MatchState::Unresolved);
    publish(&store, "default", "192.0.2.0/24", "Namespace");
    let held = session(&store);
    assert_eq!(predicate.evaluate(&held, ip, false), MatchState::True);
    publish(&store, "default", "198.51.100.0/24", "Namespace");
    assert_eq!(
        predicate.evaluate(&held, ip, false),
        MatchState::True,
        "one decision keeps its generation"
    );
    assert_eq!(predicate.evaluate(&session(&store), ip, false), MatchState::False);
    store.full_set(&HashMap::new());
    assert!(!predicate.available(&session(&store)));
    publish(&store, "default", "192.0.2.0/24", "Namespace");
    assert_eq!(predicate.evaluate(&session(&store), ip, false), MatchState::True);
}

#[test]
fn absent_cross_namespace_proof_cannot_read_a_new_private_target_or_visibility_flip() {
    let owner = owner("source", "target");
    let predicate = predicate(&owner);
    let store = EdgionConfigDataStore::new();
    let ip = Some("192.0.2.1".parse().unwrap());
    publish(&store, "target", "192.0.2.0/24", "Namespace");
    assert_eq!(predicate.evaluate(&session(&store), ip, true), MatchState::Unresolved);
    publish(&store, "target", "192.0.2.0/24", "Cluster");
    let held = session(&store);
    assert_eq!(predicate.evaluate(&held, ip, false), MatchState::True);
    publish(&store, "target", "192.0.2.0/24", "Namespace");
    assert_eq!(predicate.evaluate(&session(&store), ip, false), MatchState::Unresolved);
    assert_eq!(predicate.evaluate(&held, ip, false), MatchState::True);
}

#[test]
fn denied_scope_stays_unavailable_until_the_owner_is_recompiled() {
    let mut owner: EdgionPlugins = serde_json::from_value(json!({
        "metadata":{"name":"policy","namespace":"default"},
        "spec":{"requestPlugins":[{"type":"Mock","config":{"status":418},
            "conditions":{"run":{"allOf":[{"type":"remoteIpMatch","ipRefs":[
                {"name":"networks"},{"name":"networks"}
            ]}]}}
        }]}
    }))
    .unwrap();
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    prepared
        .visit_ip_mut(
            &mut owner,
            |mut leaf, _| {
                leaf.set_read_scopes(&[IpReadScope::SameNamespace, IpReadScope::Denied])
                    .unwrap()
            },
            |_, _| {},
        )
        .unwrap();
    let denied = predicate(&owner);
    let store = EdgionConfigDataStore::new();
    assert!(!denied.available(&session(&store)));
    publish(&store, "default", "192.0.2.0/24", "Namespace");
    let held = store.capture_snapshot();
    assert!(!denied.available_in(&held));
    let reads = std::cell::Cell::new(0);
    assert!(!denied.available_by(|key| {
        reads.set(reads.get() + 1);
        held.ip_list_availability(key)
    }));
    assert_eq!(reads.get(), 0, "immutable denied scope needs no snapshot reads");
    assert_eq!(
        denied.evaluate(&session(&store), Some("192.0.2.1".parse().unwrap()), true),
        MatchState::Unresolved
    );
    store.full_set(&HashMap::new());
    assert!(!denied.available_in(&held));
    assert!(!denied.available(&session(&store)));
    publish(&store, "default", "192.0.2.0/24", "Cluster");
    assert!(!denied.available(&session(&store)));

    prepared
        .visit_ip_mut(
            &mut owner,
            |mut leaf, _| leaf.set_read_scopes(&[IpReadScope::SameNamespace; 2]).unwrap(),
            |_, _| {},
        )
        .unwrap();
    assert!(predicate(&owner).available(&session(&store)));
    assert!(
        !denied.available(&session(&store)),
        "retained owner keeps its denied scope"
    );
}

#[test]
fn stale_permission_stamp_and_unbound_clone_are_not_usable() {
    let mut owner = owner("default", "default");
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    prepared
        .visit_ip_mut(
            &mut owner,
            |mut leaf, _| leaf.set_read_scopes(&[IpReadScope::SameNamespace]).unwrap(),
            |_, _| {},
        )
        .unwrap();
    let compiler = IpConditionCompiler::new(&owner).unwrap();
    let entries = owner.spec.request_plugins.as_ref().unwrap();
    let conditions = entries[0].conditions.as_ref().unwrap().run.as_ref().unwrap();
    let edgion_resources::resources::edgion_plugins::conditions::ConditionSet::AllOf { all_of } = conditions else {
        unreachable!()
    };
    let Condition::RemoteIpMatch(ip) = &all_of[0] else {
        unreachable!()
    };
    assert!(compiler.bind(&ip.refs).is_some());
    assert!(compiler.bind(&ip.refs.clone()).is_none());
    drop(compiler);
    owner.metadata.name = Some("retargeted-owner".into());
    assert!(IpConditionCompiler::new(&owner).is_err());
}

#[test]
fn optional_rap_empty_union_keeps_primary_positive_and_secondary_negation() {
    let owner: EdgionPlugins = serde_json::from_value(json!({
        "metadata":{"name":"rap","namespace":"default"},
        "spec":{"requestPlugins":[{"type":"RequestAccessPolicy","config":{
            "defaultProfile":"main","profiles":{},"resolvedProfiles":{"main":{"requiredRuleGroups":[{
                "name":"clients","anyOfRules":[{"name":"peer","type":"directPeerIp","config":{
                    "ipRefs":[{"name":"networks"}],"conditions":{"allOf":[{
                        "type":"remoteIpMatch","ipRefs":[{"name":"networks"}],"negate":true
                    }]}
                }}]
            }]}}
        }}]}
    }))
    .unwrap();
    let compiler = IpConditionCompiler::new(&owner).unwrap();
    assert!(compiler.required().is_empty());
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    let mut predicates = Vec::new();
    prepared
        .visit_ip(&owner, |leaf| predicates.push(compiler.bind(leaf.config()).unwrap()))
        .unwrap();
    assert_eq!(predicates.len(), 2);
    let store = EdgionConfigDataStore::new();
    let session = session(&store);
    let ip = Some("192.0.2.1".parse().unwrap());
    assert_eq!(predicates[0].evaluate(&session, ip, false), MatchState::False);
    assert_eq!(predicates[1].evaluate(&session, ip, true), MatchState::True);
}

#[test]
fn optional_rap_denied_sibling_preserves_usable_membership() {
    let mut owner: EdgionPlugins = serde_json::from_value(json!({
        "metadata":{"name":"rap","namespace":"default"},
        "spec":{"requestPlugins":[{"type":"RequestAccessPolicy","config":{
            "defaultProfile":"main","profiles":{},"resolvedProfiles":{"main":{"requiredRuleGroups":[{
                "name":"clients","anyOfRules":[{"name":"peer","type":"directPeerIp","config":{
                    "ipRefs":[{"name":"networks"},{"name":"networks"}]
                }}]
            }]}}
        }}]}
    }))
    .unwrap();
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    prepared
        .visit_ip_mut(
            &mut owner,
            |mut leaf, _| {
                leaf.set_read_scopes(&[IpReadScope::SameNamespace, IpReadScope::Denied])
                    .unwrap()
            },
            |_, _| {},
        )
        .unwrap();
    let compiler = IpConditionCompiler::new(&owner).unwrap();
    assert!(compiler.required().is_empty());
    let mut predicates = Vec::new();
    prepared
        .visit_ip(&owner, |leaf| predicates.push(compiler.bind(leaf.config()).unwrap()))
        .unwrap();
    assert_eq!(predicates.len(), 1);
    let predicate = &predicates[0];
    let store = EdgionConfigDataStore::new();
    let ip = Some("192.0.2.1".parse().unwrap());
    publish(&store, "default", "192.0.2.0/24", "Namespace");
    assert_eq!(predicate.evaluate(&session(&store), ip, false), MatchState::True);
    assert_eq!(predicate.evaluate(&session(&store), ip, true), MatchState::False);
    store.full_set(&HashMap::new());
    assert_eq!(predicate.evaluate(&session(&store), ip, false), MatchState::False);
    assert_eq!(predicate.evaluate(&session(&store), ip, true), MatchState::True);
}

#[test]
fn positive_scope_proofs_keep_identity_and_current_target_state_gates() {
    for scope in [IpReadScope::GrantAuthorized, IpReadScope::ValidationDisabled] {
        let mut owner = owner("source", "target");
        let prepared = PreparedConditionReferences::from_plugins(&owner);
        prepared
            .visit_ip_mut(
                &mut owner,
                |mut leaf, _| leaf.set_read_scopes(&[scope]).unwrap(),
                |_, _| {},
            )
            .unwrap();
        let predicate = predicate(&owner);
        let store = EdgionConfigDataStore::new();
        publish(&store, "target", "192.0.2.0/24", "Namespace");
        assert!(predicate.available(&session(&store)));
        let data = serde_json::from_value(json!({"metadata":{"name":"networks","namespace":"target"},"spec":{"data":{"type":"Selector","config":{"active":"main"}}}})).unwrap();
        store.full_set(&HashMap::from([("target/networks".into(), Arc::new(data))]));
        assert!(
            !predicate.available(&session(&store)),
            "authorization cannot bypass type"
        );
        publish(&store, "target", "192.0.2.0/24", "Namespace");
        let mut data = store.get_raw_by_key("target/networks").unwrap().as_ref().clone();
        data.spec.enable = false;
        store.full_set(&HashMap::from([("target/networks".into(), Arc::new(data))]));
        assert!(
            !predicate.available(&session(&store)),
            "authorization cannot bypass disabled targets"
        );
    }
}

#[test]
fn a_matching_slot_stamp_cannot_forge_same_namespace_read_scope() {
    let mut owner = owner("source", "target");
    let prepared = PreparedConditionReferences::from_plugins(&owner);
    prepared
        .visit_ip_mut(
            &mut owner,
            |mut leaf, _| {
                assert!(leaf.set_read_scopes(&[IpReadScope::SameNamespace]).is_err());
                leaf.set_read_scopes(&[IpReadScope::ClusterOnly]).unwrap();
                leaf.config_mut().resolved_refs.as_mut().unwrap().slots[0].scope = IpReadScope::SameNamespace;
            },
            |_, _| {},
        )
        .unwrap();
    assert!(IpConditionCompiler::new(&owner).is_err());
}

#[test]
fn invalid_actual_owner_is_static_failure_even_for_optional_negated_rap_leaves() {
    let profile = json!({"requiredRuleGroups":[{"name":"clients","anyOfRules":[{"name":"peer","type":"directPeerIp","config":{
        "ipRefs":[{"name":"networks"}],"conditions":{"allOf":[{"type":"remoteIpMatch","ipRefs":[{"name":"networks"}],"negate":true}]}
    }}]}]});
    let owner: EdgionPlugins = serde_json::from_value(json!({"metadata":{"name":"Bad_Owner","namespace":"default"},"spec":{"requestPlugins":[{
        "type":"RequestAccessPolicy","config":{"defaultProfile":"main","profiles":{"main":profile},"resolvedProfiles":{"main":profile}}
    }]}})).unwrap();
    assert!(IpConditionCompiler::new(&owner).is_err());
}
