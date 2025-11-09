use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::{interval, sleep};

use edgion::core::conf_sync::config_client::ConfigClient;
use edgion::core::conf_sync::grpc_client::ConfigSyncClient;

const GRPC_ADDR: &str = "http://127.0.0.1:50051";
const GATEWAY_CLASS_KEY: &str = "demo";

#[derive(Clone, Copy)]
pub enum RunMode {
    UntilCtrlC,
    For(Duration),
}

pub async fn run_config_sync_client(mode: RunMode) -> anyhow::Result<()> {
    println!("[CLIENT] Starting Config Sync client example");

    let mut client =
        ConfigSyncClient::connect(GRPC_ADDR.to_string(), GATEWAY_CLASS_KEY.to_string()).await?;

    println!("[CLIENT] Connected to {}", GRPC_ADDR);

    client.sync_all().await?;
    client.start_watch_all().await?;

    let hub = client.get_config_client();
    spawn_status_logger(hub.clone());

    match mode {
        RunMode::UntilCtrlC => {
            println!("[CLIENT] Running... press Ctrl+C to exit");
            tokio::signal::ctrl_c().await?;
        }
        RunMode::For(duration) => {
            println!("[CLIENT] Running for {:?}", duration);
            sleep(duration).await;
        }
    }

    println!("[CLIENT] Shutdown signal received");

    Ok(())
}

fn spawn_status_logger(hub: Arc<Mutex<ConfigClient>>) {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(10));
        loop {
            ticker.tick().await;
            let hub_guard = hub.lock().await;
            log_hub_summary(&hub_guard);
        }
    });
}

fn log_hub_summary(hub: &ConfigClient) {
    let gc = hub.list_gateway_classes();
    let specs = hub.list_edgion_gateway_config();
    let gateways = hub.list_gateways();
    let routes = hub.list_routes();
    let services = hub.list_services();
    let endpoint_slices = hub.list_endpoint_slices();
    let tls = hub.list_edgion_tls();
    let secrets = hub.list_secrets();

    println!(
        "[CLIENT] Summary key={} GatewayClass={{count:{},version:{}}} GatewayClassSpec={{count:{},version:{}}} Gateway={{count:{},version:{}}} HTTPRoute={{count:{},version:{}}} Service={{count:{},version:{}}} EndpointSlice={{count:{},version:{}}} EdgionTls={{count:{},version:{}}} Secret={{count:{},version:{}}}",
        GATEWAY_CLASS_KEY,
        gc.data.len(),
        gc.resource_version,
        specs.data.len(),
        specs.resource_version,
        gateways.data.len(),
        gateways.resource_version,
        routes.data.len(),
        routes.resource_version,
        services.data.len(),
        services.resource_version,
        endpoint_slices.data.len(),
        endpoint_slices.resource_version,
        tls.data.len(),
        tls.resource_version,
        secrets.data.len(),
        secrets.resource_version,
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual integration harness; requires running Config Sync server"]
async fn config_sync_client_manual() -> anyhow::Result<()> {
    run_config_sync_client(RunMode::UntilCtrlC).await
}
