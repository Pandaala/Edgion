use crate::types::HTTPRouteMatch;

#[derive(Clone)]
pub struct MatchInfo {
    /// route namespace
    pub rns: String,
    /// route name
    pub rn: String,
    /// match item
    pub m: HTTPRouteMatch,
}

impl MatchInfo {
    pub fn new(rns: String, rn: String, m: HTTPRouteMatch) -> Self {
        Self { rns, rn, m }
    }
}

