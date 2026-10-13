use super::*;
use edgion_resources::ResourceKind;

fn consumer(name: &str, generation: i64) -> DesiredCredentialConsumer {
    DesiredCredentialConsumer {
        reference: crate::reference_test_support::injector_reference(&crate::reference_test_support::injector_owner(
            "apps",
            name,
            &format!("uid-{name}"),
            generation,
            "ops/source",
        )),
        resource: crate::conf_mgr::sync_runtime::resource_processor::ResourceRef::new(
            ResourceKind::EdgionPlugins,
            Some("apps".to_string()),
            name.to_string(),
        ),
        uid: format!("uid-{name}"),
        generation,
        source_key: "ops/source".to_string(),
        source_uid: "source-uid".to_string(),
        source_generation: 2,
        authorization_revision: "grant-rv".to_string(),
    }
}

#[test]
fn consumer_set_revision_is_order_independent_and_change_sensitive() {
    let a = consumer("a", 1);
    let b = consumer("b", 1);
    assert_eq!(
        consumer_revision(&[a.clone(), b.clone()], 0),
        consumer_revision(&[b.clone(), a.clone()], 0)
    );
    assert_ne!(
        consumer_revision(&[a, b.clone()], 0),
        consumer_revision(&[consumer("a", 2), b], 0)
    );
    assert_ne!(consumer_revision(&[], 1), consumer_revision(&[], 2));
}

#[test]
fn binding_revision_covers_consumer_and_source_generations() {
    let original = consumer("a", 1);
    let mut changed = original.clone();
    changed.source_generation += 1;
    assert_ne!(original.binding_revision(), changed.binding_revision());
}

#[test]
fn recovery_never_restarts_rotation_interval_from_failover_time() {
    let issued = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let valid_until = issued + Duration::from_secs(3_570);
    let now_wall = issued + Duration::from_secs(15 * 60);
    let now_mono = Instant::now();
    let refresh = recovered_refresh_at(
        issued,
        valid_until,
        Duration::from_secs(15 * 60),
        "source/publication",
        now_wall,
        now_mono,
    );
    assert_eq!(refresh, now_mono, "passed periodic deadline must refresh immediately");
}

mod observer_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    #[derive(Default)]
    struct RecordingGauge {
        registrations: AtomicUsize,
        value: AtomicU64,
        pause: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
    }

    impl metrics::GaugeFn for RecordingGauge {
        fn increment(&self, _: f64) {
            panic!("unexpected gauge increment");
        }
        fn decrement(&self, _: f64) {
            panic!("unexpected gauge decrement");
        }
        fn set(&self, value: f64) {
            if value == 0.0 {
                let pause = self.pause.lock().take();
                if let Some((entered, release)) = pause {
                    entered.send(()).unwrap();
                    release.recv_timeout(Duration::from_secs(5)).unwrap();
                }
            }
            self.value.store(value.to_bits(), Ordering::SeqCst);
        }
    }

    struct Recorder(Arc<RecordingGauge>);

    impl metrics::Recorder for Recorder {
        fn describe_counter(&self, _: metrics::KeyName, _: Option<metrics::Unit>, _: metrics::SharedString) {}
        fn describe_gauge(&self, _: metrics::KeyName, _: Option<metrics::Unit>, _: metrics::SharedString) {}
        fn describe_histogram(&self, _: metrics::KeyName, _: Option<metrics::Unit>, _: metrics::SharedString) {}
        fn register_counter(&self, _: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Counter {
            metrics::Counter::noop()
        }
        fn register_histogram(&self, _: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Histogram {
            metrics::Histogram::noop()
        }
        fn register_gauge(&self, key: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Gauge {
            assert_eq!(key.name(), "edgion_controller_leader_job_alive");
            assert_eq!(
                key.labels()
                    .map(|label| (label.key(), label.value()))
                    .collect::<Vec<_>>(),
                [("leader_job", "credential_manager")]
            );
            self.0.registrations.fetch_add(1, Ordering::SeqCst);
            metrics::Gauge::from_arc(self.0.clone())
        }
    }

    #[test]
    fn current_death_publishes_zero_but_absent_and_stale_observers_register_nothing() {
        let _guard = lock_test_service();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        stop_credential_manager_service();
        let state = Arc::new(RecordingGauge::default());
        metrics::with_local_recorder(&Recorder(state.clone()), || {
            assert!(!confirm_dead_current(0));
            assert_eq!(state.registrations.load(Ordering::SeqCst), 0);
            let current = install_test_generation();
            assert!(!confirm_dead_current(current + 1));
            assert_eq!(state.registrations.load(Ordering::SeqCst), 0);
            crate::conf_mgr::sync_runtime::metrics::leader_job_gauge("credential_manager").set(1.0);
            assert!(confirm_dead_current(current));
            assert_eq!(state.registrations.load(Ordering::SeqCst), 2);
            assert_eq!(f64::from_bits(state.value.load(Ordering::SeqCst)), 0.0);
        });
        stop_credential_manager_service();
    }

    #[test]
    fn service_replacement_cannot_enter_between_death_check_and_gauge_write() {
        let _guard = lock_test_service();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        stop_credential_manager_service();
        let old = install_test_generation();
        let state = Arc::new(RecordingGauge::default());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        *state.pause.lock() = Some((entered_tx, release_rx));
        let observer_state = state.clone();
        let observer = std::thread::spawn(move || {
            metrics::with_local_recorder(&Recorder(observer_state), || confirm_dead_current(old))
        });
        let reached_write = entered_rx.recv_timeout(Duration::from_secs(5));
        let lock_held = SERVICE.try_lock().is_none();
        let replacement_state = state.clone();
        let handle = runtime.handle().clone();
        let (attempt_tx, attempt_rx) = mpsc::channel();
        let replacement = std::thread::spawn(move || {
            let _entered = handle.enter();
            attempt_tx.send(()).unwrap();
            metrics::with_local_recorder(&Recorder(replacement_state), || {
                let current = install_test_generation();
                crate::conf_mgr::sync_runtime::metrics::leader_job_gauge("credential_manager").set(1.0);
                current
            })
        });
        let attempted = attempt_rx.recv_timeout(Duration::from_secs(5));
        // Always release the paused setter before assertions or joining workers.
        let released = release_tx.send(());
        let observer_result = observer.join();
        let replacement_result = replacement.join();
        assert!(reached_write.is_ok());
        assert!(attempted.is_ok());
        assert!(released.is_ok());
        assert!(observer_result.unwrap());
        assert!(lock_held, "replacement must not acquire SERVICE during the zero write");
        assert_ne!(replacement_result.unwrap(), old);
        assert_eq!(f64::from_bits(state.value.load(Ordering::SeqCst)), 1.0);
        let registrations = state.registrations.load(Ordering::SeqCst);
        metrics::with_local_recorder(&Recorder(state.clone()), || assert!(!confirm_dead_current(old)));
        assert_eq!(state.registrations.load(Ordering::SeqCst), registrations);
        assert_eq!(f64::from_bits(state.value.load(Ordering::SeqCst)), 1.0);
        stop_credential_manager_service();
    }
}
