use std::sync::atomic::{AtomicBool, Ordering};
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

#[tokio::test]
async fn filesystem_none_leadership_runs_immediately() {
    // With leadership=None the gate must not block; the make_handle closure runs at once.
    let base = CenterBase::new();
    let ran = Arc::new(AtomicBool::new(false));
    let r = ran.clone();
    let sd = ShutdownHandle::new();
    let h = {
        let sd2 = sd.clone();
        tokio::spawn(async move {
            base.run_lifecycle_gated(sd2, "test", None, move |shutdown, _rt, _err| {
                r.store(true, Ordering::Release);
                tokio::spawn(async move {
                    let mut s = shutdown;
                    s.wait().await;
                })
            })
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(ran.load(Ordering::Acquire), "None leadership must not gate");
    sd.shutdown();
    let _ = h.await;
}
