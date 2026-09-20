//! Per-`namespace/name` persistent owner for long-lived LinkSys clients.
//!
//! The dispatcher posts [`DesiredState`]; this task is the sole publisher of the
//! small handle and owns connect backoff, handover retire, and delete.

use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use edgion_common::link_sys_core::PublishedHealth;
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
    /// Latest forced post; ordinary replays must not erase it before publication.
    pub force_gen: u64,
    pub state: DesiredState,
}

#[derive(Clone)]
pub enum DesiredState {
    Ready { link: Arc<LinkSys> },
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
    fn published_health(_client: &Self::Client) -> Option<&PublishedHealth> {
        None
    }
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
    /// Serializes publication with delete; desired's watch guard serializes posts with publication.
    publication: Mutex<()>,
    join: Mutex<Option<JoinHandle<()>>>,
    /// Generations unpublished by delete; the loop must quit all of them.
    orphan: Mutex<Vec<Arc<O::Client>>>,
    /// Retiring generation task. Delete joins it before the runner exits.
    retire: Mutex<Option<RetireTask>>,
    #[cfg(test)]
    publish_entry: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

struct RetireTask {
    abort: Option<oneshot::Sender<()>>,
    join: JoinHandle<()>,
}

impl<O: ApplyOwner> PersistentRunner<O> {
    pub fn spawn(key: String) -> Arc<Self> {
        let (tx, rx) = watch::channel(Desired {
            gen: 0,
            force_gen: 0,
            state: DesiredState::NotReady,
        });
        let shutdown = CancellationToken::new();
        let runner = Arc::new(Self {
            key,
            desired: tx,
            current: ArcSwap::from_pointee(None),
            shutdown: shutdown.clone(),
            publication: Mutex::new(()),
            join: Mutex::new(None),
            orphan: Mutex::new(Vec::new()),
            retire: Mutex::new(None),
            #[cfg(test)]
            publish_entry: Mutex::new(None),
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
            if forced {
                desired.force_gen = desired.gen;
            }
            desired.state = DesiredState::Ready { link };
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
        {
            let _publication = self.publication.lock();
            self.shutdown.cancel();
            self.unpublish_into_orphan();
        }
        self.abort_retire();
    }

    fn unpublish_into_orphan(&self) {
        let Some(current) = (*self.current.load_full()).clone() else {
            return;
        };
        let removed = PublishedHealth::transition_publication(O::published_health(&current), None, || {
            self.current.swap(Arc::new(None))
        });
        if let Some(client) = (*removed).clone() {
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
        let published = Arc::clone(&client);
        PublishedHealth::transition_publication(None, O::published_health(&client), || {
            runner.current.store(Arc::new(Some(published)));
        });
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
    // A force is fulfilled only after a generation built for it is published.
    let mut fulfilled_force_gen = 0;
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
            DesiredState::Ready { link } => {
                let start_gen = snapshot.gen;
                let forced = snapshot.force_gen > fulfilled_force_gen;
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
                        fulfilled_force_gen = snapshot.force_gen;
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
                let outcome = publish::<O>(runner, &client, start_gen);
                if !matches!(outcome, LoopExit::Continue) {
                    O::mark_retired(&client);
                    O::shutdown(client).await;
                }
                return outcome;
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

fn publish<O: ApplyOwner>(runner: &PersistentRunner<O>, client: &Arc<O::Client>, start_gen: u64) -> LoopExit {
    #[cfg(test)]
    if let Some(hook) = runner.publish_entry.lock().take() {
        hook();
    }
    publish_with_hooks(runner, client, start_gen, || {}, || {})
}

fn publish_with_hooks<O: ApplyOwner>(
    runner: &PersistentRunner<O>,
    client: &Arc<O::Client>,
    start_gen: u64,
    before_swap: impl FnOnce(),
    after_swap: impl FnOnce(),
) -> LoopExit {
    let _publication = runner.publication.lock();
    let desired = runner.desired.borrow();
    if runner.shutdown.is_cancelled() {
        return LoopExit::Shutdown;
    }
    if desired.gen != start_gen {
        return LoopExit::Aborted;
    }
    let previous = (*runner.current.load_full()).clone();
    before_swap();
    let replaced = PublishedHealth::transition_publication(
        previous.as_deref().and_then(O::published_health),
        O::published_health(client),
        || runner.current.swap(Arc::new(Some(client.clone()))),
    );
    after_swap();
    if let Some(old) = (*replaced).clone() {
        O::mark_retired(&old);
        start_retire::<O>(runner, old);
    }
    LoopExit::Continue
}

fn push_orphan<O: ApplyOwner>(runner: &PersistentRunner<O>, client: Arc<O::Client>) {
    O::mark_retired(&client);
    runner.orphan.lock().push(client);
}

async fn replace_now<O: ApplyOwner>(runner: &PersistentRunner<O>, live: Arc<O::Client>) {
    {
        let _publication = runner.publication.lock();
        let loaded = runner.current.load_full();
        if loaded.as_ref().as_ref().is_some_and(|c| Arc::ptr_eq(c, &live)) {
            PublishedHealth::transition_publication(O::published_health(&live), None, || {
                runner.current.store(Arc::new(None));
            });
        }
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
                FAKE_BUILD_FAILS.fetch_add(1, Ordering::Relaxed);
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
        let runner =
            PersistentRunner::<FakeOwner>::spawn_with_published("runner/stale-publish".to_string(), first.clone());
        let supersede = runner.clone();
        *runner.publish_entry.lock() = Some(Box::new(move || supersede.post_not_ready()));

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
                    posting_runner.post_not_ready();
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
