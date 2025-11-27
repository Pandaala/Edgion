use super::match_unit::HttpRouteRuleUnit;
use super::regex_match_unit::HttpRouteRuleRegexUnit;
use crate::types::HTTPRouteRule;
use std::sync::Arc;

/// Trait for matched route units to provide common interface
pub trait MatchedRouteUnit {
    fn identifier(&self) -> String;
    fn rule(&self) -> &Arc<HTTPRouteRule>;
    fn namespace(&self) -> &str;
    fn name(&self) -> &str;
    fn resource_key(&self) -> &str;
}

impl MatchedRouteUnit for Arc<HttpRouteRuleUnit> {
    fn identifier(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
    
    fn rule(&self) -> &Arc<HTTPRouteRule> {
        &self.rule
    }
    
    fn namespace(&self) -> &str {
        &self.namespace
    }
    
    fn name(&self) -> &str {
        &self.name
    }
    
    fn resource_key(&self) -> &str {
        &self.resource_key
    }
}

impl MatchedRouteUnit for HttpRouteRuleRegexUnit {
    fn identifier(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
    
    fn rule(&self) -> &Arc<HTTPRouteRule> {
        &self.rule
    }
    
    fn namespace(&self) -> &str {
        &self.namespace
    }
    
    fn name(&self) -> &str {
        &self.name
    }
    
    fn resource_key(&self) -> &str {
        &self.resource_key
    }
}

/// Matched route result - can be either a normal route or a regex route
#[derive(Clone)]
pub enum MatchedRoute {
    Normal(Arc<HttpRouteRuleUnit>),
    Regex(HttpRouteRuleRegexUnit),
}

impl MatchedRoute {
    /// Get the identifier (namespace/name) of the matched route
    pub fn identifier(&self) -> String {
        match self {
            Self::Normal(unit) => unit.identifier(),
            Self::Regex(unit) => unit.identifier(),
        }
    }
    
    /// Get the rule associated with the matched route
    pub fn rule(&self) -> &Arc<HTTPRouteRule> {
        match self {
            Self::Normal(unit) => unit.rule(),
            Self::Regex(unit) => unit.rule(),
        }
    }
    
    /// Get the namespace of the matched route
    pub fn namespace(&self) -> &str {
        match self {
            Self::Normal(unit) => unit.namespace(),
            Self::Regex(unit) => unit.namespace(),
        }
    }
    
    /// Get the name of the matched route
    pub fn name(&self) -> &str {
        match self {
            Self::Normal(unit) => unit.name(),
            Self::Regex(unit) => unit.name(),
        }
    }
    
    /// Get the resource key of the matched route
    pub fn resource_key(&self) -> &str {
        match self {
            Self::Normal(unit) => unit.resource_key(),
            Self::Regex(unit) => unit.resource_key(),
        }
    }
}

