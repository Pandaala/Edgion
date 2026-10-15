//! Controller-owned policy attachment admission, independent of policy arbitration.

use serde::{Deserialize, Serialize};

pub const MAX_POLICY_ANCESTORS: usize = 16;

/// Native backends have one writer. Kubernetes policies carry the exact bounded
/// Gateway keys admitted by the ownership-preserving status composition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "gateways", rename_all = "camelCase")]
pub enum PreparedPolicyAncestorAdmission {
    Unrestricted,
    Scoped(Vec<String>),
}

impl PreparedPolicyAncestorAdmission {
    /// Cold validation never turns malformed preparation into unrestricted access.
    pub fn validated(self) -> Self {
        match self {
            Self::Scoped(mut keys) => {
                if keys.len() > MAX_POLICY_ANCESTORS
                    || keys.iter().any(|key| {
                        !key.split_once('/')
                            .is_some_and(|(ns, name)| !ns.is_empty() && !name.is_empty() && !name.contains('/'))
                    })
                {
                    return Self::Scoped(Vec::new());
                }
                keys.sort_unstable();
                keys.dedup();
                Self::Scoped(keys)
            }
            native => native,
        }
    }

    /// Keys are already canonical on the publication path; borrow on selection.
    pub fn permits(&self, gateway_key: Option<&str>) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::Scoped(keys) => {
                keys.len() <= MAX_POLICY_ANCESTORS
                    && gateway_key.is_some_and(|key| keys.iter().any(|admitted| admitted == key))
            }
        }
    }
}

pub fn policy_ancestor_permitted(
    admission: Option<&PreparedPolicyAncestorAdmission>,
    gateway_key: Option<&str>,
) -> bool {
    admission.is_some_and(|admission| admission.permits(gateway_key))
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/policy_ancestor_admission_test.rs"]
mod tests;
