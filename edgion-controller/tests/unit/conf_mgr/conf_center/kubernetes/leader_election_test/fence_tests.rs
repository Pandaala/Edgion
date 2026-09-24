use super::*;
use http::{Response, StatusCode};
use kube::client::Body;
use serde_json::json;
use std::convert::Infallible;
use tower::service_fn;

fn fence() -> LeaderFence {
    LeaderFence {
        lease_uid: "lease-uid".into(),
        holder_identity: "controller-a".into(),
        term: "term-a".into(),
        lease_transitions: 3,
    }
}

fn lease_response(term: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "apiVersion": "coordination.k8s.io/v1",
                "kind": "Lease",
                "metadata": {
                    "name": "edgion-controller",
                    "namespace": "default",
                    "uid": "lease-uid",
                    "annotations": { LEADER_TERM_ANNOTATION: term },
                },
                "spec": {
                    "holderIdentity": "controller-a",
                    "leaseTransitions": 3,
                },
            })
            .to_string()
            .into_bytes(),
        ))
        .unwrap()
}

fn error_response(code: StatusCode) -> Response<Body> {
    Response::builder()
        .status(code)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "apiVersion": "v1",
                "kind": "Status",
                "status": "Failure",
                "reason": if code == StatusCode::NOT_FOUND { "NotFound" } else { "Unavailable" },
                "code": code.as_u16(),
            })
            .to_string()
            .into_bytes(),
        ))
        .unwrap()
}

#[tokio::test]
async fn fence_validation_classifies_current_lost_and_uncertain() {
    for scenario in ["current", "changed-term", "not-found", "unavailable", "step-down"] {
        let (leader_tx, _leader_rx) = watch::channel(true);
        let leader_tx = Arc::new(leader_tx);
        let response_leader = leader_tx.clone();
        let client = Client::new(
            service_fn(move |_request: http::Request<Body>| {
                let response_leader = response_leader.clone();
                async move {
                    let response = match scenario {
                        "current" => lease_response("term-a"),
                        "changed-term" => lease_response("term-b"),
                        "not-found" => error_response(StatusCode::NOT_FOUND),
                        "unavailable" => error_response(StatusCode::SERVICE_UNAVAILABLE),
                        "step-down" => {
                            response_leader.send_replace(false);
                            lease_response("term-a")
                        }
                        _ => unreachable!(),
                    };
                    Ok::<_, Infallible>(response)
                }
            }),
            "default",
        );
        let expected = fence();
        let handle = LeaderHandle {
            is_leader: leader_tx,
            client,
            lease_name: "edgion-controller".into(),
            lease_namespace: "default".into(),
            current_fence: Arc::new(RwLock::new(Some(expected.clone()))),
        };
        let result = handle.validate_fence_typed(&expected).await;
        match scenario {
            "current" => assert!(matches!(result, LeaderFenceValidation::Current)),
            "unavailable" => assert!(matches!(result, LeaderFenceValidation::Uncertain(_))),
            _ => assert!(matches!(result, LeaderFenceValidation::Lost)),
        }
    }
}
