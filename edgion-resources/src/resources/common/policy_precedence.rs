//! Complete creation instants for Controller and Gateway policy arbitration.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Controller-prepared ordering input. An absent outer value means unprepared;
/// this value with no timestamp means the source legitimately had no timestamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedPolicyPrecedence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_timestamp: Option<DateTime<Utc>>,
}

/// Oldest complete instant wins, missing timestamps sort last, then the caller's
/// existing lexical identity breaks ties. No epoch sentinel narrows the domain.
pub fn policy_precedence_key(
    creation_timestamp: Option<DateTime<Utc>>,
    lexical_key: &str,
) -> (bool, Option<DateTime<Utc>>, &str) {
    (creation_timestamp.is_none(), creation_timestamp, lexical_key)
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/policy_precedence_test.rs"]
mod tests;
