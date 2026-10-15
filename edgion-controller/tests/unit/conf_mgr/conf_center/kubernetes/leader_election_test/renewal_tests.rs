use super::*;
use crate::conf_mgr::sync_runtime::ShutdownHandle;
use http::{Method, Response, StatusCode};
use kube::client::Body;
use serde_json::{json, Value};
use std::cell::Cell;
use std::convert::Infallible;
use std::future::poll_fn;
use std::task::Poll;
use tokio::sync::oneshot;
use tower::service_fn;

#[derive(Clone, Copy, Default)]
enum Reply {
    #[default]
    Success,
    Hold,
    Error(u16),
    DeferCommit,
}

#[derive(Default)]
struct ApiState {
    lease: Value,
    pod: Value,
    pod_get: Reply,
    pod_gets: usize,
    pending_patches: Vec<Value>,
    invalid_ack: Option<&'static str>,
    get: Reply,
    put: Reply,
    patch: Reply,
    gets: usize,
    puts: Vec<Value>,
    put_codes: Vec<u16>,
    labels: Vec<bool>,
    held: Vec<oneshot::Sender<()>>,
}

// This server-side operation outlives any canceled client response future.
// No-op writes deliberately retain RV, as Kubernetes storage does.
fn commit_patch(pod: &mut Value, patch: &Value) -> u16 {
    if patch["metadata"]["uid"] != pod["metadata"]["uid"]
        || patch["metadata"]["resourceVersion"] != pod["metadata"]["resourceVersion"]
    {
        return 409;
    }
    let mut updated = pod.clone();
    for field in ["labels", "annotations"] {
        if let Some(values) = patch["metadata"][field].as_object() {
            for (key, value) in values {
                updated["metadata"][field][key] = value.clone();
            }
        }
    }
    if updated != *pod {
        let version = pod["metadata"]["resourceVersion"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            + 1;
        updated["metadata"]["resourceVersion"] = json!(version.to_string());
        *pod = updated;
    }
    200
}

// Model the replace preconditions used by these tests, not full API validation.
fn commit_lease(lease: &mut Value, replacement: &Value) -> u16 {
    if replacement["metadata"]["uid"] != lease["metadata"]["uid"] {
        return 422;
    }
    if replacement["metadata"]["resourceVersion"] != lease["metadata"]["resourceVersion"] {
        return 409;
    }
    if replacement != lease {
        let version = lease["metadata"]["resourceVersion"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            + 1;
        *lease = replacement.clone();
        lease["metadata"]["resourceVersion"] = json!(version.to_string());
    }
    200
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
            pod: json!({"apiVersion":"v1", "kind":"Pod", "metadata":{
                "name":"controller-a", "uid":"pod-uid", "resourceVersion":"1",
                "labels":{"edgion.io/leader":"false", "app":"controller"},
                "annotations":{"other":"preserved"}}}),
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
                    let is_pod = request.uri().path().contains("/pods/");
                    let method = request.method().clone();
                    let bytes = request.into_body().collect_bytes().await.unwrap();
                    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    let (reply, value, held) = {
                        let mut state = state.write();
                        let (reply, value) = match method {
                            Method::GET if is_pod => {
                                state.pod_gets += 1;
                                (state.pod_get, state.pod.clone())
                            }
                            Method::GET => {
                                state.gets += 1;
                                (state.get, state.lease.clone())
                            }
                            Method::PUT => {
                                state.puts.push(body.clone());
                                let reply = state.put;
                                let code = if let Reply::Error(code) = reply {
                                    code
                                } else {
                                    // Commit before holding the response: cancellation
                                    // cannot undo a successful server-side write.
                                    commit_lease(&mut state.lease, &body)
                                };
                                state.put_codes.push(code);
                                (
                                    if code == 200 { reply } else { Reply::Error(code) },
                                    state.lease.clone(),
                                )
                            }
                            Method::PATCH => {
                                state
                                    .labels
                                    .push(body["metadata"]["labels"]["edgion.io/leader"] == "true");
                                let reply = state.patch;
                                if matches!(reply, Reply::DeferCommit) {
                                    state.pending_patches.push(body);
                                    (Reply::Hold, state.pod.clone())
                                } else if matches!(reply, Reply::Error(_)) {
                                    (reply, state.pod.clone())
                                } else {
                                    let code = commit_patch(&mut state.pod, &body);
                                    let mut value = state.pod.clone();
                                    match state.invalid_ack {
                                        Some("resourceVersion") => {
                                            value["metadata"]["resourceVersion"] =
                                                body["metadata"]["resourceVersion"].clone()
                                        }
                                        Some("uid") => value["metadata"]["uid"] = json!("wrong-uid"),
                                        Some("label") => {
                                            value["metadata"]["labels"]["edgion.io/leader"] = json!("false")
                                        }
                                        Some("marker") => {
                                            value["metadata"]["annotations"][LEADER_LABEL_WRITE_ID_ANNOTATION] =
                                                json!("wrong-marker")
                                        }
                                        _ => {}
                                    }
                                    (if code == 200 { reply } else { Reply::Error(code) }, value)
                                }
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
    // Release the response before advancing to expiry. The direct election_wait
    // boundary test below controls readiness without the kube buffer in between.
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

#[tokio::test(start_paused = true)]
async fn canceled_label_commits_cannot_overwrite_newer_noop_state() {
    for old_value in [false, true] {
        let h = Harness::new(10);
        if !old_value {
            h.tick(2).await;
            h.api.write().put = Reply::Error(409);
        }
        h.api.write().patch = Reply::DeferCommit;
        h.tick(2).await;
        let old = h.api.write().pending_patches.pop().unwrap();
        assert_eq!(
            old["metadata"]["labels"]["edgion.io/leader"],
            json!(old_value.to_string())
        );
        h.api.write().patch = Reply::Success;
        h.api.write().put = if old_value { Reply::Error(409) } else { Reply::Success };
        h.tick(2).await;
        {
            let mut state = h.api.write();
            assert_eq!(
                state.pod["metadata"]["labels"]["edgion.io/leader"],
                json!((!old_value).to_string())
            );
            assert_eq!(commit_patch(&mut state.pod, &old), 409);
            assert_eq!(state.pod["metadata"]["labels"]["app"], "controller");
            assert_eq!(state.pod["metadata"]["annotations"]["other"], "preserved");
        }
        let reads = h.api.read().pod_gets;
        h.tick(2).await;
        assert_eq!(h.api.read().pod_gets, reads);
        h.stop().await;
    }
}

#[test]
fn server_model_retains_noop_version_and_exposes_rv_only_race() {
    let mut pod = json!({"metadata":{"uid":"uid", "resourceVersion":"1", "labels":{"edgion.io/leader":"true"}}});
    let same = pod.clone();
    assert_eq!(commit_patch(&mut pod, &same), 200);
    assert_eq!(pod["metadata"]["resourceVersion"], "1");
    let mut old = same;
    old["metadata"]["labels"]["edgion.io/leader"] = json!("false");
    assert_eq!(commit_patch(&mut pod, &old), 200);
    assert_eq!(pod["metadata"]["labels"]["edgion.io/leader"], "false");
}

#[tokio::test(start_paused = true)]
async fn pod_get_stalls_preserve_tight_renewal_and_shutdown_bounds() {
    let h = Harness::new(3);
    h.api.write().pod_get = Reply::Hold;
    h.tick(2).await;
    let fence = h.election.handle().current_fence().unwrap();
    for _ in 0..3 {
        h.tick(2).await;
        assert_eq!(h.election.handle().current_fence().unwrap(), fence);
    }
    assert!(h.api.read().labels.is_empty());
    h.api.write().get = Reply::Error(503);
    h.tick(2).await;
    h.tick(1).await;
    h.withdrawn();
    h.stop().await;

    let h = Harness::new(3);
    h.api.write().pod_get = Reply::Hold;
    h.tick(2).await;
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn pod_conflict_retries_fresh_read_without_losing_leadership() {
    let h = Harness::new(3);
    h.api.write().pod_get = Reply::Hold;
    h.tick(2).await;
    // Change the resource after GET captured its response, before PATCH arrives.
    h.api.write().pod["metadata"]["resourceVersion"] = json!("2");
    for response in std::mem::take(&mut h.api.write().held) {
        let _ = response.send(());
    }
    settle().await;
    assert!(h.election.is_leader());
    assert_eq!(h.api.read().pod["metadata"]["labels"]["edgion.io/leader"], "false");
    h.api.write().pod_get = Reply::Success;
    h.tick(2).await;
    assert!(h.election.is_leader());
    assert_eq!(h.api.read().pod["metadata"]["labels"]["edgion.io/leader"], "true");
    assert_eq!(h.api.read().pod_gets, 2);
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn pod_preflight_requires_metadata_and_distinguishes_permissions() {
    for field in ["uid", "resourceVersion"] {
        for value in [Value::Null, json!("")] {
            let h = Harness::new(10);
            h.api.write().pod["metadata"][field] = value;
            assert!(h.election.preflight_check().await.is_err());
            assert!(h.api.read().labels.is_empty());
            h.stop().await;
        }
    }
    for get in [false, true] {
        let h = Harness::new(10);
        if get {
            h.api.write().pod_get = Reply::Error(403);
        } else {
            h.api.write().patch = Reply::Error(403);
        }
        let error = h.election.preflight_check().await.unwrap_err().to_string();
        assert!(
            error.contains(if get { "cannot get Pod" } else { "cannot patch Pod" }),
            "{error}"
        );
        h.stop().await;
    }
}

#[tokio::test(start_paused = true)]
async fn recreated_pod_rejects_prepared_patch_and_invalid_ack_retries() {
    let h = Harness::new(10);
    h.api.write().patch = Reply::DeferCommit;
    h.tick(2).await;
    let old = h.api.write().pending_patches.pop().unwrap();
    {
        let mut state = h.api.write();
        state.pod["metadata"]["uid"] = json!("replacement-uid");
        assert_eq!(commit_patch(&mut state.pod, &old), 409);
        state.patch = Reply::Success;
        state.invalid_ack = Some("resourceVersion");
    }
    h.tick(2).await;
    let reads = h.api.read().pod_gets;
    h.api.write().invalid_ack = None;
    h.tick(2).await;
    assert_eq!(h.api.read().pod_gets, reads + 1);
    h.tick(2).await;
    assert_eq!(h.api.read().pod_gets, reads + 1);
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn altered_success_responses_remain_dirty_until_verified() {
    for field in ["uid", "resourceVersion", "label", "marker"] {
        let h = Harness::new(10);
        h.api.write().invalid_ack = Some(field);
        h.tick(2).await;
        h.tick(2).await;
        assert_eq!(h.api.read().pod_gets, 2, "{field}");
        h.api.write().invalid_ack = None;
        h.tick(2).await;
        h.tick(2).await;
        assert_eq!(h.api.read().pod_gets, 3, "{field}");
        h.stop().await;
    }
}

#[tokio::test(start_paused = true)]
async fn older_patch_committing_first_forces_fresh_read_retry() {
    let h = Harness::new(10);
    h.api.write().patch = Reply::DeferCommit;
    h.tick(2).await;
    let old = h.api.write().pending_patches.pop().unwrap();
    h.api.write().patch = Reply::Success;
    h.api.write().put = Reply::Error(409);
    h.api.write().pod_get = Reply::Hold;
    h.tick(2).await;
    h.withdrawn();
    // The new false attempt already read r1; the canceled true commits first.
    assert_eq!(commit_patch(&mut h.api.write().pod, &old), 200);
    for response in std::mem::take(&mut h.api.write().held) {
        let _ = response.send(());
    }
    settle().await;
    assert_eq!(h.api.read().pod["metadata"]["labels"]["edgion.io/leader"], "true");
    h.api.write().pod_get = Reply::Success;
    h.tick(2).await;
    assert_eq!(h.api.read().pod["metadata"]["labels"]["edgion.io/leader"], "false");
    assert_eq!(h.api.read().pod_gets, 3);
    h.stop().await;
}

#[test]
fn lease_replace_model_rejects_stale_metadata_without_mutation() {
    let mut lease = json!({"metadata":{"uid":"lease-uid", "resourceVersion":"7"},
        "spec":{"holderIdentity":"controller-a"}});
    let original = lease.clone();
    assert_eq!(commit_lease(&mut lease, &original), 200);
    assert_eq!(lease, original);
    for (field, value, code) in [("uid", "old-uid", 422), ("resourceVersion", "6", 409)] {
        let mut stale = original.clone();
        stale["metadata"][field] = json!(value);
        stale["spec"]["holderIdentity"] = json!("stale-writer");
        assert_eq!(commit_lease(&mut lease, &stale), code);
        assert_eq!(lease, original);
    }
    let mut replacement = original.clone();
    replacement["spec"]["holderIdentity"] = json!("controller-b");
    assert_eq!(commit_lease(&mut lease, &replacement), 200);
    replacement["metadata"]["resourceVersion"] = json!("8");
    assert_eq!(lease, replacement);
}

#[tokio::test(start_paused = true)]
async fn competing_lease_write_rejects_stale_renewal_and_withdraws_authority() {
    let h = Harness::new(10);
    h.tick(2).await;
    assert!(h.election.is_leader());
    let snapshot = h.api.read().lease.clone();
    h.api.write().get = Reply::Hold;
    h.tick(2).await;
    assert_eq!(h.api.read().held.len(), 1);
    let successor = {
        let mut state = h.api.write();
        let mut replacement = state.lease.clone();
        replacement["spec"]["holderIdentity"] = json!("controller-b");
        replacement["metadata"]["annotations"][LEADER_TERM_ANNOTATION] = json!("successor-term");
        assert_eq!(commit_lease(&mut state.lease, &replacement), 200);
        state.get = Reply::Success;
        state.lease.clone()
    };
    for response in std::mem::take(&mut h.api.write().held) {
        response.send(()).unwrap();
    }
    settle().await;
    h.withdrawn();
    {
        let state = h.api.read();
        assert_eq!(state.puts.len(), 2);
        assert_eq!(state.put_codes, [200, 409]);
        assert_eq!(
            state.puts[1]["metadata"]["resourceVersion"],
            snapshot["metadata"]["resourceVersion"]
        );
        assert_ne!(
            snapshot["metadata"]["resourceVersion"],
            successor["metadata"]["resourceVersion"]
        );
        assert_eq!(state.lease, successor);
    }
    h.stop().await;
}

#[tokio::test(start_paused = true)]
async fn election_wait_accepts_success_only_before_absolute_deadline() {
    for seconds in [9, 10, 11] {
        let ready = Cell::new(false);
        let operation = poll_fn(|_| if ready.get() { Poll::Ready(42) } else { Poll::Pending });
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut shutdown = None;
        let wait = election_wait(operation, Some(deadline), &mut shutdown);
        tokio::pin!(wait);
        // Register the expiry timer while the operation is definitely pending.
        assert!(futures::poll!(wait.as_mut()).is_pending());
        tokio::time::advance(Duration::from_secs(seconds)).await;
        ready.set(true);
        // Poll explicitly on this task: no response transport or wakeup ordering
        // can prevent the operation from being ready at this exact instant.
        let result = futures::poll!(wait.as_mut());
        if seconds < 10 {
            assert!(matches!(result, Poll::Ready(ElectionWait::Completed(42))));
        } else {
            assert!(matches!(result, Poll::Ready(ElectionWait::Expired)));
        }
    }
}
