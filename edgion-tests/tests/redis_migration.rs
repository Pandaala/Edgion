//! Real Redis provider tests. These connect only to the Redis topology fixture,
//! never start another Gateway or Controller, and are explicitly opt-in.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use edgion_gateway::link_sys::providers::redis::RedisLinkClient;
use edgion_resources::resources::link_sys::LinkSys;
use serde_json::json;

/// Several scenarios fail over, stop nodes or move slots on the shared
/// fixture, so every scenario holds this lock regardless of `--test-threads`.
/// `tokio::sync::Mutex` does not poison, so one failed scenario cannot wedge
/// the rest.
static TOPOLOGY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn init_crypto() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(edgion_common::startup::init_crypto);
}

fn client(config: serde_json::Value) -> RedisLinkClient {
    let owner: LinkSys = serde_json::from_value(json!({
        "apiVersion": "edgion.io/v1", "kind": "LinkSys",
        "metadata": {"name": "redis-migration", "namespace": "test"},
        "spec": {"type": "redis", "config": config}
    }))
    .unwrap();
    RedisLinkClient::from_link_sys(Arc::new(owner)).unwrap()
}

async fn exercise(config: serde_json::Value) {
    init_crypto();
    let client = client(config);
    tokio::time::timeout(Duration::from_secs(15), client.init_for_test())
        .await
        .unwrap()
        .unwrap();
    assert!(client.healthy());
    let key = format!("{{migration}}:kv:{}", std::process::id());
    client.set(&key, "1", Some(Duration::from_secs(60))).await.unwrap();
    assert_eq!(client.get(&key).await.unwrap().as_deref(), Some("1"));
    assert_eq!(client.incr(&key).await.unwrap(), 2);
    let hash = format!("{key}:hash");
    client.hset(&hash, "field", "value").await.unwrap();
    assert_eq!(client.hgetall(&hash).await.unwrap().get("field").unwrap(), "value");
    let lock_key = format!("{key}:lock");
    let guard = client
        .try_lock(&lock_key, Duration::from_secs(10))
        .await
        .unwrap()
        .unwrap();
    assert!(client
        .try_lock(&lock_key, Duration::from_secs(10))
        .await
        .unwrap()
        .is_none());
    assert!(guard.unlock().await.unwrap());
    client.del(&[&key, &hash]).await.unwrap();
    client.shutdown().await.unwrap();
    client.shutdown().await.unwrap();
    assert!(!client.healthy());
}

#[tokio::test]
#[ignore = "requires docker-compose.topologies.yaml"]
async fn standalone_operations_and_shutdown() {
    let _serial = TOPOLOGY_LOCK.lock().await;
    exercise(json!({"endpoints": ["127.0.0.1:16420"], "pool": {"size": 2}})).await;
}

#[tokio::test]
#[ignore = "requires docker-compose.topologies.yaml"]
async fn cluster_operations_and_shutdown() {
    let _serial = TOPOLOGY_LOCK.lock().await;
    exercise(json!({"endpoints": ["127.0.0.1:16400"], "topology": {"mode": "cluster"}, "pool": {"size": 2}})).await;
}

#[tokio::test]
#[ignore = "requires docker-compose.topologies.yaml"]
async fn sentinel_operations_and_shutdown() {
    let _serial = TOPOLOGY_LOCK.lock().await;
    exercise(json!({"topology": {"mode": "sentinel", "sentinel": {
        "masterName": "edgion-primary", "sentinels": ["127.0.0.1:16410", "127.0.0.1:16411", "127.0.0.1:16412"]
    }}, "pool": {"size": 2}}))
    .await;
}

#[tokio::test]
#[ignore = "requires docker-compose.topologies.yaml"]
async fn executed_increment_with_lost_reply_is_not_replayed() {
    let _serial = TOPOLOGY_LOCK.lock().await;
    init_crypto();
    let direct = client(json!({"endpoints": ["127.0.0.1:16420"], "pool": {"size": 1}}));
    direct.init_for_test().await.unwrap();
    let key = format!("migration:lost-reply:{}", std::process::id());
    direct.set(&key, "0", Some(Duration::from_secs(60))).await.unwrap();
    let proxy = DropProxy::spawn("127.0.0.1:16420", b"INCR").await;
    let address = proxy.address;
    let submissions = proxy.submissions.clone();
    let proxied = client(json!({"endpoints": [address.to_string()], "pool": {"size": 1},
        "timeout": {"connect": "1s", "command": "1s"}}));
    proxied.init_for_test().await.unwrap();
    assert!(proxied.incr(&key).await.is_err());
    assert_eq!(direct.get(&key).await.unwrap().as_deref(), Some("1"));
    assert_eq!(submissions.load(Ordering::SeqCst), 1);
    proxied.shutdown().await.unwrap();
    direct.del(&[&key]).await.unwrap();
    direct.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires docker-compose.topologies.yaml"]
async fn executed_unlock_with_lost_reply_is_not_replayed() {
    let _serial = TOPOLOGY_LOCK.lock().await;
    init_crypto();
    let direct = client(json!({"endpoints": ["127.0.0.1:16420"], "pool": {"size": 1}}));
    direct.init_for_test().await.unwrap();
    let key = format!("migration:lost-lua:{}", std::process::id());
    let proxy = DropProxy::spawn("127.0.0.1:16420", b"EVAL").await;
    let submissions = proxy.submissions.clone();
    let proxied = client(json!({"endpoints": [proxy.address.to_string()], "pool": {"size": 1},
        "timeout": {"connect": "1s", "command": "1s"}}));
    proxied.init_for_test().await.unwrap();
    let guard = proxied.try_lock(&key, Duration::from_secs(30)).await.unwrap().unwrap();
    assert!(guard.unlock().await.is_err());
    assert_eq!(submissions.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(direct.get(&key).await.unwrap(), None);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if proxied.get(&key).await.is_ok() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "connection stayed unusable");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(submissions.load(std::sync::atomic::Ordering::SeqCst), 1);
    proxied.shutdown().await.unwrap();
    direct.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires docker-compose.topologies.yaml"]
async fn cross_slot_delete_never_partially_executes() {
    let _serial = TOPOLOGY_LOCK.lock().await;
    init_crypto();
    let client = client(json!({"endpoints": ["127.0.0.1:16400"], "topology": {"mode": "cluster"}}));
    client.init_for_test().await.unwrap();
    let first = "{migration-a}:cross-slot";
    let second = "{migration-b}:cross-slot";
    client
        .set(first, "present", Some(Duration::from_secs(60)))
        .await
        .unwrap();
    client
        .set(second, "present", Some(Duration::from_secs(60)))
        .await
        .unwrap();
    assert!(client.del(&[first, second]).await.is_err());
    assert_eq!(client.get(first).await.unwrap().as_deref(), Some("present"));
    assert_eq!(client.get(second).await.unwrap().as_deref(), Some("present"));
    client.del(&[first]).await.unwrap();
    client.del(&[second]).await.unwrap();
    client.shutdown().await.unwrap();
}

// Cargo runs integration tests from the package root, so anchor the fixture
// path there instead of relying on the caller's working directory.
const TOPOLOGY_COMPOSE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/integration/conf/Services/redis/docker-compose.topologies.yaml"
);

struct DropProxy {
    address: std::net::SocketAddr,
    submissions: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl DropProxy {
    async fn spawn(upstream: &'static str, marker: &'static [u8]) -> Self {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let submissions = Arc::new(AtomicUsize::new(0));
        let observed = submissions.clone();
        let task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                let (downstream, _) = listener.accept().await.unwrap();
                let observed = observed.clone();
                tasks.spawn(async move {
                    let upstream = TcpStream::connect(upstream).await.unwrap();
                    let (mut dr, mut dw) = downstream.into_split();
                    let (mut ur, mut uw) = upstream.into_split();
                    let drop_reply = AtomicBool::new(false);
                    let requests = async {
                        let mut pending = Vec::new();
                        let mut buf = [0u8; 4096];
                        loop {
                            let n = dr.read(&mut buf).await?;
                            if n == 0 {
                                break;
                            }
                            pending.extend_from_slice(&buf[..n]);
                            if pending.windows(marker.len()).any(|window| window == marker)
                                && !drop_reply.swap(true, Ordering::SeqCst)
                            {
                                observed.fetch_add(1, Ordering::SeqCst);
                            }
                            if pending.len() > marker.len().saturating_sub(1) {
                                pending.drain(..pending.len() - (marker.len() - 1));
                            }
                            uw.write_all(&buf[..n]).await?;
                        }
                        Ok::<_, std::io::Error>(())
                    };
                    let replies = async {
                        let mut buf = [0u8; 4096];
                        loop {
                            let n = ur.read(&mut buf).await?;
                            if n == 0 || drop_reply.load(Ordering::SeqCst) {
                                break;
                            }
                            dw.write_all(&buf[..n]).await?;
                        }
                        Ok::<_, std::io::Error>(())
                    };
                    tokio::select! { _ = requests => {}, _ = replies => {} }
                });
            }
        });
        Self {
            address,
            submissions,
            task,
        }
    }
}

impl Drop for DropProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn fixture_output(port: u16, args: &[&str]) -> std::process::Output {
    std::process::Command::new("docker")
        .args([
            "compose",
            "-p",
            "edgion-redis-migration",
            "-f",
            TOPOLOGY_COMPOSE,
            "exec",
            "-T",
            "redis-topologies",
            "redis-cli",
            "-e",
            "--raw",
            "-p",
            &port.to_string(),
        ])
        .args(args)
        .output()
        .unwrap()
}

fn fixture_command(port: u16, args: &[&str]) -> String {
    let output = fixture_output(port, args);
    assert!(
        output.status.success(),
        "fixture command failed: {}",
        fixture_text(&output)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn fixture_text(output: &std::process::Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

/// `redis-cli --raw` prints the Sentinel address as a host line and a port line.
fn sentinel_master(port: u16) -> Option<(String, u16)> {
    let output = fixture_output(port, &["SENTINEL", "get-master-addr-by-name", "edgion-primary"]);
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let mut lines = text.lines().filter(|line| !line.is_empty());
    let host = lines.next()?.to_owned();
    let master_port = lines.next()?.parse().ok()?;
    Some((host, master_port))
}

/// Sentinel rejects a failover (NOGOODSLAVE, INPROG) until it has discovered a
/// replica and settled the previous one, so retry within the caller's budget.
async fn request_sentinel_failover(deadline: tokio::time::Instant, context: &str) {
    loop {
        let reply = fixture_output(16410, &["SENTINEL", "failover", "edgion-primary"]);
        if reply.status.success() && String::from_utf8_lossy(&reply.stdout).contains("OK") {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{context}: {}",
            fixture_text(&reply)
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn parse_masters(nodes: &str) -> Vec<(u16, String)> {
    let mut masters = Vec::new();
    for line in nodes.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let flags: Vec<_> = parts[2].split(',').collect();
        if !flags.contains(&"master") || flags.iter().any(|flag| flag.starts_with("fail")) {
            continue;
        }
        let endpoint = parts[1].split('@').next().unwrap_or(parts[1]);
        let Some(port) = endpoint.rsplit(':').next().and_then(|port| port.parse().ok()) else {
            continue;
        };
        masters.push((port, parts[0].to_owned()));
    }
    masters
}

fn slot_owner(nodes: &str, slot: u16) -> Option<(u16, String)> {
    for line in nodes.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() < 9 || !parts[2].split(',').any(|flag| flag == "master") {
            continue;
        }
        for token in &parts[8..] {
            let token = token.split('[').next().unwrap_or(token);
            let owns = if let Some((start, end)) = token.split_once('-') {
                let Ok(start) = start.parse::<u16>() else { continue };
                let Ok(end) = end.parse::<u16>() else { continue };
                slot >= start && slot <= end
            } else {
                token.parse::<u16>().ok() == Some(slot)
            };
            if owns {
                let endpoint = parts[1].split('@').next().unwrap_or(parts[1]);
                let port = endpoint.rsplit(':').next()?.parse().ok()?;
                return Some((port, parts[0].to_owned()));
            }
        }
    }
    None
}

fn restart_cluster_node(port: u16) {
    let port_text = port.to_string();
    let dir = format!("/tmp/redis-topologies/{port_text}");
    let status = std::process::Command::new("docker")
        .args([
            "compose",
            "-p",
            "edgion-redis-migration",
            "-f",
            TOPOLOGY_COMPOSE,
            "exec",
            "-T",
            "redis-topologies",
            "redis-server",
            "--port",
            &port_text,
            "--bind",
            "0.0.0.0",
            "--protected-mode",
            "no",
            "--cluster-enabled",
            "yes",
            "--cluster-config-file",
            "nodes.conf",
            "--cluster-node-timeout",
            "1000",
            "--cluster-announce-ip",
            "127.0.0.1",
            "--cluster-announce-port",
            &port_text,
            "--dir",
            &dir,
            "--save",
            "",
            "--appendonly",
            "no",
            "--logfile",
            &format!("{dir}/server.log"),
            "--daemonize",
            "yes",
        ])
        .status();
    if !status.as_ref().is_ok_and(|status| status.success()) {
        eprintln!("cluster node {port} restart command failed: {status:?}");
    }
}

struct ClusterNodeRestore(u16);

impl Drop for ClusterNodeRestore {
    fn drop(&mut self) {
        restart_cluster_node(self.0);
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            if fixture_output(self.0, &["PING"]).status.success()
                && fixture_output(self.0, &["CLUSTER", "INFO"])
                    .stdout
                    .windows(16)
                    .any(|window| window == b"cluster_state:ok")
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        // Later scenarios depend on this node; fail loudly unless already unwinding.
        let message = format!("cluster node {} did not rejoin after restart", self.0);
        if std::thread::panicking() {
            eprintln!("{message}");
        } else {
            panic!("{message}");
        }
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.topologies.yaml"]
async fn sentinel_failover_follows_the_new_primary() {
    let _serial = TOPOLOGY_LOCK.lock().await;
    init_crypto();
    let client = client(json!({"topology": {"mode": "sentinel", "sentinel": {
        "masterName": "edgion-primary", "sentinels": ["127.0.0.1:16410", "127.0.0.1:16411", "127.0.0.1:16412"]
    }}, "pool": {"size": 2}, "timeout": {"command": "1s", "connect": "1s"}}));
    client.init_for_test().await.unwrap();
    let original = sentinel_master(16410).expect("original Sentinel primary");
    request_sentinel_failover(
        tokio::time::Instant::now() + Duration::from_secs(30),
        "Sentinel failover was not admitted",
    )
    .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let current = sentinel_master(16410);
        let promoted = current.as_ref().is_some_and(|endpoint| endpoint != &original);
        // The old primary stays writable until Redis demotes it. A successful
        // SET on the pre-failover pool is not evidence that the client moved.
        let old_write = fixture_output(original.1, &["SET", "migration:failover", "stale"]);
        let old_readonly = fixture_text(&old_write).contains("READONLY");
        let wrote = client
            .set("migration:failover", "new-primary", Some(Duration::from_secs(60)))
            .await
            .is_ok();
        let visible = current.as_ref().is_some_and(|(_, port)| {
            let got = fixture_output(*port, &["GET", "migration:failover"]);
            got.status.success() && String::from_utf8_lossy(&got.stdout).trim() == "new-primary"
        });
        if promoted && old_readonly && wrote && visible {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Sentinel failover did not recover"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        client.get("migration:failover").await.unwrap().as_deref(),
        Some("new-primary")
    );
    client.shutdown().await.unwrap();
    // Restore the original primary so other fixture scenarios retain their address.
    request_sentinel_failover(
        tokio::time::Instant::now() + Duration::from_secs(30),
        "Sentinel restore was not admitted",
    )
    .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if sentinel_master(16410).as_ref() == Some(&original) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Sentinel original primary was not restored"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.topologies.yaml"]
async fn cluster_seed_loss_reestablishes_from_known_primaries() {
    let _serial = TOPOLOGY_LOCK.lock().await;
    init_crypto();
    let masters = parse_masters(&fixture_command(16400, &["CLUSTER", "NODES"]));
    assert!(masters.len() >= 2, "cluster fixture needs two primaries");
    let seed = masters[0].0;
    let survivor = masters.iter().find(|(port, _)| *port != seed).expect("survivor").0;
    let client = client(json!({
        "endpoints": [format!("127.0.0.1:{seed}")],
        "topology": {"mode": "cluster"},
        "timeout": {"connect": "1s", "command": "1s"}
    }));
    client.init_for_test().await.unwrap();
    let key = format!("{{seed-loss}}:{}", std::process::id());
    client
        .set(&key, "before", Some(Duration::from_secs(120)))
        .await
        .unwrap();
    let _restore = ClusterNodeRestore(seed);
    let shutdown = fixture_output(seed, &["SHUTDOWN", "NOSAVE"]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let ping = fixture_output(seed, &["PING"]);
        let info = fixture_output(survivor, &["CLUSTER", "INFO"]);
        let nodes = fixture_output(survivor, &["CLUSTER", "NODES"]);
        let down = !ping.status.success();
        let demoted = nodes.status.success()
            && !parse_masters(&String::from_utf8_lossy(&nodes.stdout))
                .iter()
                .any(|(port, _)| *port == seed);
        let healthy = info.status.success() && info.stdout.windows(16).any(|window| window == b"cluster_state:ok");
        if down && demoted && healthy {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "seed loss did not fail over: seed={seed} shutdown_status={} stderr={} nodes={}",
            shutdown.status,
            String::from_utf8_lossy(&shutdown.stderr),
            String::from_utf8_lossy(&nodes.stdout)
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if client.reestablish_for_test().await.is_ok() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "reestablish kept dialing only the removed seed"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if client.set(&key, "after", Some(Duration::from_secs(120))).await.is_ok() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "known-primary recovery could not write"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(client.get(&key).await.unwrap().as_deref(), Some("after"));
    client.shutdown().await.unwrap();
}

struct SlotRestore {
    slot: u16,
    owner: String,
    primaries: Vec<u16>,
}

impl Drop for SlotRestore {
    fn drop(&mut self) {
        let slot = self.slot.to_string();
        let failed: Vec<u16> = self
            .primaries
            .iter()
            .copied()
            .filter(|port| {
                !fixture_output(*port, &["CLUSTER", "SETSLOT", &slot, "NODE", &self.owner])
                    .status
                    .success()
            })
            .collect();
        if failed.is_empty() {
            return;
        }
        let message = format!("slot {slot} restore failed on primaries {failed:?}");
        if std::thread::panicking() {
            eprintln!("{message}");
        } else {
            panic!("{message}");
        }
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.topologies.yaml"]
async fn cluster_moved_follows_only_the_configured_limit() {
    let _serial = TOPOLOGY_LOCK.lock().await;
    init_crypto();
    let key = format!("moved:{}", std::process::id());
    let slot: u16 = fixture_command(16400, &["CLUSTER", "KEYSLOT", &key])
        .trim()
        .parse()
        .unwrap();
    let nodes = fixture_command(16400, &["CLUSTER", "NODES"]);
    let (owner_port, owner_id) = slot_owner(&nodes, slot).expect("slot owner");
    let masters = parse_masters(&nodes);
    let (dest_port, dest_id) = masters
        .iter()
        .find(|(port, _)| *port != owner_port)
        .cloned()
        .expect("destination primary");
    let endpoint = format!("127.0.0.1:{owner_port}");
    let limited = client(json!({
        "endpoints": [endpoint.clone()],
        "topology": {"mode": "cluster", "cluster": {"maxRedirects": 0}},
        "timeout": {"connect": "1s", "command": "1s"}
    }));
    let following = client(json!({
        "endpoints": [endpoint],
        "topology": {"mode": "cluster"},
        "timeout": {"connect": "1s", "command": "1s"}
    }));
    limited.init_for_test().await.unwrap();
    following.init_for_test().await.unwrap();
    let _restore = SlotRestore {
        slot,
        owner: owner_id,
        primaries: masters.iter().map(|(port, _)| *port).collect(),
    };
    let slot_text = slot.to_string();
    // SETSLOT NODE targets primaries; replicas reject it and follow by gossip.
    for (port, _) in &masters {
        fixture_command(*port, &["CLUSTER", "SETSLOT", &slot_text, "NODE", &dest_id]);
    }
    let blocked = limited
        .set(&key, "blocked", Some(Duration::from_secs(60)))
        .await
        .expect_err("a zero redirect limit must not follow MOVED");
    // MOVED is a server error reply, not a timeout or I/O failure.
    assert_eq!(blocked.to_string(), "Redis command failed");
    assert!(fixture_command(dest_port, &["GET", &key]).trim().is_empty());
    following
        .set(&key, "allowed", Some(Duration::from_secs(60)))
        .await
        .unwrap();
    assert_eq!(fixture_command(dest_port, &["GET", &key]).trim(), "allowed");
    limited.shutdown().await.unwrap();
    following.shutdown().await.unwrap();
    fixture_command(dest_port, &["DEL", &key]);
}
