//! WebhookKeyGet plugin implementation
//!
//! Resolves webhook vars into request context once per request. Resolution
//! failures never reject the request: a configured `default` is written, and
//! with no default the var is left unwritten.

use async_trait::async_trait;
use edgion_resources::common::WebhookExtract;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::WebhookKeyGetConfig;

use crate::link_sys::providers::webhook::resolve_webhook_key;
use crate::plugins::runtime::log::la;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};

#[async_trait]
trait WebhookKeyResolver: Send + Sync {
    async fn resolve(
        &self,
        session: &dyn PluginSession,
        webhook_ref: &str,
        path_override: Option<&str>,
        extract: &WebhookExtract,
    ) -> Option<String>;
}

struct ProductionWebhookResolver;

#[async_trait]
impl WebhookKeyResolver for ProductionWebhookResolver {
    async fn resolve(
        &self,
        session: &dyn PluginSession,
        webhook_ref: &str,
        path_override: Option<&str>,
        extract: &WebhookExtract,
    ) -> Option<String> {
        resolve_webhook_key(session, webhook_ref, None, path_override, extract).await
    }
}

/// WebhookKeyGet plugin: resolve webhook vars into request context.
pub struct WebhookKeyGet {
    name: String,
    config: WebhookKeyGetConfig,
    resolver: Box<dyn WebhookKeyResolver>,
}

impl WebhookKeyGet {
    /// Create a new WebhookKeyGet plugin from configuration.
    ///
    /// The production path always calls [`resolve_webhook_key`].
    pub fn create(config: &WebhookKeyGetConfig) -> Box<dyn RequestFilter> {
        let mut validated_config = config.clone();
        let _ = validated_config.validate_and_compile();
        Box::new(WebhookKeyGet {
            name: "WebhookKeyGet".to_string(),
            config: validated_config,
            resolver: Box::new(ProductionWebhookResolver),
        })
    }

    #[cfg(test)]
    fn create_with_resolver(
        config: &WebhookKeyGetConfig,
        resolver: Box<dyn WebhookKeyResolver>,
    ) -> Box<dyn RequestFilter> {
        let mut validated_config = config.clone();
        let _ = validated_config.validate_and_compile();
        Box::new(WebhookKeyGet {
            name: "WebhookKeyGet".to_string(),
            config: validated_config,
            resolver,
        })
    }
}

#[async_trait]
impl RequestFilter for WebhookKeyGet {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        for var in &self.config.vars {
            let resolved = self
                .resolver
                .resolve(session, &var.webhook_ref, var.path_override.as_deref(), &var.extract)
                .await;

            let value = match resolved {
                Some(v) => {
                    plugin_log.push_fmt("key {} set", la![var.name.as_str()]);
                    Some(v)
                }
                None => match var.default.as_ref() {
                    Some(default) => {
                        plugin_log.push_fmt("key {} default", la![var.name.as_str()]);
                        Some(default.clone())
                    }
                    None => {
                        plugin_log.push_fmt("key {} skip", la![var.name.as_str()]);
                        None
                    }
                },
            };

            if let Some(v) = value {
                if session.set_ctx_var(&var.name, &v).is_err() {
                    plugin_log.push_err("ctx var set fail");
                }
            }
        }

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::edgion_plugins::WebhookVar;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::Arc;

    struct ScriptedResolver {
        /// Values returned in call order. `None` means the resolver yielded nothing.
        values: Vec<Option<String>>,
        idx: Mutex<usize>,
    }

    #[async_trait]
    impl WebhookKeyResolver for ScriptedResolver {
        async fn resolve(
            &self,
            _session: &dyn PluginSession,
            _webhook_ref: &str,
            _path_override: Option<&str>,
            _extract: &WebhookExtract,
        ) -> Option<String> {
            let mut idx = self.idx.lock();
            let i = *idx;
            *idx += 1;
            drop(idx);
            self.values.get(i).cloned().flatten()
        }
    }

    /// Records the var *name* by looking it up from webhook_ref via the test map.
    struct NamedScriptedResolver {
        /// webhook_ref -> (var name used in the test, resolved value)
        by_ref: HashMap<String, (String, Option<String>)>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl WebhookKeyResolver for NamedScriptedResolver {
        async fn resolve(
            &self,
            _session: &dyn PluginSession,
            webhook_ref: &str,
            _path_override: Option<&str>,
            _extract: &WebhookExtract,
        ) -> Option<String> {
            let (name, value) = self
                .by_ref
                .get(webhook_ref)
                .cloned()
                .unwrap_or_else(|| (webhook_ref.to_string(), None));
            self.calls.lock().push(name);
            value
        }
    }

    fn var(name: &str, webhook_ref: &str, default: Option<&str>) -> WebhookVar {
        WebhookVar {
            name: name.to_string(),
            webhook_ref: webhook_ref.to_string(),
            path_override: None,
            extract: WebhookExtract::BodyText,
            default: default.map(|s| s.to_string()),
        }
    }

    fn wire_ctx_store(mock: &mut MockPluginSession) -> Arc<Mutex<HashMap<String, String>>> {
        let store = Arc::new(Mutex::new(HashMap::<String, String>::new()));
        mock.expect_set_ctx_var().returning({
            let store = store.clone();
            move |k, v| {
                store.lock().insert(k.to_string(), v.to_string());
                Ok(())
            }
        });
        mock.expect_get_ctx_var().returning({
            let store = store.clone();
            move |k| store.lock().get(k).cloned()
        });
        store
    }

    #[tokio::test]
    async fn resolution_success_writes_ctx_var() {
        let config = WebhookKeyGetConfig {
            vars: vec![var("tenant", "ns/resolver", None)],
        };
        let plugin = WebhookKeyGet::create_with_resolver(
            &config,
            Box::new(ScriptedResolver {
                values: vec![Some("gold".to_string())],
                idx: Mutex::new(0),
            }),
        );

        let mut session = MockPluginSession::new();
        wire_ctx_store(&mut session);
        let mut plugin_log = PluginLog::new("WebhookKeyGet");

        let result = plugin.run_request(&mut session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(session.get_ctx_var("tenant").as_deref(), Some("gold"));
    }

    #[tokio::test]
    async fn resolver_none_with_default_writes_default() {
        let config = WebhookKeyGetConfig {
            vars: vec![var("tenant", "ns/resolver", Some("free"))],
        };
        let plugin = WebhookKeyGet::create_with_resolver(
            &config,
            Box::new(ScriptedResolver {
                values: vec![None],
                idx: Mutex::new(0),
            }),
        );

        let mut session = MockPluginSession::new();
        wire_ctx_store(&mut session);
        let mut plugin_log = PluginLog::new("WebhookKeyGet");

        let result = plugin.run_request(&mut session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(session.get_ctx_var("tenant").as_deref(), Some("free"));
    }

    #[tokio::test]
    async fn resolver_none_without_default_leaves_var_unwritten() {
        let config = WebhookKeyGetConfig {
            vars: vec![var("tenant", "ns/resolver", None)],
        };
        let plugin = WebhookKeyGet::create_with_resolver(
            &config,
            Box::new(ScriptedResolver {
                values: vec![None],
                idx: Mutex::new(0),
            }),
        );

        let mut session = MockPluginSession::new();
        wire_ctx_store(&mut session);
        let mut plugin_log = PluginLog::new("WebhookKeyGet");

        let result = plugin.run_request(&mut session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(session.get_ctx_var("tenant"), None);
    }

    #[tokio::test]
    async fn multiple_vars_resolve_in_configuration_order() {
        let config = WebhookKeyGetConfig {
            vars: vec![var("first", "ns/one", None), var("second", "ns/two", None)],
        };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let plugin = WebhookKeyGet::create_with_resolver(
            &config,
            Box::new(NamedScriptedResolver {
                by_ref: HashMap::from([
                    ("ns/one".to_string(), ("first".to_string(), Some("a".to_string()))),
                    ("ns/two".to_string(), ("second".to_string(), Some("b".to_string()))),
                ]),
                calls: calls.clone(),
            }),
        );

        let mut session = MockPluginSession::new();
        wire_ctx_store(&mut session);
        let mut plugin_log = PluginLog::new("WebhookKeyGet");

        let result = plugin.run_request(&mut session, &mut plugin_log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(*calls.lock(), vec!["first", "second"]);
        assert_eq!(session.get_ctx_var("first").as_deref(), Some("a"));
        assert_eq!(session.get_ctx_var("second").as_deref(), Some("b"));
    }
}
