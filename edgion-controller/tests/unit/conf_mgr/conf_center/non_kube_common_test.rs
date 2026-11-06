use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::time::Duration;

use crate::conf_mgr::sync_runtime::ShutdownHandle;

use super::CenterBase;

#[test]
fn local_controller_phase_definitions_put_config_data_before_gateway() {
    let mut providers = Vec::new();
    macro_rules! collect_provider {
        ($K:ty, $handler:expr) => {
            providers.push(stringify!($K));
        };
    }
    for_each_provider_core_resource!(collect_provider, "edgion.io/gateway-controller");
    providers.push("EndpointSlice");

    let mut dependents = Vec::new();
    macro_rules! collect_dependent {
        ($K:ty, $handler:expr) => {
            dependents.push(stringify!($K));
        };
    }
    for_each_dependent_resource!(collect_dependent, "edgion.io/gateway-controller");

    assert!(providers.contains(&"EdgionConfigData"));
    assert!(!providers.contains(&"Gateway"));
    assert!(!dependents.contains(&"EdgionConfigData"));
    assert!(!dependents.contains(&"Gateway"));
}

struct LifecycleHandler;

#[async_trait::async_trait]
impl super::ProcessorHandler<edgion_resources::HTTPRoute> for LifecycleHandler {}

fn register_ready_processor(runtime: &Arc<crate::conf_mgr::ControllerRuntime>) {
    let processor = Arc::new(super::ResourceProcessor::<edgion_resources::HTTPRoute>::new(
        "HTTPRoute",
        10,
        Arc::new(LifecycleHandler),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    processor.on_init_done();
    runtime.processor_registry.mark_registration_complete();
}

fn assert_generation_withdrawn(base: &CenterBase) {
    assert!(!base.is_ready());
    assert!(base.config_sync_server().is_none());
    assert!(base.controller_handle.lock().is_none());
    assert!(!base.runtime.processor_registry.is_registration_complete());
}

#[test]
fn filesystem_none_leadership_runs_immediately() {
    let _guard = crate::services::credential_manager::lock_test_service();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let base = CenterBase::default();
            let generations = Cell::new(0);
            let sd = ShutdownHandle::new();
            let lifecycle =
                base.run_lifecycle_gated(sd.clone(), "test", None, |mut shutdown, runtime, error_tx, _scope| {
                    generations.set(generations.get() + 1);
                    register_ready_processor(&runtime);
                    tokio::spawn(async move {
                        let _error_tx = error_tx;
                        shutdown.wait().await;
                    })
                });
            tokio::pin!(lifecycle);
            assert!(futures::poll!(&mut lifecycle).is_pending());
            assert_eq!(generations.get(), 1, "None leadership must not gate");
            assert!(base.is_ready());
            sd.shutdown();
            lifecycle.await.unwrap();
            assert_generation_withdrawn(&base);
        });
}

#[test]
fn backend_completion_withdraws_ready_generation_before_waiting_for_shutdown() {
    let _guard = crate::services::credential_manager::lock_test_service();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            // Closed channel and explicitly reported failure share cleanup-before-wait.
            // When orderly shutdown is already requested, closure remains normal shutdown.
            for (reported_error, orderly_shutdown) in [(false, false), (true, false), (false, true)] {
                let base = CenterBase::default();
                let sd = ShutdownHandle::new();
                let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
                let (finished_tx, finished_rx) = tokio::sync::oneshot::channel::<()>();
                let controls = RefCell::new(Some((finish_rx, finished_tx)));
                let iteration = RefCell::new(None);
                let lifecycle =
                    base.run_lifecycle_gated(sd.clone(), "test", None, |shutdown, runtime, error_tx, _scope| {
                        *iteration.borrow_mut() = Some(shutdown);
                        register_ready_processor(&runtime);
                        let (finish_rx, finished_tx) = controls.borrow_mut().take().unwrap();
                        tokio::spawn(async move {
                            finish_rx.await.unwrap();
                            if reported_error {
                                error_tx.send("synthetic backend failure".to_string()).unwrap();
                            } else {
                                drop(error_tx);
                            }
                            finished_tx.send(()).unwrap();
                        })
                    });
                tokio::pin!(lifecycle);
                assert!(futures::poll!(&mut lifecycle).is_pending());
                assert!(base.is_ready());
                assert!(base.config_sync_server().is_some());
                if orderly_shutdown {
                    sd.shutdown();
                }
                finish_tx.send(()).unwrap();
                tokio::time::timeout(Duration::from_secs(2), finished_rx)
                    .await
                    .unwrap()
                    .unwrap();
                if orderly_shutdown {
                    lifecycle.await.unwrap();
                } else {
                    assert!(futures::poll!(&mut lifecycle).is_pending());
                    assert!(!sd.is_shutdown());
                    assert_generation_withdrawn(&base);
                    sd.shutdown();
                    lifecycle.await.unwrap();
                }
                assert_generation_withdrawn(&base);
                assert!(iteration.borrow().as_ref().unwrap().is_shutdown());
            }
        });
}

#[test]
fn shutdown_or_initialization_failure_drains_descendants_before_reset() {
    use crate::conf_mgr::conf_center::etcd::leadership_gate::{LeaderHandle, LeaderState};
    let _guard = crate::services::credential_manager::lock_test_service();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            for (initialize, backend_failure, leadership_transition) in [
                (false, false, false),
                (true, false, false),
                (false, true, false),
                (true, false, true),
            ] {
                let base = CenterBase::default();
                let shutdown = ShutdownHandle::new();
                let leader = LeaderState::new(Duration::from_secs(30));
                leader.set_leader(true);
                let started = Arc::new(tokio::sync::Notify::new());
                let draining = Arc::new(tokio::sync::Notify::new());
                let release = Arc::new(tokio::sync::Semaphore::new(0));
                let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let generations = Cell::new(0);
                let lifecycle = base.run_lifecycle_gated(
                    shutdown.clone(),
                    "drain-test",
                    leadership_transition.then(|| LeaderHandle::from_state(leader.clone())),
                    |mut signal, runtime, error_tx, scope| {
                        generations.set(generations.get() + 1);
                        register_ready_processor(&runtime);
                        if !initialize {
                            // Leave an incomplete registration to exercise the
                            // initialization wait rather than the serving wait.
                            runtime.processor_registry.clear_processors();
                        }
                        let started = started.clone();
                        let draining = draining.clone();
                        let release = release.clone();
                        let dropped = dropped.clone();
                        let cancel = scope.cancel.clone();
                        // Deliberately delay one descendant's completion after
                        // cancellation. The center must wait, not merely signal.
                        scope.tracker.spawn(async move {
                            cancel.cancelled().await;
                            draining.notify_one();
                            let _permit = release.acquire().await.unwrap();
                            dropped.store(true, std::sync::atomic::Ordering::Release);
                        });
                        tokio::spawn(async move {
                            started.notify_one();
                            if backend_failure {
                                error_tx.send("synthetic initialization failure".to_owned()).unwrap();
                            } else {
                                let _error_tx = error_tx;
                                signal.wait().await;
                            }
                        })
                    },
                );
                let driver = async {
                    started.notified().await;
                    if leadership_transition {
                        leader.set_leader(false);
                    } else if !backend_failure {
                        shutdown.shutdown();
                    }
                    draining.notified().await;
                    assert!(!base.is_ready(), "withdraw readiness before waiting for descendants");
                    assert_eq!(
                        base.runtime.processor_registry.is_registration_complete(),
                        initialize,
                        "reset must wait until all descendants finish"
                    );
                    assert!(!dropped.load(std::sync::atomic::Ordering::Acquire));
                    if leadership_transition {
                        // Leadership returning during drain cannot override a
                        // process shutdown and launch another generation.
                        shutdown.shutdown();
                        leader.set_leader(true);
                    }
                    release.add_permits(1);
                };
                let (result, ()) =
                    tokio::time::timeout(Duration::from_secs(2), async { tokio::join!(lifecycle, driver) })
                        .await
                        .unwrap();
                assert_eq!(result.is_err(), backend_failure);
                assert!(dropped.load(std::sync::atomic::Ordering::Acquire));
                assert_eq!(generations.get(), 1, "shutdown cannot start a successor");
                assert_generation_withdrawn(&base);
            }
        });
}
