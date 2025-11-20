use super::r#match::RouteRuntime;
use crate::types::err::EdError;
use crate::types::HTTPRouteRule;
use pingora_proxy::Session;

pub struct HttpRouteRuntime {
    pub namespace: String,
    pub name: String,
    pub hostname: Vec<String>,
    pub rule: HTTPRouteRule,
}

impl HttpRouteRuntime {
    pub fn new(namespace: String, name: String, rule: HTTPRouteRule, hostname: Vec<String>) -> HttpRouteRuntime {
        Self {
            namespace,
            name,
            hostname,
            rule,
        }
    }
}

impl RouteRuntime for HttpRouteRuntime {
    fn extract_paths(&self) -> Vec<(String, bool)> {
        let mut paths = Vec::new();

        if let Some(matches) = &self.rule.matches {
            for route_match in matches {
                if let Some(path) = &route_match.path {
                    if let Some(value) = &path.value {
                        let is_prefix = path.match_type.as_deref().map(|t| t == "PathPrefix").unwrap_or(false);
                        paths.push((value.clone(), is_prefix));
                    }
                }
            }
        }

        paths
    }

    fn identifier(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }

    fn deep_match(&self, _session: &Session) -> Result<bool, EdError> {
        Ok(true)
    }
}
