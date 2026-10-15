use anyhow::{Context, Result};
use edgion_resources::resources::common::ParentReference;
use serde_json::Value;

const CONTROLLER_NAME: &str = "edgion.io/gateway-controller";

/// Return the selected controller's resource conditions in either test topology.
pub fn conditions(resource: &Value) -> Result<&Vec<Value>> {
    conditions_for(resource, std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true"))
}

fn conditions_for(resource: &Value, kubernetes: bool) -> Result<&Vec<Value>> {
    let status = if kubernetes {
        let mut controllers = resource["status"]["controllers"]
            .as_array()
            .context("missing Kubernetes controller observations")?
            .iter()
            .filter(|controller| controller["controllerName"] == CONTROLLER_NAME);
        let controller = controllers.next().context("missing Kubernetes controller status")?;
        anyhow::ensure!(controllers.next().is_none(), "ambiguous Kubernetes controller status");
        &controller["status"]
    } else {
        &resource["status"]
    };
    status["conditions"].as_array().context("missing status conditions")
}

/// Match only the scenario's controller and exact Gateway attachment.
pub fn has_condition(
    resource: &Value,
    kind: &str,
    gateway: &ParentReference,
    type_: &str,
    status: &str,
    reason: Option<&str>,
) -> Result<bool> {
    let conditions = match kind {
        "EdgionBackend" => conditions(resource)?,
        "HTTPRoute" => attachment_conditions(resource, "parents", "parentRef", gateway)?,
        "BackendTLSPolicy" | "EdgionBackendTrafficPolicy" => {
            attachment_conditions(resource, "ancestors", "ancestorRef", gateway)?
        }
        _ => anyhow::bail!("unsupported dynamic status resource kind: {kind}"),
    };
    Ok(conditions.iter().any(|condition| {
        condition["type"].as_str() == Some(type_)
            && condition["status"].as_str() == Some(status)
            && reason.is_none_or(|expected| condition["reason"].as_str() == Some(expected))
    }))
}

fn attachment_conditions<'a>(
    resource: &'a Value,
    list: &str,
    reference: &str,
    gateway: &ParentReference,
) -> Result<&'a Vec<Value>> {
    let namespace = resource["metadata"]["namespace"].as_str();
    let expected = gateway.canonical_identity(namespace);
    let entries = resource["status"][list]
        .as_array()
        .with_context(|| format!("missing status.{list}"))?;
    let mut selected = None;
    for entry in entries
        .iter()
        .filter(|entry| entry["controllerName"] == CONTROLLER_NAME)
    {
        let attachment: ParentReference = serde_json::from_value(entry[reference].clone())
            .with_context(|| format!("invalid status.{list}.{reference}"))?;
        if attachment.canonical_identity(namespace) == expected {
            anyhow::ensure!(selected.is_none(), "ambiguous controller attachment in status.{list}");
            selected = Some(entry);
        }
    }
    selected.with_context(|| format!("missing controller attachment in status.{list}"))?["conditions"]
        .as_array()
        .with_context(|| format!("missing controller attachment conditions in status.{list}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn gateway() -> ParentReference {
        serde_json::from_value(json!({"name": "gw", "namespace": "apps"})).unwrap()
    }

    fn condition() -> Value {
        json!({"type": "ResolvedRefs", "status": "False", "reason": "BackendNotFound"})
    }

    fn entry(reference: &str, parent: Value) -> Value {
        let mut entry = json!({"controllerName": CONTROLLER_NAME, "conditions": [condition()]});
        entry[reference] = parent;
        entry
    }

    fn resource(list: &str, entries: Vec<Value>) -> Value {
        let mut resource = json!({"metadata": {"namespace": "apps"}, "status": {}});
        resource["status"][list] = json!(entries);
        resource
    }

    #[test]
    fn simple_conditions_select_native_or_the_unique_controller_observation() {
        let native = json!({"status": {"conditions": [condition()]}});
        let partitioned = json!({"status": {"controllers": [
            {"controllerName": "foreign/controller", "status": {
                "conditions": [{"type": "ResolvedRefs", "status": "True"}]
            }},
            {"controllerName": CONTROLLER_NAME, "status": native["status"]}
        ]}});
        assert_eq!(conditions_for(&native, false).unwrap(), &vec![condition()]);
        assert_eq!(conditions_for(&partitioned, true).unwrap(), &vec![condition()]);
        assert!(conditions_for(&native, true).is_err());
        assert!(conditions_for(&partitioned, false).is_err());
    }

    #[test]
    fn simple_conditions_reject_missing_duplicate_and_incomplete_observations() {
        let local = json!({"controllerName": CONTROLLER_NAME, "status": {"conditions": [condition()]}});
        for status in [
            json!({}),
            json!({"controllers": []}),
            json!({"conditions": [condition()], "controllers": [
                {"controllerName": "foreign/controller", "status": {"conditions": [condition()]}}
            ]}),
            json!({"controllers": [local.clone(), local]}),
            json!({"controllers": [{"controllerName": CONTROLLER_NAME, "status": {}}]}),
        ] {
            assert!(conditions_for(&json!({"status": status}), true).is_err());
        }
        assert!(conditions_for(&json!({"status": {}}), false).is_err());
    }

    #[test]
    fn backend_predicate_ignores_other_observations_and_nested_condition_shaped_data() {
        let local = json!({"conditions": [{"type": "ResolvedRefs", "status": "True"}]});
        let status = if std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
            json!({"controllers": [
                {"controllerName": "foreign/controller", "status": {"conditions": [condition()]}},
                {"controllerName": CONTROLLER_NAME, "status": local}
            ]})
        } else {
            local
        };
        let resource = json!({"status": status, "spec": {"nested": [condition()]}});
        assert!(!has_condition(
            &resource,
            "EdgionBackend",
            &gateway(),
            "ResolvedRefs",
            "False",
            Some("BackendNotFound")
        )
        .unwrap());
        assert!(has_condition(&resource, "EdgionBackend", &gateway(), "ResolvedRefs", "True", None).unwrap());
        assert!(has_condition(&resource, "Unknown", &gateway(), "ResolvedRefs", "True", None).is_err());
    }

    #[test]
    fn attachment_conditions_use_canonical_defaults_and_the_selected_controller() {
        for (kind, list, reference) in [
            ("HTTPRoute", "parents", "parentRef"),
            ("BackendTLSPolicy", "ancestors", "ancestorRef"),
            ("EdgionBackendTrafficPolicy", "ancestors", "ancestorRef"),
        ] {
            let mut foreign = entry(reference, json!({"name": "gw"}));
            foreign["controllerName"] = json!("foreign/controller");
            foreign["conditions"][0]["status"] = json!("True");
            let selected = entry(reference, json!({"name": "gw"}));
            let resource = resource(list, vec![foreign, selected]);
            let explicit: ParentReference = serde_json::from_value(json!({
                "group": "gateway.networking.k8s.io", "kind": "Gateway", "namespace": "apps", "name": "gw"
            }))
            .unwrap();
            assert!(has_condition(
                &resource,
                kind,
                &explicit,
                "ResolvedRefs",
                "False",
                Some("BackendNotFound")
            )
            .unwrap());
            assert!(!has_condition(&resource, kind, &explicit, "ResolvedRefs", "True", None).unwrap());
            assert!(!has_condition(&resource, kind, &explicit, "Accepted", "False", None).unwrap());
            assert!(!has_condition(
                &resource,
                kind,
                &explicit,
                "ResolvedRefs",
                "False",
                Some("RefNotPermitted")
            )
            .unwrap());
            assert!(has_condition(&resource, kind, &explicit, "ResolvedRefs", "False", None).unwrap());
        }
    }

    #[test]
    fn attachment_conditions_never_substitute_a_different_identity_dimension() {
        for (list, reference) in [("parents", "parentRef"), ("ancestors", "ancestorRef")] {
            for patch in [
                json!({"group": ""}),
                json!({"group": "other.example"}),
                json!({"kind": "Service"}),
                json!({"namespace": "elsewhere"}),
                json!({"name": "another-gw"}),
                json!({"sectionName": "https"}),
                json!({"port": 443}),
            ] {
                let mut parent = json!({"name": "gw"});
                parent
                    .as_object_mut()
                    .unwrap()
                    .extend(patch.as_object().unwrap().clone());
                let resource = resource(list, vec![entry(reference, parent)]);
                assert!(
                    attachment_conditions(&resource, list, reference, &gateway()).is_err(),
                    "{patch}"
                );
            }
        }
    }

    #[test]
    fn attachment_conditions_reject_missing_ambiguous_and_incomplete_entries() {
        for (list, reference) in [("parents", "parentRef"), ("ancestors", "ancestorRef")] {
            let selected = entry(reference, json!({"name": "gw"}));
            let equivalent = entry(
                reference,
                json!({
                    "group": "gateway.networking.k8s.io", "kind": "Gateway", "namespace": "apps", "name": "gw"
                }),
            );
            let mut foreign = selected.clone();
            foreign["controllerName"] = json!("foreign/controller");
            let mut incomplete = selected.clone();
            incomplete.as_object_mut().unwrap().remove("conditions");
            for entries in [vec![], vec![foreign], vec![selected, equivalent], vec![incomplete]] {
                let resource = resource(list, entries);
                assert!(attachment_conditions(&resource, list, reference, &gateway()).is_err());
            }
            assert!(attachment_conditions(&json!({}), list, reference, &gateway()).is_err());
            let malformed = resource(list, vec![entry(reference, json!({"name": 42}))]);
            assert!(attachment_conditions(&malformed, list, reference, &gateway()).is_err());
        }
    }

    #[test]
    fn policy_gateway_namespace_is_independent_of_the_policy_namespace() {
        let mut policy = resource(
            "ancestors",
            vec![entry("ancestorRef", json!({"name": "gw", "namespace": "apps"}))],
        );
        policy["metadata"]["namespace"] = json!("backends");
        assert!(has_condition(
            &policy,
            "EdgionBackendTrafficPolicy",
            &gateway(),
            "ResolvedRefs",
            "False",
            None
        )
        .unwrap());
        policy["status"]["ancestors"][0]["ancestorRef"]["namespace"] = Value::Null;
        assert!(has_condition(
            &policy,
            "EdgionBackendTrafficPolicy",
            &gateway(),
            "ResolvedRefs",
            "False",
            None
        )
        .is_err());
    }
}
