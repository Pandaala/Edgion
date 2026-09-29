use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use edgion_resources::resources::link_sys::LinkSys;

use super::*;
use crate::link_sys::runtime::apply::FingerprintHasher;

static FAKE_BUILD_FAILS: AtomicUsize = AtomicUsize::new(0);

struct FakeOwner;

struct FakeClient {
    fingerprint: RuntimeFingerprint,
    live: usize,
    healthy: Arc<PublishedHealth>,
    inits: Arc<AtomicUsize>,
    shutdowns: Arc<AtomicUsize>,
    retired: AtomicBool,
    init_ok: bool,
}

fn redis_ls(name: &str, endpoint: &str, rv: &str) -> LinkSys {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "LinkSys",
        "metadata": {"namespace": "runner", "name": name, "resourceVersion": rv},
        "spec": {"type": "redis", "config": {"endpoints": [endpoint]}}
    }))
    .expect("test LinkSys")
}

fn fp_for(ls: &LinkSys) -> RuntimeFingerprint {
    let mut rebuild = FingerprintHasher::new();
    if let edgion_resources::resources::link_sys::SystemConfig::Redis(cfg) = &ls.spec.config {
        for endpoint in &cfg.endpoints {
            rebuild.bytes(endpoint.as_bytes());
        }
    }
    RuntimeFingerprint::from_parts(rebuild.finish(), [0; 32], ls.metadata.resource_version.clone())
}

#[async_trait]
impl ApplyOwner for FakeOwner {
    type Client = FakeClient;

    fn fingerprint(incoming: &LinkSys) -> anyhow::Result<RuntimeFingerprint> {
        Ok(fp_for(incoming))
    }

    fn fingerprint_live(client: &Self::Client) -> RuntimeFingerprint {
        client.fingerprint.clone()
    }

    fn coverage(client: &Self::Client) -> ConnectionCoverage {
        ConnectionCoverage {
            live: client.live,
            configured: 1,
        }
    }

    fn published_health(client: &Self::Client) -> Option<&PublishedHealth> {
        Some(client.healthy.as_ref())
    }

    fn build(key: &str, incoming: &LinkSys) -> anyhow::Result<Arc<Self::Client>> {
        let endpoint = match &incoming.spec.config {
            edgion_resources::resources::link_sys::SystemConfig::Redis(cfg) => {
                cfg.endpoints.first().cloned().unwrap_or_default()
            }
            _ => String::new(),
        };
        if endpoint.contains("invalid") {
            if key == "runner/invalid-build" {
                FAKE_BUILD_FAILS.fetch_add(1, Ordering::Relaxed);
            }
            anyhow::bail!("invalid spec");
        }
        Ok(Arc::new(FakeClient {
            fingerprint: fp_for(incoming),
            live: if endpoint.contains("down") { 0 } else { 1 },
            healthy: crate::link_sys::runtime::health_probe::new_published_health(key, true),
            inits: Arc::new(AtomicUsize::new(0)),
            shutdowns: Arc::new(AtomicUsize::new(0)),
            retired: AtomicBool::new(false),
            init_ok: !endpoint.contains("fail"),
        }))
    }

    async fn init(client: &Self::Client) -> anyhow::Result<()> {
        client.inits.fetch_add(1, Ordering::Relaxed);
        if client.init_ok {
            Ok(())
        } else {
            anyhow::bail!("init refused")
        }
    }

    async fn shutdown(client: Arc<Self::Client>) {
        client.shutdowns.fetch_add(1, Ordering::Relaxed);
    }

    fn mark_retired(client: &Self::Client) {
        client.healthy.deactivate();
        client.retired.store(true, Ordering::Relaxed);
    }

    fn label() -> &'static str {
        "Fake"
    }
}

async fn wait_until(mut pred: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !pred() {
        if tokio::time::Instant::now() >= deadline {
            panic!("timed out waiting for runner condition");
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn first_successful_init_publishes() {
    let runner = PersistentRunner::<FakeOwner>::spawn("runner/publish".to_string());
    assert!(runner.current().is_none());
    runner.post_ready(Arc::new(redis_ls("publish", "redis://up:6379", "1")), false);
    wait_until(|| runner.current().is_some()).await;
    runner.begin_delete();
    assert!(runner.current().is_none());
    runner.await_join().await;
    assert!(runner.current().is_none());
}

#[tokio::test]
async fn removal_releases_idle_owner_without_awaiting_join() {
    let runner = PersistentRunner::<FakeOwner>::spawn("runner/removed-idle".into());
    let weak = Arc::downgrade(&runner);
    tokio::task::yield_now().await;
    runner.begin_delete();
    drop(runner);
    wait_until(|| weak.upgrade().is_none()).await;
}

#[tokio::test]
async fn removal_releases_owner_while_a_caller_retains_the_client() {
    let link = redis_ls("removed-retained", "redis://up:6379", "1");
    let client = FakeOwner::build("runner/removed-retained", &link).unwrap();
    let runner = PersistentRunner::<FakeOwner>::spawn_with_published("runner/removed-retained".into(), client.clone());
    let weak = Arc::downgrade(&runner);
    runner.begin_delete();
    assert!(runner.current().is_none());
    drop(runner);
    wait_until(|| weak.upgrade().is_none()).await;
    assert!(client.retired.load(Ordering::Relaxed));
    assert_eq!(client.shutdowns.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn equal_fingerprint_reuses_published_arc() {
    let runner = PersistentRunner::<FakeOwner>::spawn("runner/reuse".to_string());
    let first_ls = Arc::new(redis_ls("reuse", "redis://up:6379", "1"));
    runner.post_ready(Arc::clone(&first_ls), false);
    wait_until(|| runner.current().is_some()).await;
    let first = runner.current().unwrap();
    runner.post_ready(Arc::new(redis_ls("reuse", "redis://up:6379", "2")), false);
    tokio::time::sleep(Duration::from_millis(30)).await;
    let second = runner.current().unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    runner.begin_delete();
    runner.await_join().await;
}

#[tokio::test(flavor = "current_thread")]
async fn equal_replay_coalesced_with_forced_post_still_rebuilds() {
    let link = redis_ls("coalesced-force", "redis://up:6379", "1");
    let first = FakeOwner::build("runner/coalesced-force", &link).unwrap();
    let runner =
        PersistentRunner::<FakeOwner>::spawn_with_published("runner/coalesced-force".to_string(), first.clone());

    runner.post_ready(Arc::new(link.clone()), true);
    runner.post_ready(Arc::new(link), false);
    wait_until(|| runner.current().is_some_and(|client| !Arc::ptr_eq(&client, &first))).await;

    runner.begin_delete();
    runner.await_join().await;
}

#[tokio::test(flavor = "current_thread")]
async fn equal_replay_superseding_forced_candidate_still_rebuilds() {
    let link = redis_ls("superseded-force", "redis://up:6379", "1");
    let first = FakeOwner::build("runner/superseded-force", &link).unwrap();
    let runner =
        PersistentRunner::<FakeOwner>::spawn_with_published("runner/superseded-force".to_string(), first.clone());
    let replay = runner.clone();
    let replay_link = link.clone();
    *runner.publish_entry.lock() = Some(Box::new(move || replay.post_ready(Arc::new(replay_link), false)));

    runner.post_ready(Arc::new(link), true);
    wait_until(|| runner.current().is_some_and(|client| !Arc::ptr_eq(&client, &first))).await;

    runner.begin_delete();
    runner.await_join().await;
}

#[tokio::test]
async fn failed_init_does_not_publish() {
    let runner = PersistentRunner::<FakeOwner>::spawn("runner/fail".to_string());
    runner.post_ready(Arc::new(redis_ls("fail", "redis://fail:6379", "1")), false);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(runner.current().is_none());
    runner.begin_delete();
    runner.await_join().await;
}

#[tokio::test]
async fn failed_handover_keeps_published_generation() {
    let first_ls = redis_ls("failed-handover", "redis://up:6379", "1");
    let first_client = FakeOwner::build("runner/failed-handover", &first_ls).unwrap();
    let runner =
        PersistentRunner::<FakeOwner>::spawn_with_published("runner/failed-handover".to_string(), first_client.clone());
    runner.post_ready(Arc::new(redis_ls("failed-handover", "redis://fail:6379", "2")), false);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let current = runner.current().expect("old generation remains published");
    assert!(Arc::ptr_eq(&first_client, &current));
    runner.begin_delete();
    runner.await_join().await;
}

#[tokio::test]
async fn live_zero_replace_now_swaps_generation() {
    let first_ls = redis_ls("replace", "redis://down:6379", "1");
    let first_client = FakeOwner::build("runner/replace", &first_ls).unwrap();
    let runner = PersistentRunner::<FakeOwner>::spawn_with_published("runner/replace".to_string(), first_client);
    let first = runner.current().unwrap();
    runner.post_ready(Arc::new(redis_ls("replace", "redis://up:6379", "2")), false);
    wait_until(|| runner.current().is_some_and(|c| !Arc::ptr_eq(&c, &first))).await;
    assert!(first.retired.load(Ordering::Relaxed));
    runner.begin_delete();
    runner.await_join().await;
}

#[tokio::test(flavor = "current_thread")]
async fn forced_rebuild_retires_live_client_before_failed_replacement_and_recovers() {
    for endpoint in ["redis://invalid:6379", "redis://fail:6379"] {
        let link = redis_ls("force-failure", "redis://up:6379", "1");
        let first = FakeOwner::build("runner/force-failure", &link).unwrap();
        let runner =
            PersistentRunner::<FakeOwner>::spawn_with_published("runner/force-failure".to_string(), first.clone());
        runner.post_ready(Arc::new(redis_ls("force-failure", endpoint, "2")), true);
        wait_until(|| first.shutdowns.load(Ordering::Relaxed) == 1).await;
        assert!(runner.current().is_none());
        assert!(first.retired.load(Ordering::Relaxed));

        runner.post_ready(Arc::new(link), false);
        wait_until(|| runner.current().is_some()).await;
        assert!(!Arc::ptr_eq(&first, &runner.current().unwrap()));
        runner.begin_delete();
        runner.await_join().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn forced_not_ready_survives_ordinary_replay_and_retires_live_client() {
    let link = redis_ls("force-not-ready", "redis://up:6379", "1");
    let first = FakeOwner::build("runner/force-not-ready", &link).unwrap();
    let runner =
        PersistentRunner::<FakeOwner>::spawn_with_published("runner/force-not-ready".to_string(), first.clone());
    runner.post_not_ready(true);
    runner.post_not_ready(false);
    wait_until(|| first.shutdowns.load(Ordering::Relaxed) == 1).await;
    assert!(runner.current().is_none());
    assert!(first.retired.load(Ordering::Relaxed));

    runner.post_ready(Arc::new(link), false);
    wait_until(|| runner.current().is_some()).await;
    assert!(!Arc::ptr_eq(&first, &runner.current().unwrap()));
    runner.begin_delete();
    runner.await_join().await;
}

#[tokio::test]
async fn live_positive_handover_publishes_next_before_old_shutdown() {
    let first_ls = redis_ls("handover", "redis://up:6379", "1");
    let first_client = FakeOwner::build("runner/handover", &first_ls).unwrap();
    let runner = PersistentRunner::<FakeOwner>::spawn_with_published("runner/handover".to_string(), first_client);
    let first = runner.current().unwrap();
    runner.post_ready(Arc::new(redis_ls("handover", "redis://up2:6379", "2")), false);
    wait_until(|| runner.current().is_some_and(|c| !Arc::ptr_eq(&c, &first))).await;
    assert!(first.retired.load(Ordering::Relaxed));
    assert!(runner.current().is_some());
    runner.begin_delete();
    runner.await_join().await;
}

#[tokio::test(flavor = "current_thread")]
async fn handover_reports_only_the_new_published_generation() {
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};

    let key = "runner/health-handover";
    let old_link = redis_ls("health-handover", "redis://up:6379", "1");
    let old = FakeOwner::build(key, &old_link).unwrap();
    let runner = PersistentRunner::<FakeOwner>::spawn_with_published(key.to_string(), Arc::clone(&old));
    let next_link = redis_ls("health-handover", "redis://up2:6379", "2");
    let next = FakeOwner::build(key, &next_link).unwrap();

    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    metrics::with_local_recorder(&recorder, || {
        assert!(matches!(
            publish_with_hooks(&runner, &next, 0, || {}, || {}),
            LoopExit::Continue
        ));
        assert!(old.healthy.update(false, false));
        assert!(next.healthy.update(false, false));
    });

    let flips: Vec<_> = snapshotter
        .snapshot()
        .into_vec()
        .into_iter()
        .filter(|(metric, _, _, _)| {
            metric.key().name() == crate::observe::metrics::registry::names::LINK_SYS_HEALTH_FLIPS_TOTAL
                && metric
                    .key()
                    .labels()
                    .any(|label| label.key() == "link" && label.value() == key)
        })
        .collect();
    assert_eq!(flips.len(), 1, "one degraded series must be present");
    assert!(flips[0]
        .0
        .key()
        .labels()
        .any(|label| label.key() == "direction" && label.value() == "degraded"));
    assert!(matches!(flips[0].3, DebugValue::Counter(value) if value == 1));

    runner.begin_delete();
    runner.await_join().await;
}

#[tokio::test]
async fn superseded_at_publish_keeps_old_client_and_runner() {
    let first_ls = redis_ls("stale-publish", "redis://up:6379", "1");
    let first = FakeOwner::build("runner/stale-publish", &first_ls).unwrap();
    let runner = PersistentRunner::<FakeOwner>::spawn_with_published("runner/stale-publish".to_string(), first.clone());
    let supersede = runner.clone();
    *runner.publish_entry.lock() = Some(Box::new(move || supersede.post_not_ready(false)));

    runner.post_ready(Arc::new(redis_ls("stale-publish", "redis://up2:6379", "2")), false);
    wait_until(|| matches!(runner.desired.borrow().state, DesiredState::NotReady)).await;
    assert!(Arc::ptr_eq(&runner.current().unwrap(), &first));
    assert_eq!(first.shutdowns.load(Ordering::Relaxed), 0);
    assert!(!first.retired.load(Ordering::Relaxed));
    assert!(!runner.join.lock().as_ref().unwrap().is_finished());

    runner.post_ready(Arc::new(redis_ls("stale-publish", "redis://up3:6379", "3")), false);
    wait_until(|| runner.current().is_some_and(|client| !Arc::ptr_eq(&client, &first))).await;
    runner.begin_delete();
    runner.await_join().await;
}

#[tokio::test(flavor = "current_thread")]
async fn desired_post_during_swap_keeps_published_client_and_runner() {
    let first_ls = redis_ls("swap-post", "redis://up:6379", "1");
    let first = FakeOwner::build("runner/swap-post", &first_ls).unwrap();
    let runner = PersistentRunner::<FakeOwner>::spawn_with_published("runner/swap-post".to_string(), first);
    let next_ls = redis_ls("swap-post", "redis://up2:6379", "2");
    let next = FakeOwner::build("runner/swap-post", &next_ls).unwrap();
    runner.post_ready(Arc::new(next_ls), false);
    let start_gen = runner.desired.borrow().gen;
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let posting_runner = runner.clone();
    let mut post = None;

    let outcome = publish_with_hooks(
        &runner,
        &next,
        start_gen,
        || {},
        || {
            post = Some(std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                posting_runner.post_not_ready(false);
                done_tx.send(()).unwrap();
            }));
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            assert!(matches!(
                done_rx.recv_timeout(Duration::from_millis(30)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
        },
    );
    assert!(matches!(outcome, LoopExit::Continue));
    post.unwrap().join().unwrap();
    assert!(Arc::ptr_eq(&runner.current().unwrap(), &next));
    assert!(!next.retired.load(Ordering::Relaxed));
    tokio::task::yield_now().await;
    assert!(Arc::ptr_eq(&runner.current().unwrap(), &next));
    assert!(!runner.join.lock().as_ref().unwrap().is_finished());

    runner.post_ready(Arc::new(redis_ls("swap-post", "redis://up3:6379", "3")), false);
    wait_until(|| runner.current().is_some_and(|client| !Arc::ptr_eq(&client, &next))).await;
    runner.begin_delete();
    runner.await_join().await;
}

#[tokio::test(flavor = "current_thread")]
async fn delete_during_swap_unpublishes_the_new_client() {
    let runner = PersistentRunner::<FakeOwner>::spawn("runner/swap-delete".to_string());
    let link = redis_ls("swap-delete", "redis://up:6379", "1");
    let client = FakeOwner::build("runner/swap-delete", &link).unwrap();
    runner.post_ready(Arc::new(link), false);
    let start_gen = runner.desired.borrow().gen;
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let deleting_runner = runner.clone();
    let mut delete = None;

    let outcome = publish_with_hooks(
        &runner,
        &client,
        start_gen,
        || {},
        || {
            delete = Some(std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                deleting_runner.begin_delete();
                done_tx.send(()).unwrap();
            }));
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            assert!(matches!(
                done_rx.recv_timeout(Duration::from_millis(30)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
        },
    );
    assert!(matches!(outcome, LoopExit::Continue));
    delete.unwrap().join().unwrap();
    assert!(runner.current().is_none());
    runner.await_join().await;
    assert_eq!(client.shutdowns.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn delete_before_or_during_first_publish_leaves_current_empty() {
    let runner = PersistentRunner::<FakeOwner>::spawn("runner/delete-first".to_string());
    runner.post_ready(Arc::new(redis_ls("delete-first", "redis://up:6379", "1")), false);
    runner.begin_delete();
    runner.await_join().await;
    assert!(runner.current().is_none());
}

#[tokio::test]
async fn equal_fingerprint_with_live_zero_does_not_rebuild() {
    let first_ls = redis_ls("live0-reuse", "redis://down:6379", "1");
    let first_client = FakeOwner::build("runner/live0-reuse", &first_ls).unwrap();
    let inits = first_client.inits.clone();
    let runner = PersistentRunner::<FakeOwner>::spawn_with_published("runner/live0-reuse".to_string(), first_client);
    runner.post_ready(Arc::new(redis_ls("live0-reuse", "redis://down:6379", "2")), false);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(inits.load(Ordering::Relaxed), 0);
    assert!(runner.current().is_some());
    runner.begin_delete();
    runner.await_join().await;
}

#[tokio::test(start_paused = true)]
async fn deterministic_build_failure_waits_for_new_desired() {
    let key = "runner/invalid-build".to_string();
    FAKE_BUILD_FAILS.store(0, Ordering::Relaxed);
    let runner = PersistentRunner::<FakeOwner>::spawn(key);
    runner.post_ready(Arc::new(redis_ls("invalid-build", "redis://invalid:6379", "1")), false);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(FAKE_BUILD_FAILS.load(Ordering::Relaxed), 1);
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        FAKE_BUILD_FAILS.load(Ordering::Relaxed),
        1,
        "build errors must not retry on connect backoff"
    );
    runner.begin_delete();
    runner.await_join().await;
}
