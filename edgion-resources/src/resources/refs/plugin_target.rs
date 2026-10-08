//! Cold EdgionPlugins reference selection without changing raw source ownership.

use super::{ObjectIdentity, ReferenceError, ResourceScope};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginTarget<T> {
    Missing,
    Unique { raw_key: String, source: T },
    Ambiguous,
}

fn validate_key(key: &str) -> Result<(&str, &str), ReferenceError> {
    let (namespace, name) = key.split_once('/').ok_or(ReferenceError::MissingNamespace)?;
    ObjectIdentity::checked("edgion.io", "EdgionPlugins", ResourceScope::Namespaced(namespace), name)?;
    Ok((namespace, name))
}

/// Validate before any exact reads. Only an omitted raw namespace can denote default.
pub fn classify<T>(key: &str, mut read: impl FnMut(&str) -> Option<T>) -> Result<PluginTarget<T>, ReferenceError> {
    let (namespace, name) = validate_key(key)?;
    let qualified = read(key);
    let bare = if namespace == "default" { read(name) } else { None };
    Ok(match (qualified, bare) {
        (Some(_), Some(_)) => PluginTarget::Ambiguous,
        (Some(source), None) => PluginTarget::Unique {
            raw_key: key.to_owned(),
            source,
        },
        (None, Some(source)) => PluginTarget::Unique {
            raw_key: name.to_owned(),
            source,
        },
        (None, None) => PluginTarget::Missing,
    })
}

/// Project a source event to its reference target; never rewrite the raw store key.
pub fn semantic_key(raw_key: &str) -> Result<String, ReferenceError> {
    let key = if raw_key.contains('/') {
        raw_key.to_owned()
    } else {
        format!("default/{raw_key}")
    };
    validate_key(&key)?;
    Ok(key)
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/plugin_target_test.rs"]
mod tests;
