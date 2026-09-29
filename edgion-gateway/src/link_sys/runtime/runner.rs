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

    pub fn post_not_ready(&self, forced: bool) {
        self.desired.send_modify(|desired| {
            desired.gen = desired.gen.saturating_add(1);
            if forced {
                desired.force_gen = desired.gen;
            }
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
    pub(crate) fn desired_link_for_test(&self) -> Option<Arc<LinkSys>> {
        match &self.desired.borrow().state {
            DesiredState::Ready { link } => Some(Arc::clone(link)),
            DesiredState::NotReady => None,
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
        // Global TLS changes may revoke the material held by the published
        // generation. Retire it before fingerprinting or building a replacement,
        // including when the latest resource is not ready. Ordinary updates
        // retain the existing handover behavior.
        if snapshot.force_gen > fulfilled_force_gen {
            if let Some(live) = runner.current() {
                replace_now::<O>(&runner, live).await;
            }
            join_retire::<O>(&runner).await;
            if shutdown.is_cancelled() {
                finish_delete::<O>(&runner).await;
                return;
            }
        }
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
#[path = "../../../tests/unit/link_sys/runtime/runner_test.rs"]
mod tests;
