use super::*;

#[test]
fn tls_route_preflight_matches_the_single_canonical_watch_and_status_type() {
    use edgion_resources::TLSRoute;
    use kube::Resource;
    let entries: Vec<_> = canonical_sources().filter(|entry| entry.kind == "TLSRoute").collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].version, TLSRoute::version(&()));
    assert_eq!(GROUP, TLSRoute::group(&()));
    assert_eq!(TLSRoute::api_version(&()), "gateway.networking.k8s.io/v1");
}

#[test]
fn backend_is_a_known_no_watch_kind() {
    assert!(other_known_kind_names().contains("EdgionBackend"));
}

#[tokio::test]
async fn discovery_validates_group_version_and_ignores_subresources() {
    use kube::client::Body;
    use tower::service_fn;
    for (group_version, name, expected) in [
        ("gateway.networking.k8s.io/v1", "gateways", Some(true)),
        ("invalid/group/version", "gateways", None),
        ("gateway.networking.k8s.io/v1", "gateways/status", Some(false)),
    ] {
        let client = Client::new(
            service_fn(move |_| async move {
                Ok::<_, std::convert::Infallible>(http::Response::new(Body::from(
                    serde_json::json!({
                        "groupVersion":group_version,"resources":[{
                            "name":name,"kind":"Gateway","namespaced":true,"singularName":"gateway","verbs":["list"]
                        }]
                    })
                    .to_string()
                    .into_bytes(),
                )))
            }),
            "default",
        );
        let result = is_kind_served(&client, &GroupVersionKind::gvk(GROUP, "v1", "Gateway")).await;
        match expected {
            Some(expected) => assert_eq!(result.unwrap(), expected),
            None => assert!(matches!(result, Err(ConfWriterError::InternalError(_)))),
        }
    }
}
