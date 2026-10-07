use super::*;

fn owner(tls: Option<serde_json::Value>) -> Arc<LinkSys> {
    Arc::new(
        serde_json::from_value(serde_json::json!({
            "metadata":{"name":"es","namespace":"apps"},
            "spec":{"type":"elasticsearch","config":{"endpoints":["https://es.example:9200"],"tls":tls}}
        }))
        .unwrap(),
    )
}
fn empty() -> Arc<GlobalOutboundTlsSnapshot> {
    Arc::new(GlobalOutboundTlsSnapshot::from_configs([]))
}

#[test]
fn actual_source_is_proved_even_without_active_tls() {
    for tls in [None, Some(serde_json::json!({"enabled":false}))] {
        let valid = owner(tls);
        assert!(PreparedLinkSysTls::with_snapshot(Arc::clone(&valid), empty()).is_ok());
        for field in ["name", "empty-namespace", "namespace"] {
            let mut bad = (*valid).clone();
            match field {
                "name" => bad.metadata.name = None,
                "empty-namespace" => bad.metadata.namespace = Some("".into()),
                _ => bad.metadata.namespace = Some("Bad".into()),
            }
            assert!(PreparedLinkSysTls::with_snapshot(Arc::new(bad), empty()).is_err());
        }
    }
}

#[test]
fn callback_borrows_actual_owner_and_the_captured_default_generation() {
    let actual = owner(Some(serde_json::json!({"enabled":true,"verify":false})));
    let defaults = empty();
    let prepared = PreparedLinkSysTls::with_snapshot(Arc::clone(&actual), Arc::clone(&defaults)).unwrap();
    assert!(std::ptr::eq(prepared.owner(), actual.as_ref()));
    prepared
        .with_tls_selection(|proof, global| {
            assert!(std::ptr::eq(global, defaults.as_ref()));
            assert_eq!(proof.active_field().unwrap().policy().verify, Some(false));
            Ok(())
        })
        .unwrap();
}

#[test]
fn whole_ca_and_client_selection_precedes_unused_verify_false_and_disable_gates() {
    let valid = serde_json::json!({"enabled":true,"verify":false,
        "validation":{"caCertificateRefs":[{"kind":"Secret","name":"a"},{"kind":"Secret","name":"b"}]},
        "resolvedCaCertificates":[{"metadata":{"name":"a","namespace":"apps"}},{"metadata":{"name":"b","namespace":"apps"}}],
        "clientCertificateRef":{"name":"client"},
        "resolvedClientCertificate":{"metadata":{"name":"client","namespace":"apps"}}
    });
    assert!(PreparedLinkSysTls::with_snapshot(owner(Some(valid.clone())), empty()).is_ok());
    for field in [
        "group",
        "order",
        "extra-ca",
        "missing-client",
        "wrong-client",
        "extra-client",
        "marker",
        "bad-target",
    ] {
        let mut invalid = valid.clone();
        match field {
            "group" => invalid["validation"]["caCertificateRefs"][0]["group"] = "foreign.example".into(),
            "order" => invalid["resolvedCaCertificates"].as_array_mut().unwrap().swap(0, 1),
            "extra-ca" => invalid["resolvedCaCertificates"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({})),
            "missing-client" => invalid["resolvedClientCertificate"] = serde_json::Value::Null,
            "wrong-client" => invalid["resolvedClientCertificate"]["metadata"]["namespace"] = "other".into(),
            "extra-client" => invalid["clientCertificateRef"] = serde_json::Value::Null,
            "marker" => {
                invalid["resolvedCaCertificates"][0]["metadata"]["annotations"] =
                    serde_json::json!({"edgion.io/controller-ca-source":"ConfigMap"})
            }
            _ => invalid["validation"]["caCertificateRefs"][0]["namespace"] = "".into(),
        }
        assert!(
            PreparedLinkSysTls::with_snapshot(owner(Some(invalid.clone())), empty()).is_err(),
            "{field}"
        );
        invalid["enabled"] = false.into();
        assert!(
            PreparedLinkSysTls::with_snapshot(owner(Some(invalid)), empty()).is_ok(),
            "inactive {field}"
        );
    }
}
