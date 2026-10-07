use super::*;
use crate::resources::reference_grant::{ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo};

fn grant(from_group: &str, to_group: &str, name: Option<&str>) -> ReferenceGrant {
    let mut grant = ReferenceGrant::new(
        "permission",
        ReferenceGrantSpec {
            from: vec![ReferenceGrantFrom {
                group: from_group.into(),
                kind: "Service".into(),
                namespace: "source".into(),
            }],
            to: vec![ReferenceGrantTo {
                group: to_group.into(),
                kind: "Secret".into(),
                name: name.map(str::to_owned),
            }],
        },
    );
    grant.metadata.namespace = Some("target".into());
    grant
}

fn query<'a>(source_group: &'a str, target_group: &'a str, target_namespace: &'a str) -> GrantQuery<'a> {
    GrantQuery::new(
        ReferenceSource::checked(source_group, "Service", ResourceScope::Namespaced("source")).unwrap(),
        ObjectIdentity::checked(
            target_group,
            "Secret",
            ResourceScope::Namespaced(target_namespace),
            "material",
        )
        .unwrap(),
    )
}

#[test]
fn grant_groups_match_exactly_in_both_directions() {
    let canonical = grant("", "", None);
    assert!(query("", "", "target").matches(&canonical));
    assert!(!query("core", "", "target").matches(&canonical));
    assert!(!query("", "core", "target").matches(&canonical));
    assert!(!query("", "", "target").matches(&grant("core", "", None)));
    assert!(!query("", "", "target").matches(&grant("", "core", None)));
}

#[test]
fn only_omitted_grant_name_is_a_wildcard_and_target_is_concrete() {
    let query = query("", "", "target");
    assert!(query.matches(&grant("", "", None)));
    assert!(query.matches(&grant("", "", Some("material"))));
    assert!(!query.matches(&grant("", "", Some("other"))));
    assert!(!query.matches(&grant("", "", Some(""))));
    assert!(!allows_reference(
        &grant("", "", None),
        "source",
        "",
        "Service",
        "",
        "Secret",
        None
    ));
}

#[test]
fn source_and_target_must_match_inside_the_same_grant() {
    let mut source_only = grant("", "", None);
    source_only.spec.to[0].kind = "ConfigMap".into();
    let mut target_only = grant("", "", None);
    target_only.spec.from[0].namespace = "different".into();
    let query = query("", "", "target");
    assert!(![source_only, target_only].iter().any(|grant| query.matches(grant)));
    let mut alternatives = grant("", "", Some("other"));
    alternatives.spec.to.push(ReferenceGrantTo {
        group: "".into(),
        kind: "Secret".into(),
        name: None,
    });
    assert!(query.matches(&alternatives));
    assert!(!self::query("", "", "another-target").matches(&alternatives));
    alternatives.metadata.namespace = None;
    assert!(!query.matches(&alternatives));
}

#[test]
fn cluster_source_cannot_gain_reference_grant_authority() {
    let query = GrantQuery::new(
        ReferenceSource::checked("edgion.io", "EdgionGatewayConfig", ResourceScope::Cluster).unwrap(),
        ObjectIdentity::checked("", "Secret", ResourceScope::Namespaced("target"), "material").unwrap(),
    );
    assert!(!query.matches(&grant("edgion.io", "", None)));
}
