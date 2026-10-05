use super::*;
fn config() -> EtcdClientConfig {
    serde_json::from_value(serde_json::json!({"endpoints":["http://localhost:2379"]})).unwrap()
}
#[test]
fn disabled_metrics_do_not_install_completion_observation() {
    let observer = GatewayEtcdObserver::new("observation/disabled", &config()).unwrap();
    assert!(!observer.observes_completions());
    assert!(!observer.logging_enabled);
}
#[test]
fn optional_defaults_log_lifecycle_but_not_successful_probes() {
    let mut cfg = config();
    cfg.observability = Some(serde_json::from_value(serde_json::json!({"logging":{"enabled":true}})).unwrap());
    let observer = GatewayEtcdObserver::new("observation/logging", &cfg).unwrap();
    assert_eq!(observer.level, tracing::Level::INFO);
    let mut event = MaintenanceEvent {
        kind: MaintenanceKind::Probe,
        outcome: MaintenanceOutcome::Ok,
        elapsed: Duration::from_secs(10),
        health_changed: None,
    };
    assert!(!observer.optional_event(event));
    event.kind = MaintenanceKind::Init;
    assert!(observer.optional_event(event));
    observer.deactivate();
    observer.maintenance(event);
    assert!(!*observer.active.lock());
    assert_eq!(observer.last_optional.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[test]
fn retirement_fences_late_completion_and_preserves_existing_count() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    metrics::with_local_recorder(&recorder, || {
        let mut cfg = config();
        cfg.observability = Some(serde_json::from_value(serde_json::json!({"metrics":{"enabled":true}})).unwrap());
        let observer = GatewayEtcdObserver::new("observation/retired", &cfg).unwrap();
        observer.completed(Operation::Kv, Outcome::Ok);
        observer.deactivate();
        observer.deactivate();
        observer.completed(Operation::Kv, Outcome::Ok);
        observer.completed(Operation::Kv, Outcome::Transport);
        let rendered = recorder.handle().render();
        let line = rendered
            .lines()
            .find(|line| {
                line.contains("link=\"observation/retired\"")
                    && line.contains("operation=\"kv\"")
                    && line.contains("outcome=\"ok\"")
            })
            .unwrap();
        assert!(line.ends_with(" 1"));
        assert!(!rendered
            .lines()
            .any(|line| line.contains("link=\"observation/retired\"") && line.contains("outcome=\"transport\"")));
    });
}

#[test]
fn invalid_link_labels_are_rejected_before_metric_registration() {
    let mut cfg = config();
    cfg.observability = Some(serde_json::from_value(serde_json::json!({"metrics":{"enabled":true}})).unwrap());
    for key in [
        "missing-slash",
        "ns/",
        "/name",
        "ns/a/b",
        "ns/UPPER",
        "bad namespace/name",
    ] {
        assert!(GatewayEtcdObserver::new(key, &cfg).is_err());
    }
}

#[test]
fn concurrent_completion_after_deactivate_returns_cannot_record() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let observer = metrics::with_local_recorder(&recorder, || {
        let mut cfg = config();
        cfg.observability = Some(serde_json::from_value(serde_json::json!({"metrics":{"enabled":true}})).unwrap());
        GatewayEtcdObserver::new("observation/concurrent", &cfg).unwrap()
    });
    let boundary = Arc::new(std::sync::Barrier::new(2));
    let late = observer.clone();
    let other_boundary = boundary.clone();
    let worker = std::thread::spawn(move || {
        // This owner already exists before retirement. The barrier releases it
        // only after deactivate has returned on the owning thread.
        other_boundary.wait();
        late.completed(Operation::Kv, Outcome::Ok);
    });
    observer.completed(Operation::Kv, Outcome::Ok);
    observer.deactivate();
    boundary.wait();
    worker.join().unwrap();
    let rendered = recorder.handle().render();
    let line = rendered
        .lines()
        .find(|line| {
            line.contains("link=\"observation/concurrent\"")
                && line.contains("operation=\"kv\"")
                && line.contains("outcome=\"ok\"")
        })
        .unwrap();
    assert!(line.ends_with(" 1"));
}

#[test]
fn optional_logging_controls_actual_maintenance_events_without_masking_health() {
    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let output = Arc::new(Mutex::new(Vec::new()));
    let writer = Capture(output.clone());
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let disabled = GatewayEtcdObserver::new("observation/mandatory", &config()).unwrap();
        disabled.maintenance(MaintenanceEvent {
            kind: MaintenanceKind::Probe,
            outcome: MaintenanceOutcome::Failed,
            elapsed: Duration::ZERO,
            health_changed: Some(false),
        });
        disabled.completed(Operation::Kv, Outcome::Protocol);
        let mut cfg = config();
        cfg.observability = Some(
            serde_json::from_value(
                serde_json::json!({"logging":{"enabled":true,"level":"debug","slowLogThreshold":10}}),
            )
            .unwrap(),
        );
        let enabled = GatewayEtcdObserver::new("observation/slow", &cfg).unwrap();
        enabled.maintenance(MaintenanceEvent {
            kind: MaintenanceKind::Probe,
            outcome: MaintenanceOutcome::Ok,
            elapsed: Duration::from_millis(9),
            health_changed: None,
        });
        enabled.maintenance(MaintenanceEvent {
            kind: MaintenanceKind::Probe,
            outcome: MaintenanceOutcome::Ok,
            elapsed: Duration::from_millis(10),
            health_changed: None,
        });
        enabled.maintenance(MaintenanceEvent {
            kind: MaintenanceKind::Probe,
            outcome: MaintenanceOutcome::Ok,
            elapsed: Duration::from_millis(10),
            health_changed: None,
        });
        enabled.deactivate();
        enabled.deactivate();
    });
    let output = String::from_utf8(output.lock().clone()).unwrap();
    assert_eq!(output.matches("link-sys degraded").count(), 1);
    assert_eq!(output.matches("Etcd maintenance").count(), 2); // One slow summary and one retirement request.
    assert!(output.contains("DEBUG"));
    assert!(output.contains("retirement_requested=true"));
    assert!(!output.contains("protocol"));
}
