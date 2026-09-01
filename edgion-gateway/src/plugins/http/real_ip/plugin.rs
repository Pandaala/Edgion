//! Real IP plugin implementation
//!
//! Extracts the real client IP address from HTTP headers when behind trusted proxies.

use async_trait::async_trait;

use crate::plugins::runtime::log::{la, sensitive};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use crate::util::RealIpExtractor;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::RealIpConfig;

/// RealIp plugin: extracts real client IP using a per-instance RealIpExtractor.
pub struct RealIp {
    extractor: Option<RealIpExtractor>,
    header_log: &'static str,
}

impl RealIp {
    fn header_log(real_ip_header: &str) -> &'static str {
        match real_ip_header.to_ascii_lowercase().as_str() {
            "x-forwarded-for" => "header xff",
            "x-real-ip" => "header real-ip",
            "forwarded" => "header forwarded",
            _ => "header custom",
        }
    }

    /// Create a new RealIp plugin from configuration
    pub fn create(config: &RealIpConfig) -> Box<dyn RequestFilter> {
        // Build RealIpExtractor from plugin config
        let extractor = if !config.trusted_ips.is_empty() {
            match RealIpExtractor::new(
                &config.trusted_ips,
                config.real_ip_header.clone(),
                config.recursive,
                config.max_trusted_hops,
            ) {
                Ok(e) => Some(e),
                Err(err) => {
                    tracing::error!(
                        error = ?err,
                        "Failed to create RealIpExtractor from plugin config"
                    );
                    None
                }
            }
        } else {
            tracing::warn!("RealIp plugin: trusted_ips is empty, plugin will be no-op");
            None
        };

        let plugin = RealIp {
            extractor,
            header_log: Self::header_log(&config.real_ip_header),
        };

        Box::new(plugin)
    }
}

#[async_trait]
impl RequestFilter for RealIp {
    fn name(&self) -> &str {
        "RealIp"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Check if extractor is available
        let Some(ref extractor) = self.extractor else {
            plugin_log.push_err("no extractor");
            return PluginRunningResult::GoodNext;
        };

        plugin_log.push(self.header_log);

        let header_value = session.header_value(extractor.real_ip_header());
        let resolution = extractor.resolve_real_ip(session.downstream_peer_ip(), header_value.as_deref());

        if resolution.downstream_trusted {
            plugin_log.push("trusted proxy:match");
        } else {
            plugin_log.push("trusted proxy:miss");
        }

        let remote_ip_changed = session.remote_ip() != resolution.remote_ip.as_str();
        if remote_ip_changed {
            if let Err(_e) = session.set_remote_ip(&resolution.remote_ip) {
                plugin_log.push_err("set remote IP failed");
                return PluginRunningResult::GoodNext;
            }

            plugin_log.push_fmt("ip updated: {}", la![sensitive(&resolution.remote_ip)]);
        } else {
            plugin_log.push("ip unchanged");
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use mockall::predicate::eq;
    use serde_json::json;

    fn create_test_config() -> RealIpConfig {
        use edgion_resources::resources::edgion_config_data::IpGroup;
        RealIpConfig {
            trusted_ips: vec![IpGroup {
                name: "private".to_string(),
                description: None,
                cidrs: vec!["192.168.0.0/16".to_string(), "198.51.100.0/24".to_string()],
            }],
            real_ip_header: "X-Forwarded-For".to_string(),
            recursive: true,
            max_trusted_hops: None,
        }
    }

    #[test]
    fn test_extract_recursive() {
        let config = create_test_config(); // recursive: true
        let extractor = RealIpExtractor::new(
            &config.trusted_ips,
            config.real_ip_header.clone(),
            config.recursive,
            config.max_trusted_hops,
        )
        .unwrap();

        // Case 1: Right-to-left, find first non-trusted
        let result = extractor.extract_from_header_value("192.168.1.254", "203.0.113.1, 198.51.100.2, 192.168.1.1");
        assert_eq!(result, "203.0.113.1");

        // Case 2: All trusted, use leftmost (Nginx behavior)
        let result = extractor.extract_from_header_value("192.168.1.254", "192.168.1.1, 198.51.100.2");
        assert_eq!(result, "192.168.1.1");

        // Case 3: Single untrusted IP
        let result = extractor.extract_from_header_value("192.168.1.254", "8.8.8.8");
        assert_eq!(result, "8.8.8.8");
    }

    #[test]
    fn test_client_not_trusted() {
        let config = create_test_config();
        let extractor = RealIpExtractor::new(
            &config.trusted_ips,
            config.real_ip_header.clone(),
            config.recursive,
            config.max_trusted_hops,
        )
        .unwrap();

        // An untrusted downstream peer remains the RemoteIp.
        let result = extractor.extract_from_header_value("8.8.8.8", "203.0.113.1");
        assert_eq!(result, "8.8.8.8");
    }

    #[test]
    fn name_and_header_log_are_prepared_once() {
        let first = RealIp::create(&create_test_config());
        let second = RealIp::create(&create_test_config());
        assert_eq!(first.name(), "RealIp");
        assert!(std::ptr::eq(first.name().as_ptr(), second.name().as_ptr()));

        assert_eq!(RealIp::header_log("X-FORWARDED-FOR"), "header xff");
        assert_eq!(RealIp::header_log("x-real-IP"), "header real-ip");
        assert_eq!(RealIp::header_log("FORWARDED"), "header forwarded");
        assert_eq!(RealIp::header_log(" X-Forwarded-For"), "header custom");
        assert_eq!(RealIp::header_log("X-Client-IP"), "header custom");
    }

    #[tokio::test]
    async fn trusted_peer_updates_remote_ip_with_one_read_per_input() {
        const CHILD_ENV: &str = "EDGION_REAL_IP_MASKED_LOG_CHILD";
        const TEST_NAME: &str = "trusted_peer_updates_remote_ip_with_one_read_per_input";
        const TEST_MODULE: &str = "plugins::http::real_ip::plugin::tests";

        if std::env::var(CHILD_ENV).as_deref() != Ok(TEST_NAME) {
            let exact_test_name = format!("{TEST_MODULE}::{TEST_NAME}");
            let output = std::process::Command::new(
                std::env::current_exe().expect("the Rust test harness must have a current executable"),
            )
            .args(["--exact", exact_test_name.as_str(), "--nocapture"])
            .env(CHILD_ENV, TEST_NAME)
            .output()
            .expect("the isolated RealIp masked-log test process must start");
            assert!(
                output.status.success(),
                "isolated RealIp masked-log test failed:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        // This child runs only the exact test selected above, so changing the
        // process-global mode cannot race another lib test.
        edgion_resources::sensitive::SENSITIVE_MODE.store(1, std::sync::atomic::Ordering::SeqCst);

        let plugin = RealIp::create(&create_test_config());
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .with(eq("X-Forwarded-For"))
            .times(1)
            .return_const(Some("203.0.113.1, 192.168.1.1".to_string()));
        session
            .expect_downstream_peer_ip()
            .times(1)
            .return_const("192.168.1.254".to_string());
        session
            .expect_remote_ip()
            .times(1)
            .return_const("192.168.1.254".to_string());
        session
            .expect_set_remote_ip()
            .with(eq("203.0.113.1"))
            .times(1)
            .returning(|_| Ok(()));
        let mut log = PluginLog::new(plugin.name());

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        let entries = serde_json::to_value(&log).unwrap()["log"].as_array().unwrap().clone();
        assert_eq!(entries[0], json!("header xff"));
        assert_eq!(entries[1], json!("trusted proxy:match"));
        assert!(entries[2].as_str().unwrap().starts_with("ip updated: "));
        assert!(!serde_json::to_string(&log).unwrap().contains("203.0.113.1"));
    }

    #[tokio::test]
    async fn missing_header_keeps_remote_ip_without_calling_setter() {
        let plugin = RealIp::create(&create_test_config());
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .with(eq("X-Forwarded-For"))
            .times(1)
            .return_const(None);
        session
            .expect_downstream_peer_ip()
            .times(1)
            .return_const("192.168.1.254".to_string());
        session
            .expect_remote_ip()
            .times(1)
            .return_const("192.168.1.254".to_string());
        session.expect_set_remote_ip().times(0);
        let mut log = PluginLog::new(plugin.name());

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert_eq!(
            serde_json::to_value(&log).unwrap()["log"],
            json!(["header xff", "trusted proxy:match", "ip unchanged"])
        );
    }

    #[tokio::test]
    async fn untrusted_peer_ignores_forwarded_header() {
        let plugin = RealIp::create(&create_test_config());
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .with(eq("X-Forwarded-For"))
            .times(1)
            .return_const(Some("203.0.113.1".to_string()));
        session
            .expect_downstream_peer_ip()
            .times(1)
            .return_const("8.8.8.8".to_string());
        session.expect_remote_ip().times(1).return_const("8.8.8.8".to_string());
        session.expect_set_remote_ip().times(0);
        let mut log = PluginLog::new(plugin.name());

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert_eq!(
            serde_json::to_value(&log).unwrap()["log"],
            json!(["header xff", "trusted proxy:miss", "ip unchanged"])
        );
    }

    #[tokio::test]
    async fn invalid_or_empty_peer_resolves_to_unchanged_unspecified_ip() {
        for peer in ["", "not-an-address"] {
            let plugin = RealIp::create(&create_test_config());
            let mut session = MockPluginSession::new();
            session
                .expect_header_value()
                .with(eq("X-Forwarded-For"))
                .times(1)
                .return_const(None);
            session
                .expect_downstream_peer_ip()
                .times(1)
                .return_const(peer.to_string());
            session.expect_remote_ip().times(1).return_const("0.0.0.0".to_string());
            session.expect_set_remote_ip().times(0);
            let mut log = PluginLog::new(plugin.name());

            assert_eq!(
                plugin.run_request(&mut session, &mut log).await,
                PluginRunningResult::GoodNext
            );
            assert_eq!(
                serde_json::to_value(&log).unwrap()["log"],
                json!(["header xff", "trusted proxy:miss", "ip unchanged"]),
                "peer={peer:?}"
            );
        }
    }

    #[tokio::test]
    async fn setter_failure_is_non_terminal_and_statically_logged() {
        let plugin = RealIp::create(&create_test_config());
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .times(1)
            .return_const(Some("203.0.113.1".to_string()));
        session
            .expect_downstream_peer_ip()
            .times(1)
            .return_const("192.168.1.254".to_string());
        session
            .expect_remote_ip()
            .times(1)
            .return_const("192.168.1.254".to_string());
        session
            .expect_set_remote_ip()
            .with(eq("203.0.113.1"))
            .times(1)
            .returning(|_| Err(Box::new(std::io::Error::other("test setter failure"))));
        let mut log = PluginLog::new(plugin.name());

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert_eq!(
            serde_json::to_value(&log).unwrap()["log"],
            json!(["header xff", "trusted proxy:match", "err: set remote IP failed"])
        );
    }

    #[tokio::test]
    async fn missing_extractor_does_not_read_session() {
        let plugin = RealIp::create(&RealIpConfig::default());
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new(plugin.name());

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert_eq!(serde_json::to_value(&log).unwrap()["log"], json!(["err: no extractor"]));
    }
}
