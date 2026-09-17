//! Per-`namespace/name` persistent owner for long-lived LinkSys clients.
//!
//! The dispatcher posts [`DesiredState`]; this task is the sole publisher of the
//! small handle and owns connect backoff, handover retire, and delete.

use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use edgion_resources::resources::link_sys::LinkSys;
use parking_lot::Mutex;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::apply::{
    next_connect_backoff, plan_apply, ApplyPlan, ConnectionCoverage, RuntimeFingerprint, CONNECT_BACKOFF_MAX,
    CONNECT_BACKOFF_START, HANDOVER_RETIRE,
};

/// Posted by the dispatcher. `gen` increments on every post so an in-flight
/// `init` can detect that desired has moved on (D2).
#[derive(Clone)]
pub struct Desired {
    pub gen: u64,
    pub state: DesiredState,
}

#[derive(Clone)]
pub enum DesiredState {
    Ready { link: Arc<LinkSys>, forced: bool },
    NotReady,
}

enum LoopExit {
    Continue,
    Aborted,
    Shutdown,
}

/// Provider-specific generation factory used by [`PersistentRunner`].
#[async_trait]
pub trait ApplyOwner: Send + Sync + 'static {
    type Client: Send + Sync + 'static;

    fn fingerprint(incoming: &LinkSys) -> anyhow::Result<RuntimeFingerprint>;
    fn fingerprint_live(client: &Self::Client) -> RuntimeFingerprint;
    fn coverage(client: &Self::Client) -> ConnectionCoverage;
    fn build(key: &str, incoming: &LinkSys) -> anyhow::Result<Arc<Self::Client>>;
    async fn init(client: &Self::Client) -> anyhow::Result<()>;
    async fn shutdown(client: Arc<Self::Client>);
    fn mark_retired(client: &Self::Client);
    fn label() -> &'static str;
}

pub struct PersistentRunner<O: ApplyOwner> {
    key: String,
    desired: watch::Sender<Desired>,
    current: ArcSwap<Option<Arc<O::Client>>>,
    shutdown: CancellationToken,
    join: Mutex<Option<JoinHandle<()>>>,
    /// Generations unpublished by delete/lost-publish races; the loop must quit all of them.
    orphan: Mutex<Vec<Arc<O::Client>>>,
    /// Retiring generation task. Delete joins it before the runner exits.
    retire: Mutex<Option<RetireTask>>,
}

struct RetireTask {
    abort: Option<oneshot::Sender<()>>,
    join: JoinHandle<()>,
}

impl<O: ApplyOwner> PersistentRunner<O> {
    pub fn spawn(key: String) -> Arc<Self> {
        let (tx, rx) = watch::channel(Desired {
            gen: 0,
            state: DesiredState::NotReady,
        });
        let shutdown = CancellationToken::new();
        let runner = Arc::new(Self {
            key,
            desired: tx,
            current: ArcSwap::from_pointee(None),
            shutdown: shutdown.clone(),
            join: Mutex::new(None),
            orphan: Mutex::new(Vec::new()),
            retire: Mutex::new(None),
        });
        let loop_runner = Arc::clone(&runner);
        let join = tokio::spawn(run_loop::<O>(loop_runner, rx, shutdown));
        *runner.join.lock() = Some(join);
        runner
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn current(&self) -> Option<Arc<O::Client>> {
        (*self.current.load_full()).clone()
    }

    pub fn post_ready(&self, link: Arc<LinkSys>, forced: bool) {
        self.desired.send_modify(|desired| {
            desired.gen = desired.gen.saturating_add(1);
            desired.state = DesiredState::Ready { link, forced };
        });
    }

    pub fn post_not_ready(&self) {
        self.desired.send_modify(|desired| {
            desired.gen = desired.gen.saturating_add(1);
            desired.state = DesiredState::NotReady;
        });
    }

    /// Unpublish immediately and cancel the loop. The loop shuts down inners.
    pub fn begin_delete(&self) {
        tracing::info!(key = %self.key(), label = O::label(), "deleting LinkSys runner");
        self.shutdown.cancel();
        self.unpublish_into_orphan();
        self.abort_retire();
    }

    fn unpublish_into_orphan(&self) {
        if let Some(client) = (*self.current.swap(Arc::new(None))).clone() {
            push_orphan(self, client);
        }
    }

    fn abort_retire(&self) {
        let mut retire = self.retire.lock();
        if let Some(task) = retire.as_mut() {
            if let Some(abort) = task.abort.take() {
                let _ = abort.send(());
            }
        }
    }

    pub async fn await_join(&self) {
        let handle = self.join.lock().take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }

    #[cfg(test)]
    pub fn spawn_with_published(key: String, client: Arc<O::Client>) -> Arc<Self> {
        let runner = Self::spawn(key);
        runner.current.store(Arc::new(Some(client)));
        runner
    }
}

impl<O: ApplyOwner> Drop for PersistentRunner<O> {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

struct ConnectBackoff {
    current: Duration,
}

impl ConnectBackoff {
    fn new() -> Self {
        Self {
            current: CONNECT_BACKOFF_START,
        }
    }

    fn reset(&mut self) {
        self.current = CONNECT_BACKOFF_START;
    }

    fn next_wait(&mut self) -> Duration {
        let wait = self.current;
        self.current = next_connect_backoff(self.current, CONNECT_BACKOFF_MAX);
        wait
    }
}

async fn run_loop<O: ApplyOwner>(
    runner: Arc<PersistentRunner<O>>,
    mut rx: watch::Receiver<Desired>,
    shutdown: CancellationToken,
) {
    let mut backoff = ConnectBackoff::new();
    loop {
        if shutdown.is_cancelled() {
            finish_delete::<O>(&runner).await;
            return;
        }

        let snapshot = rx.borrow().clone();
        match snapshot.state {
            DesiredState::NotReady => {
                backoff.reset();
                if wait_change_or_shutdown(&mut rx, &shutdown).await {
                    finish_delete::<O>(&runner).await;
                    return;
                }
            }
            DesiredState::Ready { link, forced } => {
                let start_gen = snapshot.gen;
                let incoming_fp = match O::fingerprint(&link) {
                    Ok(fp) => fp,
                    Err(e) => {
                        tracing::error!(
                            key = %runner.key,
                            label = O::label(),
                            error = %e,
                            "fingerprint failed"
                        );
                        if wait_change_or_shutdown(&mut rx, &shutdown).await {
                            finish_delete::<O>(&runner).await;
                            return;
                        }
                        continue;
                    }
                };

                if let Some(live) = runner.current() {
                    let live_fp = O::fingerprint_live(&live);
                    let plan = plan_apply(Some(&live_fp), &incoming_fp, O::coverage(&live), forced);
                    match plan {
                        ApplyPlan::Reuse => {
                            backoff.reset();
                            if wait_change_or_shutdown(&mut rx, &shutdown).await {
                                finish_delete::<O>(&runner).await;
                                return;
                            }
                            continue;
                        }
                        ApplyPlan::ReplaceNow => {
                            replace_now::<O>(&runner, live).await;
                            if shutdown.is_cancelled() {
                                finish_delete::<O>(&runner).await;
                                return;
                            }
                        }
                        ApplyPlan::Handover => {}
                        ApplyPlan::Delete => {
                            finish_delete::<O>(&runner).await;
                            return;
                        }
                    }
                }

                match connect_generation::<O>(&runner, &mut rx, &shutdown, &link, start_gen, &mut backoff).await {
                    LoopExit::Continue => {
                        backoff.reset();
                        if wait_change_or_shutdown(&mut rx, &shutdown).await {
                            finish_delete::<O>(&runner).await;
                            return;
                        }
                    }
                    LoopExit::Aborted => {}
                    LoopExit::Shutdown => {
                        finish_delete::<O>(&runner).await;
                        return;
                    }
                }
            }
        }
    }
}

/// `true` means the loop must exit (shutdown or sender dropped).
async fn wait_change_or_shutdown(rx: &mut watch::Receiver<Desired>, shutdown: &CancellationToken) -> bool {
    tokio::select! {
        _ = shutdown.cancelled() => true,
        result = rx.changed() => result.is_err(),
    }
}

async fn wait_until_gen_changes(rx: &mut watch::Receiver<Desired>, start_gen: u64) {
    loop {
        if rx.changed().await.is_err() {
            return;
        }
        if rx.borrow().gen != start_gen {
            return;
        }
    }
}

async fn connect_generation<O: ApplyOwner>(
    runner: &PersistentRunner<O>,
    rx: &mut watch::Receiver<Desired>,
    shutdown: &CancellationToken,
    link: &LinkSys,
    start_gen: u64,
    backoff: &mut ConnectBackoff,
) -> LoopExit {
    loop {
        if shutdown.is_cancelled() {
            return LoopExit::Shutdown;
        }
        if rx.borrow().gen != start_gen {
            return LoopExit::Aborted;
        }
        if matches!(rx.borrow().state, DesiredState::NotReady) {
            return LoopExit::Aborted;
        }

        // D5: at most current + one connecting/retiring generation. A previous
        // handover's retire must not overlap a new connect.
        if runner.current().is_some() {
            join_retire::<O>(runner).await;
        }

        let client = match O::build(&runner.key, link) {
            Ok(client) => client,
            Err(e) => {
                tracing::error!(
                    key = %runner.key,
                    label = O::label(),
                    error = %e,
                    "from_config failed"
                );
                // Construction is a pure function of the current desired spec.
                // Do not retry it until a newer desired generation arrives.
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => return LoopExit::Shutdown,
                    _ = wait_until_gen_changes(rx, start_gen) => return LoopExit::Aborted,
                }
            }
        };

        let init_result = tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                O::shutdown(client).await;
                return LoopExit::Shutdown;
            }
            _ = wait_until_gen_changes(rx, start_gen) => {
                O::shutdown(client).await;
                return LoopExit::Aborted;
            }
            result = O::init(&client) => result,
        };

        match init_result {
            Ok(()) => {
                if shutdown.is_cancelled() {
                    O::shutdown(client).await;
                    return LoopExit::Shutdown;
                }
                if rx.borrow().gen != start_gen {
                    O::shutdown(client).await;
                    return LoopExit::Aborted;
                }
                if !publish::<O>(runner, client, start_gen) {
                    return LoopExit::Shutdown;
                }
                return LoopExit::Continue;
            }
            Err(e) => {
                tracing::warn!(
                    key = %runner.key,
                    label = O::label(),
                    error = %e,
                    "connect failed; retrying with runner backoff"
                );
                O::shutdown(client).await;
                match wait_backoff(rx, shutdown, start_gen, backoff.next_wait()).await {
                    LoopExit::Continue => continue,
                    other => return other,
                }
            }
        }
    }
}

/// [`LoopExit::Continue`] means the sleep elapsed and the caller should retry.
async fn wait_backoff(
    rx: &mut watch::Receiver<Desired>,
    shutdown: &CancellationToken,
    start_gen: u64,
    wait: Duration,
) -> LoopExit {
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => LoopExit::Shutdown,
        _ = wait_until_gen_changes(rx, start_gen) => LoopExit::Aborted,
        _ = tokio::time::sleep(wait) => LoopExit::Continue,
    }
}

/// Returns false if the runner was cancelled and the generation must not stay published.
fn publish<O: ApplyOwner>(runner: &PersistentRunner<O>, client: Arc<O::Client>, start_gen: u64) -> bool {
    if runner.shutdown.is_cancelled() || runner.desired.borrow().gen != start_gen {
        push_orphan(runner, client);
        return false;
    }
    let previous = runner.current.swap(Arc::new(Some(client.clone())));
    if runner.shutdown.is_cancelled() || runner.desired.borrow().gen != start_gen {
        let loaded = runner.current.load_full();
        if loaded.as_ref().as_ref().is_some_and(|c| Arc::ptr_eq(c, &client)) {
            runner.current.store(Arc::new(None));
        }
        push_orphan(runner, client);
        if let Some(old) = (*previous).clone() {
            push_orphan(runner, old);
        }
        return false;
    }
    if let Some(old) = (*previous).clone() {
        O::mark_retired(&old);
        start_retire::<O>(runner, old);
    }
    true
}

fn push_orphan<O: ApplyOwner>(runner: &PersistentRunner<O>, client: Arc<O::Client>) {
    O::mark_retired(&client);
    runner.orphan.lock().push(client);
}

async fn replace_now<O: ApplyOwner>(runner: &PersistentRunner<O>, live: Arc<O::Client>) {
    let loaded = runner.current.load_full();
    if loaded.as_ref().as_ref().is_some_and(|c| Arc::ptr_eq(c, &live)) {
        runner.current.store(Arc::new(None));
    }
    if runner.shutdown.is_cancelled() {
        push_orphan(runner, live);
        return;
    }
    O::mark_retired(&live);
    O::shutdown(live).await;
}

fn start_retire<O: ApplyOwner>(runner: &PersistentRunner<O>, old: Arc<O::Client>) {
    let (tx, rx) = oneshot::channel();
    let cancel = runner.shutdown.clone();
    let join = tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(HANDOVER_RETIRE) => {}
            _ = rx => {}
            _ = cancel.cancelled() => {}
        }
        O::shutdown(old).await;
    });
    *runner.retire.lock() = Some(RetireTask { abort: Some(tx), join });
}

async fn join_retire<O: ApplyOwner>(runner: &PersistentRunner<O>) {
    let task = runner.retire.lock().take();
    if let Some(mut task) = task {
        if let Some(abort) = task.abort.take() {
            let _ = abort.send(());
        }
        let _ = task.join.await;
    }
}

async fn finish_delete<O: ApplyOwner>(runner: &PersistentRunner<O>) {
    runner.unpublish_into_orphan();
    join_retire::<O>(runner).await;
    let orphans = std::mem::take(&mut *runner.orphan.lock());
    for client in orphans {
        O::shutdown(client).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use edgion_resources::resources::link_sys::LinkSys;

    use super::*;
    use crate::link_sys::runtime::apply::FingerprintHasher;

    static FAKE_BUILD_FAILS: AtomicUsize = AtomicUsize::new(0);

    struct FakeOwner;

    struct FakeClient {
        fingerprint: RuntimeFingerprint,
        live: usize,
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

        fn build(_key: &str, incoming: &LinkSys) -> anyhow::Result<Arc<Self::Client>> {
            let endpoint = match &incoming.spec.config {
                edgion_resources::resources::link_sys::SystemConfig::Redis(cfg) => {
                    cfg.endpoints.first().cloned().unwrap_or_default()
                }
                _ => String::new(),
            };
            if endpoint.contains("invalid") {
                FAKE_BUILD_FAILS.fetch_add(1, Ordering::Relaxed);
                anyhow::bail!("invalid spec");
            }
            Ok(Arc::new(FakeClient {
                fingerprint: fp_for(incoming),
                live: if endpoint.contains("down") { 0 } else { 1 },
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
        let runner = PersistentRunner::<FakeOwner>::spawn_with_published(
            "runner/failed-handover".to_string(),
            first_client.clone(),
        );
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
        let runner =
            PersistentRunner::<FakeOwner>::spawn_with_published("runner/live0-reuse".to_string(), first_client);
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
}
