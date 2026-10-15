use super::*;
use crate::conf_mgr::sync_runtime::ShutdownHandle;
use http::{Method, Response, StatusCode};
use kube::client::Body;
use serde_json::{json, Value};
use std::convert::Infallible;
use tokio::sync::oneshot;
use tower::service_fn;

#[derive(Clone, Copy, Default)]
enum Reply {
    #[default]
    Success,
    Hold,
    Error(u16),
}

#[derive(Default)]
struct ApiState {
    lease: Value,
    get: Reply,
    put: Reply,
    patch: Reply,
    gets: usize,
    puts: Vec<Value>,
    labels: Vec<bool>,
    held: Vec<oneshot::Sender<()>>,
}

fn response(value: Value, code: u16) -> Response<Body> {
    Response::builder()
        .status(code)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(value.to_string().into_bytes()))
        .unwrap()
}

struct Harness {
    election: LeaderElection,
    api: Arc<RwLock<ApiState>>,
    shutdown: ShutdownHandle,
    task: tokio::task::JoinHandle<Result<()>>,
}

impl Harness {
    fn new(deadline: u64) -> Self {
        let api = Arc::new(RwLock::new(ApiState {
            lease: json!({"apiVersion":"coordination.k8s.io/v1", "kind":"Lease",
                "metadata":{"name":"controller", "namespace":"default", "uid":"uid", "resourceVersion":"1"},
                "spec":{"holderIdentity":"controller-a", "leaseTransitions":3}}),
            ..Default::default()
        }));
        let state = api.clone();
        let client = Client::new(
            service_fn(move |request: http::Request<Body>| {
                let state = state.clone();
                async move {
                    let method = request.method().clone();
                    let bytes = request.into_body().collect_bytes().await.unwrap();
                    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    let (reply, value, held) = {
                        let mut state = state.write();
                        let (reply, value) = match method {
                            Method::GET => {
                                state.gets += 1;
                                (state.get, state.lease.clone())
                            }
                            Method::PUT => {
                                state.puts.push(body.clone());
                                if !matches!(state.put, Reply::Error(_)) {
                                    // A submitted write can land even if the client
                                    // subsequently cancels waiting for its response.
                                    state.lease = body.clone();
                                    let version = state.puts.len() + 1;
                                    state.lease["metadata"]["resourceVersion"] = json!(version.to_string());
                                }
                                (state.put, state.lease.clone())
                            }
                            Method::PATCH => {
                                state
                                    .labels
                                    .push(body["metadata"]["labels"]["edgion.io/leader"] == "true");
                                (
                                    state.patch,
                                    json!({"apiVersion":"v1", "kind":"Pod", "metadata":{"name":"controller-a"}}),
                                )
                            }
                            Method::POST => (Reply::Success, state.lease.clone()),
                            _ => panic!("unexpected method"),
                        };
                        let held = if matches!(reply, Reply::Hold) {
                            let (tx, rx) = oneshot::channel();
                            state.held.push(tx);
                            Some(rx)
                        } else {
                            None
                        };
                        (reply, value, held)
                    };
                    if let Some(held) = held {
                        let _ = held.await;
                    }
                    let response = match reply {
                        Reply::Error(code) => response(
                            json!({"apiVersion":"v1", "kind":"Status", "status":"Failure",
                        "reason":"InjectedFailure", "message":"injected API failure", "code":code}),
                            code,
                        ),
                        _ => response(value, 200),
                    };
                    Ok::<_, Infallible>(response)
                }
            }),
            "default",
        );
        let (flag, _) = watch::channel(false);
        let election = LeaderElection::new(
            client,
            LeaderElectionConfig {
                lease_name: "controller".into(),
                lease_namespace: "default".into(),
                pod_namespace: "default".into(),
                identity: "controller-a".into(),
                lease_duration_secs: 15,
                renew_deadline_secs: deadline,
                retry_period_secs: 2,
            },
            Arc::new(flag),
        );
        let shutdown = ShutdownHandle::new();
        let runner = election.clone();
        let signal = shutdown.signal();
        let task = tokio::spawn(async move { runner.run(Some(signal)).await });
        Self {
            election,
            api,
            shutdown,
            task,
        }
    }

    async fn tick(&self, seconds: u64) {
        settle().await;
        tokio::time::advance(Duration::from_secs(seconds)).await;
        settle().await;
    }

    fn withdrawn(&self) {
        assert!(!self.election.is_leader());
        assert!(self.election.current_fence.read().is_none());
    }

    async fn stop(self) {
        self.shutdown.shutdown();
        settle().await;
        tokio::time::advance(Duration::from_secs(3)).await;
        settle().await;
        tokio::time::advance(Duration::from_secs(2)).await;
        settle().await;
        assert!(self.task.is_finished());
        self.withdrawn();
        self.task.await.unwrap().unwrap();
    }
}

async fn settle() {
    // Yield to the in-process kube buffer and election task without allowing
    // paused time to auto-advance to the next timer.
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn stalled_lease_get_and_replace_expire_without_late_revival() {
    for hold_put in [false, true] {
        let h = Harness::new(10);
        h.tick(2).await;
        let old = h.election.handle().current_fence().unwrap();
        if hold_put {
            h.api.write().put = Reply::Hold;
        } else {
            h.api.write().get = Reply::Hold;
        }
        h.tick(2).await;
        assert_eq!(h.api.read().held.len(), 1);
        h.tick(8).await;
        h.withdrawn();
        let held = std::mem::take(&mut h.api.write().held);
        for response in held {
            let _ = response.send(());
        }
        settle().await;
        h.withdrawn();
        h.api.write().get = Reply::Success;
        h.api.write().put = Reply::Success;
        let reads = h.api.read().gets;
        h.tick(2).await;
        let fresh = h.election.handle().current_fence().unwrap();
        assert_ne!(fresh.term, old.term);
        assert!(h.api.read().gets > reads);
        {
            let state = h.api.read();
            assert_eq!(
                state.puts.last().unwrap()["metadata"]["resourceVersion"],
                json!(state.puts.len().to_string())
            );
        }
        h.stop().await;
    }
}

#[tokio::test(start_paused = true)]
async fn stalled_acquisition_and_retry_labels_do_not_consume_renewal_cadence() {
    let h = Harness::new(3);
    h.api.write().patch = Reply::Hold;
    h.tick(2).await;
    let fence = h.election.handle().current_fence().unwrap();
    for _ in 0..5 {
        h.tick(2).await;
        assert_eq!(h.election.handle().current_fence().unwrap(), fence);
    }
    assert_eq!(h.api.read().puts.len(), 6);
    assert_eq!(h.api.read().labels.len(), 6);
    h.api.write().get = Reply::Error(503);
    h.tick(2).await;
    assert!(h.election.is_leader());
    h.tick(1).await;
    h.withdrawn();
    assert_eq!(h.api.read().labels.last(), Some(&false));
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn transient_errors_keep_grace_but_sleep_cannot_outlive_deadline() {
    let h = Harness::new(3);
    h.tick(2).await;
    h.api.write().get = Reply::Error(503);
    h.tick(2).await;
    assert!(h.election.is_leader());
    h.tick(1).await;
    h.withdrawn();
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn timely_success_refreshes_deadline_and_transient_errors_preserve_term() {
    let h = Harness::new(10);
    h.tick(2).await;
    let fence = h.election.handle().current_fence().unwrap();
    h.api.write().get = Reply::Error(503);
    h.tick(2).await;
    assert_eq!(h.election.handle().current_fence().unwrap(), fence);
    h.api.write().get = Reply::Hold;
    h.tick(2).await;
    h.tick(5).await;
    h.api.write().get = Reply::Success;
    for response in std::mem::take(&mut h.api.write().held) {
        let _ = response.send(());
    }
    settle().await;
    h.tick(1).await;
    assert_eq!(h.election.handle().current_fence().unwrap(), fence);
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn simultaneous_success_and_expiry_cannot_republish_fence() {
    let h = Harness::new(10);
    h.tick(2).await;
    h.api.write().put = Reply::Hold;
    h.tick(2).await;
    for response in std::mem::take(&mut h.api.write().held) {
        let _ = response.send(());
    }
    // Both the queued response and deadline are ready before the owner runs.
    tokio::time::advance(Duration::from_secs(8)).await;
    settle().await;
    h.withdrawn();
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn authoritative_loss_clears_fence_before_stalled_false_label() {
    for scenario in ["get404", "put404", "put409", "holder"] {
        let h = Harness::new(10);
        h.tick(2).await;
        {
            let mut state = h.api.write();
            state.patch = Reply::Hold;
            match scenario {
                "get404" => state.get = Reply::Error(404),
                "put404" => state.put = Reply::Error(404),
                "put409" => state.put = Reply::Error(409),
                _ => state.lease["spec"]["holderIdentity"] = json!("controller-b"),
            }
        }
        h.tick(2).await;
        h.withdrawn();
        assert_eq!(h.api.read().labels.last(), Some(&false));
        h.stop().await;
    }
}

#[tokio::test(start_paused = true)]
async fn shutdown_interrupts_pending_lease_and_label_and_bounds_release() {
    for label in [false, true] {
        let h = Harness::new(10);
        if label {
            h.api.write().patch = Reply::Hold;
        }
        h.tick(2).await;
        h.api.write().get = Reply::Hold;
        if !label {
            h.tick(2).await;
        }
        h.shutdown.shutdown();
        settle().await;
        h.withdrawn();
        assert!(!h.task.is_finished());
        h.stop().await;
    }
}

#[tokio::test(start_paused = true)]
async fn preflight_resets_label_and_rejects_patch_rbac_failure() {
    let h = Harness::new(10);
    h.election.preflight_check().await.unwrap();
    assert_eq!(h.api.read().labels.as_slice(), &[false]);
    h.api.write().patch = Reply::Error(StatusCode::FORBIDDEN.as_u16());
    let error = h.election.preflight_check().await.unwrap_err();
    assert!(error.to_string().contains("Failed startup pod label permission check"));
    h.withdrawn();
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn failed_label_retries_current_state_without_flapping_authority() {
    let h = Harness::new(3);
    h.api.write().patch = Reply::Error(503);
    h.tick(2).await;
    let fence = h.election.handle().current_fence().unwrap();
    h.tick(2).await;
    assert_eq!(h.election.handle().current_fence().unwrap(), fence);
    h.api.write().patch = Reply::Success;
    h.tick(2).await;
    let patches = h.api.read().labels.len();
    h.tick(2).await;
    assert_eq!(h.api.read().labels.len(), patches);
    h.shutdown.shutdown();
    settle().await;
    h.withdrawn();
    assert!(h.task.is_finished());
    {
        let state = h.api.read();
        let release = state.puts.last().unwrap();
        assert!(release["spec"]["holderIdentity"].is_null());
        assert!(release["spec"]["renewTime"].is_null());
        assert_eq!(
            release["metadata"]["resourceVersion"],
            json!(state.puts.len().to_string())
        );
        assert_eq!(state.labels.last(), Some(&false));
    }
    h.stop().await;
}
