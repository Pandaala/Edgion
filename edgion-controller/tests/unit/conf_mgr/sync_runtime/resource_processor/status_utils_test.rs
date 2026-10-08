use super::*;

#[test]
fn test_partially_invalid_condition_true_and_false() {
    let t = partially_invalid_condition(Some("group 'a' cidr 'x': invalid CIDR".into()), Some(1));
    assert_eq!(t.type_, "PartiallyInvalid");
    assert_eq!(t.status, "True");
    assert_eq!(t.reason, "RuleInvalid");
    assert!(t.message.contains("invalid CIDR"));

    let f = partially_invalid_condition(None, Some(1));
    assert_eq!(f.status, "False");
    assert_eq!(f.reason, "AllRulesValid");
}

#[test]
fn test_update_condition_insert() {
    let mut conditions = Vec::new();
    let new_cond = condition_true("TestType", "TestReason", "Test message", Some(1));

    update_condition(&mut conditions, new_cond);

    assert_eq!(conditions.len(), 1);
    assert_eq!(conditions[0].type_, "TestType");
    assert_eq!(conditions[0].status, "True");
}

#[test]
fn test_update_condition_status_change() {
    let mut conditions = vec![condition_true("TestType", "OldReason", "Old message", Some(1))];
    conditions[0].last_transition_time = "1970-01-01T00:00:00Z".to_string();

    let old_time = conditions[0].last_transition_time.clone();

    // Update with same status - time should NOT change
    let same_status = condition_true("TestType", "NewReason", "New message", Some(2));
    update_condition(&mut conditions, same_status);
    assert_eq!(conditions[0].last_transition_time, old_time);
    assert_eq!(conditions[0].reason, "NewReason");

    // Update with different status - time SHOULD change
    let diff_status = condition_false("TestType", "FailReason", "Failed", Some(3));
    update_condition(&mut conditions, diff_status);
    assert_ne!(conditions[0].last_transition_time, old_time);
    assert_eq!(conditions[0].status, "False");
}

#[test]
fn test_resolved_refs_condition_no_errors() {
    let cond = resolved_refs_condition(&[], Some(1));
    assert_eq!(cond.status, "True");
    assert_eq!(cond.reason, "ResolvedRefs");
}

#[test]
fn region_route_invalid_overlay_uses_existing_reference_reason_and_priority() {
    let errors = vec![
        ResolvedRefsError::SecretNotFound {
            namespace: "default".into(),
            name: "secret".into(),
        },
        ResolvedRefsError::EdgionConfigDataInvalid {
            namespace: "default".into(),
            name: "overlay".into(),
            detail: "region hash range exceeds consumer modulo".into(),
        },
    ];
    let condition = resolved_refs_condition(&errors, Some(1));
    assert_eq!(condition.status, "False");
    assert_eq!(condition.reason, "InvalidEdgionConfigDataRef");
    assert!(condition.message.contains("default/overlay"));
    assert!(condition.message.contains("exceeds consumer modulo"));
}

#[test]
fn test_accepted_error_not_allowed_by_listeners() {
    let err = AcceptedError::NotAllowedByListeners {
        resource_ns: "test-ns".to_string(),
    };
    assert_eq!(err.reason(), "NotAllowedByListeners");
    assert_eq!(err.message(), "Namespace 'test-ns' not allowed by Gateway listeners");
}

#[test]
fn test_accepted_error_not_allowed_by_listener_kind() {
    let err = AcceptedError::NotAllowedByListenerKind {
        resource_kind: "GRPCRoute".to_string(),
    };
    assert_eq!(err.reason(), "NotAllowedByListeners");
    assert_eq!(err.message(), "Route kind 'GRPCRoute' not allowed by Gateway listeners");
}

#[test]
fn test_accepted_error_no_matching_listener_hostname() {
    let err = AcceptedError::NoMatchingListenerHostname {
        hostnames: vec!["a.example.com".to_string(), "b.example.com".to_string()],
    };
    assert_eq!(err.reason(), "NoMatchingListenerHostname");
    assert!(err.message().contains("a.example.com"));
    assert!(err.message().contains("b.example.com"));
}

#[test]
fn test_accepted_error_no_matching_parent() {
    let err = AcceptedError::NoMatchingParent {
        section_name: "https".to_string(),
    };
    assert_eq!(err.reason(), "NoMatchingParent");
    assert_eq!(err.message(), "No matching listener for sectionName 'https'");
}

#[test]
fn test_accepted_error_parent_not_found() {
    let err = AcceptedError::ParentNotFound {
        parent_name: "default/my-gateway".to_string(),
    };
    assert_eq!(err.reason(), "NoMatchingParent");
    assert_eq!(err.message(), "Parent Gateway 'default/my-gateway' not found");
}

#[test]
fn test_resolved_refs_error_invalid_kind() {
    let err = ResolvedRefsError::InvalidKind {
        kind: "Deployment".to_string(),
        name: "my-deploy".to_string(),
    };
    assert_eq!(err.reason(), "InvalidKind");
    assert_eq!(
        err.message(),
        "Invalid backend ref kind 'Deployment' for backend 'my-deploy'"
    );
}

#[test]
fn test_resolved_refs_error_backend_not_found() {
    let err = ResolvedRefsError::BackendNotFound {
        namespace: "default".to_string(),
        name: "my-svc".to_string(),
    };
    assert_eq!(err.reason(), "BackendNotFound");
    assert_eq!(err.message(), "Service 'default/my-svc' not found");
}

#[test]
fn test_resolved_refs_error_ref_not_permitted() {
    let err = ResolvedRefsError::RefNotPermitted {
        from_namespace: "app-ns".to_string(),
        from_kind: "HTTPRoute",
        target_namespace: "other-ns".to_string(),
        target_name: "other-svc".to_string(),
    };
    assert_eq!(err.reason(), "RefNotPermitted");
    assert_eq!(
        err.message(),
        "Reference from app-ns/HTTPRoute to \"other-ns/other-svc\" is not permitted without a ReferenceGrant"
    );
}

#[test]
fn test_resolved_refs_condition_single_error() {
    let errors = vec![ResolvedRefsError::BackendNotFound {
        namespace: "default".to_string(),
        name: "svc-a".to_string(),
    }];
    let cond = resolved_refs_condition(&errors, Some(1));
    assert_eq!(cond.status, "False");
    assert_eq!(cond.reason, "BackendNotFound");
    assert!(cond.message.contains("svc-a"));
}

#[test]
fn test_resolved_refs_reason_priority_invalid_kind_highest() {
    let errors = vec![
        ResolvedRefsError::RefNotPermitted {
            from_namespace: "app-ns".to_string(),
            from_kind: "HTTPRoute",
            target_namespace: "ns".to_string(),
            target_name: "svc".to_string(),
        },
        ResolvedRefsError::InvalidKind {
            kind: "Deployment".to_string(),
            name: "deploy".to_string(),
        },
        ResolvedRefsError::BackendNotFound {
            namespace: "default".to_string(),
            name: "svc2".to_string(),
        },
    ];
    let cond = resolved_refs_condition(&errors, Some(1));
    assert_eq!(cond.reason, "InvalidKind");
}

#[test]
fn test_resolved_refs_reason_priority_ref_not_permitted_over_backend() {
    let errors = vec![
        ResolvedRefsError::BackendNotFound {
            namespace: "default".to_string(),
            name: "svc".to_string(),
        },
        ResolvedRefsError::RefNotPermitted {
            from_namespace: "app-ns".to_string(),
            from_kind: "HTTPRoute",
            target_namespace: "other".to_string(),
            target_name: "svc2".to_string(),
        },
    ];
    let cond = resolved_refs_condition(&errors, Some(1));
    assert_eq!(cond.reason, "RefNotPermitted");
}

#[test]
fn test_resolved_refs_condition_message_joins_multiple() {
    let errors = vec![
        ResolvedRefsError::BackendNotFound {
            namespace: "a".to_string(),
            name: "svc1".to_string(),
        },
        ResolvedRefsError::BackendNotFound {
            namespace: "b".to_string(),
            name: "svc2".to_string(),
        },
    ];
    let cond = resolved_refs_condition(&errors, Some(1));
    assert!(cond.message.contains("a/svc1"));
    assert!(cond.message.contains("b/svc2"));
    assert!(cond.message.contains("; "));
}

#[test]
fn test_set_parent_conditions_no_errors() {
    let mut conditions = Vec::new();
    set_parent_conditions(&mut conditions, &[], Some(1));

    assert_eq!(conditions.len(), 2); // Accepted, ResolvedRefs
    assert!(conditions.iter().all(|c| c.status == "True"));
    assert!(conditions.iter().any(|c| c.type_ == "Accepted"));
    assert!(conditions.iter().any(|c| c.type_ == "ResolvedRefs"));
    assert!(!conditions.iter().any(|c| c.type_ == "Programmed"));
    assert!(!conditions.iter().any(|c| c.type_ == "Ready"));
}

#[test]
fn test_set_parent_conditions_with_resolved_refs_errors() {
    let mut conditions = Vec::new();
    let errors = vec![ResolvedRefsError::RefNotPermitted {
        from_namespace: "app-ns".to_string(),
        from_kind: "HTTPRoute",
        target_namespace: "ns".to_string(),
        target_name: "svc".to_string(),
    }];
    set_parent_conditions(&mut conditions, &errors, Some(1));

    let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
    assert_eq!(accepted.status, "True");

    let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
    assert_eq!(resolved.status, "False");
    assert_eq!(resolved.reason, "RefNotPermitted");
}

#[test]
fn test_set_parent_conditions_full_with_accepted_errors() {
    let mut conditions = Vec::new();
    let accepted_errors = vec![AcceptedError::NotAllowedByListeners {
        resource_ns: "blocked-ns".to_string(),
    }];
    set_parent_conditions_full(&mut conditions, &accepted_errors, &[], Some(1));

    assert_eq!(conditions.len(), 2); // Accepted, ResolvedRefs only

    let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, "NotAllowedByListeners");
    assert!(accepted.message.contains("blocked-ns"));

    assert!(!conditions.iter().any(|c| c.type_ == "Programmed"));
    assert!(!conditions.iter().any(|c| c.type_ == "Ready"));

    let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
    assert_eq!(resolved.status, "True");
}

#[test]
fn test_set_parent_conditions_full_with_both_errors() {
    let mut conditions = Vec::new();
    let accepted_errors = vec![AcceptedError::NoMatchingParent {
        section_name: "nonexistent".to_string(),
    }];
    let resolved_errors = vec![ResolvedRefsError::BackendNotFound {
        namespace: "default".to_string(),
        name: "missing-svc".to_string(),
    }];
    set_parent_conditions_full(&mut conditions, &accepted_errors, &resolved_errors, Some(1));

    let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, "NoMatchingParent");

    let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
    assert_eq!(resolved.status, "False");
    assert_eq!(resolved.reason, "BackendNotFound");
}

#[test]
fn test_accepted_error_invalid() {
    let err = AcceptedError::Invalid {
        message: "Secret 'ns/secret' not found".to_string(),
    };
    assert_eq!(err.reason(), "Invalid");
    assert_eq!(err.message(), "Secret 'ns/secret' not found");
}

#[test]
fn test_accepted_error_from_validation_errors_empty() {
    let errors = AcceptedError::from_validation_errors(&[]);
    assert!(errors.is_empty());
}

#[test]
fn test_accepted_error_from_validation_errors_multiple() {
    let validation = vec![
        "Secret 'ns/secret' not found".to_string(),
        "No parent_refs configured".to_string(),
    ];
    let errors = AcceptedError::from_validation_errors(&validation);
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].reason(), "Invalid");
    assert_eq!(errors[0].message(), "Secret 'ns/secret' not found");
    assert_eq!(errors[1].message(), "No parent_refs configured");
}

#[test]
fn test_set_parent_conditions_full_with_validation_errors() {
    let mut conditions = Vec::new();
    let accepted_errors =
        AcceptedError::from_validation_errors(&["Secret 'test/missing' not found (may arrive later)".to_string()]);
    set_parent_conditions_full(&mut conditions, &accepted_errors, &[], Some(1));

    assert_eq!(conditions.len(), 2); // Accepted, ResolvedRefs only

    let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, "Invalid");
    assert!(accepted.message.contains("Secret 'test/missing' not found"));

    assert!(!conditions.iter().any(|c| c.type_ == "Programmed"));
    assert!(!conditions.iter().any(|c| c.type_ == "Ready"));

    let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
    assert_eq!(resolved.status, "True");
}

#[test]
fn test_resolved_refs_error_secret_not_found_reason_is_secret_not_found() {
    // SecretNotFound must map to SECRET_NOT_FOUND, not BACKEND_NOT_FOUND —
    // using BackendNotFound would mislead operators of Secret-backed CRDs
    // (LinkSys / EdgionPlugins / EdgionTls) into looking for a missing Service.
    let err = ResolvedRefsError::SecretNotFound {
        namespace: "ns".to_string(),
        name: "missing".to_string(),
    };
    assert_eq!(err.reason(), "SecretNotFound");
    assert!(err.message().contains("ns/missing"));
}

#[test]
fn test_resolved_refs_error_configmap_not_found() {
    let err = ResolvedRefsError::ConfigMapNotFound {
        namespace: "default".to_string(),
        name: "ca-bundle".to_string(),
    };
    assert_eq!(err.reason(), "ConfigMapNotFound");
    assert_eq!(err.message(), "ConfigMap 'default/ca-bundle' not found");
}

#[test]
fn test_resolved_refs_error_missing_keys() {
    let err = ResolvedRefsError::MissingKeys {
        keys: vec!["password", "username"],
    };
    assert_eq!(err.reason(), "MissingKeys");
    assert!(err.message().contains("password"));
    assert!(err.message().contains("username"));
}

#[test]
fn test_resolved_refs_error_decode_error() {
    let err = ResolvedRefsError::DecodeError {
        key: "password",
        detail: "invalid utf-8".to_string(),
    };
    assert_eq!(err.reason(), "DecodeError");
    assert!(err.message().contains("password"));
    assert!(err.message().contains("invalid utf-8"));
}

#[test]
fn test_resolved_refs_error_invalid_secret_type() {
    const ALLOWED: &[&str] = &["kubernetes.io/basic-auth", "Opaque", ""];
    let err = ResolvedRefsError::InvalidSecretType {
        got: "kubernetes.io/ssh-auth".to_string(),
        allowed: ALLOWED,
    };
    assert_eq!(err.reason(), "InvalidSecretType");
    assert!(err.message().contains("ssh-auth"));
}

#[test]
fn test_resolved_refs_reason_priority_invalid_secret_type_before_backend() {
    // Two errors: one Secret-content issue, one missing-resource issue.
    // `InvalidSecretType` has higher priority than `SecretNotFound` — the
    // Secret both exists and has the wrong type, so reporting "not found"
    // would mislead the operator.
    let errors = vec![
        ResolvedRefsError::SecretNotFound {
            namespace: "ns".to_string(),
            name: "other".to_string(),
        },
        ResolvedRefsError::InvalidSecretType {
            got: "kubernetes.io/tls".to_string(),
            allowed: &["kubernetes.io/basic-auth"],
        },
    ];
    let cond = resolved_refs_condition(&errors, Some(1));
    assert_eq!(cond.reason, "InvalidSecretType");
}

#[test]
fn cert_content_invalid_maps_to_invalid_certificate_reason() {
    let e = ResolvedRefsError::CertContentInvalid {
        detail: "Certificate expired or not yet valid (valid: a - b)".to_string(),
    };
    assert_eq!(e.reason(), condition_reasons::INVALID_CERTIFICATE);
    assert!(e.message().contains("expired"));
    let _ = pick_resolved_refs_reason(std::slice::from_ref(&e));
}

#[test]
fn test_set_parent_conditions_full_validation_and_resolved_refs_errors() {
    let mut conditions = Vec::new();
    let accepted_errors = AcceptedError::from_validation_errors(&["Secret 'ns/bad' not found".to_string()]);
    let resolved_errors = vec![ResolvedRefsError::BackendNotFound {
        namespace: "ns".to_string(),
        name: "bad".to_string(),
    }];
    set_parent_conditions_full(&mut conditions, &accepted_errors, &resolved_errors, Some(2));

    let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, "Invalid");
    assert_eq!(accepted.observed_generation, Some(2));

    let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
    assert_eq!(resolved.status, "False");
    assert_eq!(resolved.reason, "BackendNotFound");
}

#[test]
fn ambiguous_plugin_reference_has_bounded_resolved_refs_diagnostic() {
    let error = ResolvedRefsError::AmbiguousEdgionPlugins {
        namespace: "default".into(),
        name: "plugins".into(),
    };
    assert_eq!(error.reason(), "InvalidPluginsRef");
    assert_eq!(error.message(), "EdgionPlugins reference default/plugins is ambiguous");
}

#[test]
fn ambiguity_reason_priority_is_independent_of_input_order() {
    let ambiguity = ResolvedRefsError::AmbiguousEdgionPlugins {
        namespace: "default".into(),
        name: "plugins".into(),
    };
    for existing in [
        ResolvedRefsError::InvalidKind {
            kind: "Unsupported".into(),
            name: "target".into(),
        },
        ResolvedRefsError::InvalidReference {
            error: edgion_resources::resources::refs::ReferenceError::MissingNamespace,
        },
        ResolvedRefsError::RefNotPermitted {
            from_namespace: "apps".into(),
            from_kind: "HTTPRoute",
            target_namespace: "default".into(),
            target_name: "plugins".into(),
        },
    ] {
        assert_eq!(
            pick_resolved_refs_reason(&[ambiguity.clone(), existing.clone()]),
            existing.reason()
        );
        assert_eq!(
            pick_resolved_refs_reason(&[existing.clone(), ambiguity.clone()]),
            existing.reason()
        );
    }
}
