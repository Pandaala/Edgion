use super::*;
use crate::conf_mgr::conf_center::traits::LeadershipStatus;

/// Build a `KubernetesCenter` without touching a cluster. Only the
/// leadership signal is exercised, which never talks to the apiserver.
pub(super) fn unconnected_center() -> KubernetesCenter {
    let config = KubernetesConfig::default();
    let runtime = Arc::new(ControllerRuntime::new());
    runtime.set_single_publisher(config.ha_mode() != HaMode::AllServe);
    KubernetesCenter {
        controller_name: "edgion.io/gateway-controller".into(),
        config,
        writer: KubernetesStorage::new_unconnected_for_test(),
        config_sync_server: RwLock::new(None),
        shutdown_handle: Mutex::new(None),
        reload_tx: Mutex::new(None),
        runtime,
        leader_flag: Arc::new(watch::channel(false).0),
    }
}

#[tokio::test]
async fn controller_epochs_receive_the_center_selected_reference_grant_transport() {
    use super::super::version_detection::{GatewayClassVersion, GatewayVersion, ReferenceGrantVersion};
    let center = unconnected_center();
    let selected = center.writer.selection();
    selected.set(ReferenceGrantVersion::V1Beta1).unwrap();
    let selected_class = center.writer.gateway_class_selection();
    selected_class.set(GatewayClassVersion::V1Beta1).unwrap();
    let selected_gateway = center.writer.gateway_selection();
    selected_gateway.set(GatewayVersion::V1Beta1).unwrap();
    let client = Client::new(
        tower::service_fn(|_request: http::Request<kube::client::Body>| async {
            Ok::<_, std::convert::Infallible>(http::Response::new(kube::client::Body::empty()))
        }),
        "default",
    );
    for _ in 0..2 {
        let version = selected.get().copied();
        let controller = center
            .create_k8s_controller(
                &client,
                version,
                selected_class.get().copied(),
                selected_gateway.get().copied(),
                None,
                HashMap::new(),
            )
            .unwrap();
        assert_eq!(controller.reference_grant_version, Some(ReferenceGrantVersion::V1Beta1));
        assert_eq!(controller.gateway_class_version, Some(GatewayClassVersion::V1Beta1));
        assert_eq!(controller.gateway_version, Some(GatewayVersion::V1Beta1));
    }
    assert_eq!(
        center.writer.selected_reference_grant_version().await.unwrap(),
        ReferenceGrantVersion::V1Beta1
    );
    assert_eq!(
        center.writer.selected_gateway_class_version().await.unwrap(),
        GatewayClassVersion::V1Beta1
    );
    assert_eq!(
        center.writer.selected_gateway_version().await.unwrap(),
        GatewayVersion::V1Beta1
    );
}

#[tokio::test]
async fn epoch_stop_withdraws_publication_before_blocked_descendant_drain() {
    let center = unconnected_center();
    center.set_config_sync_server(Some(Arc::new(ConfigSyncServer::new())));
    let watchers = WatcherSet::new(CancellationToken::new());
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
    watchers.tracker.spawn(async move {
        let _ = finish_rx.await;
    });
    let cleanup = center.stop_resource_epoch(watchers);
    futures::pin_mut!(cleanup);
    assert!(futures::poll!(&mut cleanup).is_pending());
    assert!(center.config_sync_server.read().is_none());
    finish_tx.send(()).unwrap();
    cleanup.await;
}

/// The two leadership signals deliberately DISAGREE in Kubernetes mode.
///
/// `leadership_status()` must stay `NotApplicable` — it drives the Admin API
/// not-leader 409 gate, and Kubernetes routes admin traffic to the leader
/// via the `edgion.io/leader` pod-label Service selector while `all-serve`
/// followers must keep serving reads. `leader_signal()` must nevertheless be
/// `Some`, because the federation client is a process-wide singleton keyed
/// by a replica-invariant `controller_id`.
///
/// "Unifying" the two would either 409 every follower request or let every
/// replica fight over the federation session. This test exists to make that
/// mistake fail loudly.
#[tokio::test]
async fn leadership_status_is_not_applicable_while_leader_signal_is_some() {
    let center = unconnected_center();
    assert_eq!(
        center.leadership_status(),
        LeadershipStatus::NotApplicable,
        "K8s must keep the Admin API not-leader gate disabled"
    );
    assert!(
        center.leader_signal().is_some(),
        "K8s must still expose a live leadership signal for the federation client"
    );
}

/// The signal exists from `new()` onward and tracks the shared sender, so
/// the federation supervisor can never subscribe before the signal exists.
#[tokio::test]
async fn leader_signal_tracks_the_shared_flag() {
    let center = unconnected_center();
    let mut rx = center.leader_signal().expect("signal present at construction");
    assert!(!*rx.borrow_and_update(), "a fresh center starts as a follower");

    center.leader_flag.send_replace(true);
    assert!(*rx.borrow(), "promotion must be visible to subscribers");
}
