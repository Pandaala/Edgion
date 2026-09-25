use super::*;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

fn contribution(name: &str, declaration: ProxyProtocolPolicyDeclaration) -> ProxyProtocolListenerContribution {
    ProxyProtocolListenerContribution {
        listener_name: name.to_string(),
        listener_key: ListenerKey::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 80), "HTTP"),
        deployment_scope: String::new(),
        declaration,
    }
}

#[test]
fn exact_replay_has_no_dirty_peers() {
    let manager = ProxyProtocolListenerPolicyManager::new();
    let entries = vec![contribution("http", ProxyProtocolPolicyDeclaration::Required)];
    assert!(manager
        .replace_gateway_contributions("default/a", entries.clone())
        .is_empty());
    assert!(manager.replace_gateway_contributions("default/a", entries).is_empty());
}

#[test]
fn dedicated_gateways_do_not_share_proxy_policy_conflicts() {
    let manager = ProxyProtocolListenerPolicyManager::new();
    let mut first = contribution("http", ProxyProtocolPolicyDeclaration::Required);
    first.deployment_scope = "default/a".to_string();
    let mut second = contribution("http", ProxyProtocolPolicyDeclaration::Disabled);
    second.deployment_scope = "default/b".to_string();
    manager.replace_gateway_contributions("default/a", vec![first]);
    manager.replace_gateway_contributions("default/b", vec![second]);
    assert!(manager.conflicting_listener_names("default/a").is_empty());
    assert!(manager.conflicting_listener_names("default/b").is_empty());
}

#[test]
fn conflict_create_and_delete_return_only_changed_peer() {
    let manager = ProxyProtocolListenerPolicyManager::new();
    manager.replace_gateway_contributions(
        "default/a",
        vec![contribution("http-a", ProxyProtocolPolicyDeclaration::Required)],
    );

    assert_eq!(
        manager.replace_gateway_contributions(
            "default/b",
            vec![contribution("http-b", ProxyProtocolPolicyDeclaration::Disabled)],
        ),
        vec!["default/a".to_string()]
    );
    assert_eq!(
        manager.conflicting_listener_names("default/a"),
        BTreeSet::from(["http-a".to_string()])
    );
    assert_eq!(
        manager.replace_gateway_contributions("default/b", Vec::new()),
        vec!["default/a".to_string()]
    );
    assert!(manager.conflicting_listener_names("default/a").is_empty());
}

#[test]
fn declaration_change_that_preserves_conflict_has_no_dirty_peer() {
    let manager = ProxyProtocolListenerPolicyManager::new();
    manager.replace_gateway_contributions(
        "default/a",
        vec![contribution("http-a", ProxyProtocolPolicyDeclaration::Required)],
    );
    manager.replace_gateway_contributions(
        "default/b",
        vec![contribution("http-b", ProxyProtocolPolicyDeclaration::Disabled)],
    );

    assert!(manager
        .replace_gateway_contributions(
            "default/b",
            vec![contribution(
                "http-b",
                ProxyProtocolPolicyDeclaration::TrustedSources(vec!["192.0.2.0/24".to_string()]),
            )],
        )
        .is_empty());
}

#[test]
fn identical_declarations_do_not_conflict_or_requeue() {
    let manager = ProxyProtocolListenerPolicyManager::new();
    manager.replace_gateway_contributions(
        "default/a",
        vec![contribution("http-a", ProxyProtocolPolicyDeclaration::Required)],
    );
    assert!(manager
        .replace_gateway_contributions(
            "default/b",
            vec![contribution("http-b", ProxyProtocolPolicyDeclaration::Required)],
        )
        .is_empty());
    assert!(manager.conflicting_listener_names("default/a").is_empty());
    assert!(manager.conflicting_listener_names("default/b").is_empty());
}

#[test]
fn mode_and_cidr_declaration_mismatches_conflict() {
    for second in [
        ProxyProtocolPolicyDeclaration::TrustedSources(vec!["192.0.2.0/24".to_string()]),
        ProxyProtocolPolicyDeclaration::TrustedSources(vec!["198.51.100.0/24".to_string()]),
    ] {
        let manager = ProxyProtocolListenerPolicyManager::new();
        manager.replace_gateway_contributions(
            "default/a",
            vec![contribution(
                "http-a",
                ProxyProtocolPolicyDeclaration::TrustedSources(vec!["192.0.2.0/24".to_string()]),
            )],
        );
        let dirty = manager.replace_gateway_contributions("default/b", vec![contribution("http-b", second.clone())]);
        if second == ProxyProtocolPolicyDeclaration::TrustedSources(vec!["192.0.2.0/24".to_string()]) {
            assert!(dirty.is_empty());
        } else {
            assert_eq!(dirty, vec!["default/a".to_string()]);
        }
    }

    let manager = ProxyProtocolListenerPolicyManager::new();
    manager.replace_gateway_contributions(
        "default/a",
        vec![contribution("http-a", ProxyProtocolPolicyDeclaration::Required)],
    );
    assert_eq!(
        manager.replace_gateway_contributions(
            "default/b",
            vec![contribution(
                "http-b",
                ProxyProtocolPolicyDeclaration::TrustedSources(vec!["192.0.2.0/24".to_string()]),
            )],
        ),
        vec!["default/a".to_string()]
    );
}
