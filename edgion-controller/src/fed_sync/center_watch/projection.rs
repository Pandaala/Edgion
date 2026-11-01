//! Allowlisted Center identities. No source spec or resolved runtime object is retained.
use edgion_resources::prelude_resources::{EdgionConfigData, EdgionPlugins};
use edgion_resources::resources::edgion_config_data::{region_route::validate_setting_identity, ConfigEntry};
use edgion_resources::resources::edgion_plugins::EdgionPlugin;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::Serialize;

use super::StoreError;

const MAX_ENTRIES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum SourceKind {
    EdgionConfigData,
    EdgionPlugins,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceKey {
    pub(crate) kind: SourceKind,
    pub(crate) namespace: String,
    pub(crate) name: String,
}
impl SourceKey {
    pub(crate) fn new(kind: SourceKind, namespace: &str, name: &str) -> Result<Self, StoreError> {
        Ok(Self {
            kind,
            namespace: field(namespace, 63)?,
            name: field(name, 253)?,
        })
    }
    pub(super) fn budget_bytes(&self) -> usize {
        256 + 6 * (self.namespace.capacity() + self.name.capacity())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub(crate) key: SourceKey,
    pub(crate) uid: Option<String>,
    pub(crate) resource_version: Option<String>,
}
impl Identity {
    fn from_meta(kind: SourceKind, meta: &ObjectMeta) -> Result<Self, StoreError> {
        Ok(Self {
            key: SourceKey::new(
                kind,
                meta.namespace.as_deref().unwrap_or("default"),
                meta.name.as_deref().unwrap_or(""),
            )?,
            uid: optional(meta.uid.as_deref(), 128)?,
            resource_version: optional(meta.resource_version.as_deref(), 256)?,
        })
    }
    fn budget_bytes(&self) -> usize {
        self.key.budget_bytes() + 256 + 6 * (length(&self.uid) + length(&self.resource_version))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub(crate) identity: Identity,
    pub(crate) enabled: bool,
    pub(crate) cluster_name: Option<String>,
    pub(crate) region_name: Option<String>,
    pub(crate) invalid_identity: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Consumer {
    pub(crate) entry_index: usize,
    pub(crate) alias: Option<String>,
    pub(crate) enabled: bool,
    pub(crate) setting_namespace: String,
    pub(crate) setting_name: String,
    pub(crate) fail_open: bool,
    pub(crate) status_on_error: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginResource {
    pub(crate) identity: Identity,
    pub(crate) entries: Vec<Consumer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum Record {
    Setting(Setting),
    Plugin(PluginResource),
}
impl Record {
    pub(crate) fn key(&self) -> &SourceKey {
        match self {
            Self::Setting(value) => &value.identity.key,
            Self::Plugin(value) => &value.identity.key,
        }
    }

    /// Conservative bound covering heap bookkeeping and worst-case JSON escaping.
    /// No generic JSON tree or output buffer is allocated to measure the record.
    pub(super) fn budget_bytes(&self) -> usize {
        match self {
            Self::Setting(row) => {
                row.identity.budget_bytes() + 512 + 6 * (length(&row.cluster_name) + length(&row.region_name))
            }
            Self::Plugin(row) => {
                row.identity.budget_bytes()
                    + 512
                    + row.entries.capacity() * 256
                    + row
                        .entries
                        .iter()
                        .map(|entry| {
                            6 * (length(&entry.alias)
                                + entry.setting_namespace.capacity()
                                + entry.setting_name.capacity())
                        })
                        .sum::<usize>()
            }
        }
    }
}

fn field(value: &str, max: usize) -> Result<String, StoreError> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(StoreError::Projection);
    }
    Ok(value.to_owned())
}
fn optional(value: Option<&str>, max: usize) -> Result<Option<String>, StoreError> {
    value.map(|text| field(text, max)).transpose()
}
fn length(value: &Option<String>) -> usize {
    value.as_ref().map_or(0, String::capacity)
}

pub fn setting(source: &EdgionConfigData) -> Result<Option<Record>, StoreError> {
    let ConfigEntry::ClusterRegionSetting(config) = &source.spec.data else {
        return Ok(None);
    };
    let valid_identity = validate_setting_identity(&config.cluster_name, &config.region_name).is_ok();
    Ok(Some(Record::Setting(Setting {
        identity: Identity::from_meta(SourceKind::EdgionConfigData, &source.metadata)?,
        enabled: source.spec.enable,
        cluster_name: valid_identity.then(|| config.cluster_name.clone()),
        region_name: valid_identity.then(|| config.region_name.clone()),
        invalid_identity: !valid_identity,
    })))
}

pub fn plugins(source: &EdgionPlugins) -> Result<Option<Record>, StoreError> {
    let mut entries = Vec::new();
    for (entry_index, entry) in source.spec.request_plugins.iter().flatten().enumerate() {
        let EdgionPlugin::RegionRoute(config) = &entry.plugin else {
            continue;
        };
        if entries.len() == MAX_ENTRIES {
            return Err(StoreError::Projection);
        }
        entries.push(Consumer {
            entry_index,
            alias: optional(entry.alias.as_deref(), 256)?,
            enabled: entry.enable,
            setting_namespace: field(
                config
                    .cluster_region_ref
                    .namespace
                    .as_deref()
                    .unwrap_or(source.metadata.namespace.as_deref().unwrap_or("default")),
                63,
            )?,
            setting_name: field(&config.cluster_region_ref.name, 253)?,
            fail_open: config.fail_open,
            status_on_error: config.status_on_error,
        });
    }
    if entries.is_empty() {
        return Ok(None);
    }
    Ok(Some(Record::Plugin(PluginResource {
        identity: Identity::from_meta(SourceKind::EdgionPlugins, &source.metadata)?,
        entries,
    })))
}

#[cfg(test)]
#[path = "../../../tests/unit/fed_sync/center_watch/projection_test.rs"]
mod tests;
