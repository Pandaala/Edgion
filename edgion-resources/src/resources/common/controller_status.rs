//! Controller-partitioned status envelopes for resources with flat local status.
//!
//! FileSystem and etcd retain the native local status shape. Kubernetes may carry
//! multiple controller observations in the `controllers` envelope; callers must
//! project one observation before mutating a local status.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::DeserializeOwned;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::Condition;

/// Maximum number of controller observations retained in a Kubernetes status.
pub const MAX_CONTROLLER_OBSERVATIONS: usize = 32;

/// A deployment's status observation, keyed only by controllerName.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControllerObservation<T> {
    #[schemars(length(min = 1))]
    pub controller_name: String,
    pub status: T,
}

/// The Kubernetes-only wire envelope. It deliberately has no native-status
/// alternative: `ControllerStatus` handles that compatibility at serde time.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControllerEnvelope<T> {
    #[schemars(length(max = MAX_CONTROLLER_OBSERVATIONS))]
    pub controllers: Vec<ControllerObservation<T>>,
}

/// The common local status shared by resources whose only local field is
/// Gateway API-style conditions.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConditionsStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

/// A flat status in native backends or a Kubernetes controller partition.
///
/// Do not dereference this type into `T`: an unprojected controller envelope
/// must never silently become a default local status.
#[derive(Clone, Debug, PartialEq)]
pub enum ControllerStatus<T> {
    Native(T),
    Partitioned(Vec<ControllerObservation<T>>),
    /// An invalid Kubernetes envelope retained verbatim so it cannot prevent
    /// the resource's configuration from being processed. The status pipeline
    /// rejects it before persistence rather than treating it as local status.
    Opaque(serde_json::Value),
}

impl<T: Default> Default for ControllerStatus<T> {
    fn default() -> Self {
        Self::Native(T::default())
    }
}

impl<T> From<T> for ControllerStatus<T> {
    fn from(status: T) -> Self {
        Self::Native(status)
    }
}

impl<T> ControllerStatus<T> {
    /// Consume a native status. An unprojected controller envelope returns
    /// `None` so callers must explicitly select an observation first.
    pub fn into_local(self) -> Option<T> {
        match self {
            Self::Native(status) => Some(status),
            Self::Partitioned(_) => None,
            Self::Opaque(_) => None,
        }
    }

    /// Return the native local status when this value is not an envelope.
    pub fn as_local(&self) -> Option<&T> {
        match self {
            Self::Native(status) => Some(status),
            Self::Partitioned(_) => None,
            Self::Opaque(_) => None,
        }
    }

    /// Return the native local status mutably when this value is not an envelope.
    pub fn as_local_mut(&mut self) -> Option<&mut T> {
        match self {
            Self::Native(status) => Some(status),
            Self::Partitioned(_) => None,
            Self::Opaque(_) => None,
        }
    }

    /// Return controller observations when this value is a Kubernetes envelope.
    pub fn controllers(&self) -> Option<&[ControllerObservation<T>]> {
        match self {
            Self::Native(_) => None,
            Self::Opaque(_) => None,
            Self::Partitioned(controllers) => Some(controllers),
        }
    }
}

impl<T: Serialize> Serialize for ControllerStatus<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Native(status) => status.serialize(serializer),
            Self::Partitioned(controllers) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("controllers", controllers)?;
                map.end()
            }
            Self::Opaque(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for ControllerStatus<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if let Some(object) = value.as_object() {
            if object.contains_key("controllers") {
                if object.len() != 1 {
                    return Ok(Self::Opaque(value));
                }
                let Ok(envelope) = serde_json::from_value::<ControllerEnvelope<T>>(value.clone()) else {
                    return Ok(Self::Opaque(value));
                };
                if envelope.controllers.len() > MAX_CONTROLLER_OBSERVATIONS {
                    return Ok(Self::Opaque(value));
                }
                return Ok(Self::Partitioned(envelope.controllers));
            }
        }
        serde_json::from_value(value.clone())
            .map(Self::Native)
            .or(Ok(Self::Opaque(value)))
    }
}

impl<T: JsonSchema> JsonSchema for ControllerStatus<T> {
    fn schema_name() -> Cow<'static, str> {
        format!("ControllerStatus_{}", T::schema_name()).into()
    }

    fn schema_id() -> Cow<'static, str> {
        format!("{}::ControllerStatus<{}>", module_path!(), T::schema_id()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        ControllerEnvelope::<T>::json_schema(generator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Local {
        value: String,
    }

    #[test]
    fn serde_preserves_native_and_partitioned_shapes_and_rejects_mixing() {
        let native = ControllerStatus::from(Local { value: "native".into() });
        assert_eq!(
            serde_json::to_value(&native).unwrap(),
            serde_json::json!({"value": "native"})
        );
        assert_eq!(
            serde_json::from_value::<ControllerStatus<Local>>(serde_json::json!({"value": "native"})).unwrap(),
            native
        );

        let partitioned = ControllerStatus::Partitioned(vec![ControllerObservation {
            controller_name: "example.io/controller".into(),
            status: Local {
                value: "partitioned".into(),
            },
        }]);
        assert_eq!(
            serde_json::to_value(&partitioned).unwrap(),
            serde_json::json!({"controllers": [{
                "controllerName": "example.io/controller",
                "status": {"value": "partitioned"}
            }]})
        );
        let mixed = serde_json::from_value::<ControllerStatus<Local>>(serde_json::json!({
            "controllers": [], "value": "mixed"
        }))
        .unwrap();
        assert!(matches!(mixed, ControllerStatus::Opaque(_)));
    }

    #[test]
    fn schema_exposes_only_the_controller_envelope() {
        let schema = schemars::schema_for!(ControllerStatus<Local>);
        let value = serde_json::to_value(schema).unwrap();
        assert!(value["properties"].get("controllers").is_some());
        assert!(value["properties"].get("value").is_none());
    }
}
