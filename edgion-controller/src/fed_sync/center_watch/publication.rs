//! Pure projections driven by the existing per-kind resource processor.
use super::{
    projection::{self, SourceKey, SourceKind},
    CenterWatchStore, StoreError,
};
use edgion_resources::prelude_resources::{EdgionConfigData, EdgionPlugins};
use std::any::Any;

pub(crate) fn apply<T: Any>(store: &CenterWatchStore, generation: &str, source: &T, eligible: bool) {
    let any = source as &dyn Any;
    let (kind, metadata) = if let Some(source) = any.downcast_ref::<EdgionConfigData>() {
        (SourceKind::EdgionConfigData, &source.metadata)
    } else if let Some(source) = any.downcast_ref::<EdgionPlugins>() {
        (SourceKind::EdgionPlugins, &source.metadata)
    } else {
        return;
    };
    let key = SourceKey::new(
        kind,
        metadata.namespace.as_deref().unwrap_or("default"),
        metadata.name.as_deref().unwrap_or(""),
    );
    let Ok(key) = key else {
        let _ = store.fail(generation, StoreError::Projection);
        return;
    };
    if !eligible {
        let _ = store.delete(generation, &key);
        return;
    }
    let projected = match kind {
        SourceKind::EdgionConfigData => {
            projection::setting(any.downcast_ref::<EdgionConfigData>().expect("matched type"))
        }
        SourceKind::EdgionPlugins => projection::plugins(any.downcast_ref::<EdgionPlugins>().expect("matched type")),
    };
    match projected {
        Ok(Some(row)) => {
            let _ = store.upsert(generation, row);
        }
        Ok(None) => {
            let _ = store.delete(generation, &key);
        }
        Err(error) => {
            let _ = store.fail_source(generation, key, error);
        }
    }
}

pub(crate) fn delete(store: &CenterWatchStore, generation: &str, kind: &str, key: &str) {
    let kind = match kind {
        "EdgionConfigData" => SourceKind::EdgionConfigData,
        "EdgionPlugins" => SourceKind::EdgionPlugins,
        _ => return,
    };
    let (namespace, name) = key.split_once('/').unwrap_or(("default", key));
    match SourceKey::new(kind, namespace, name) {
        Ok(key) => {
            let _ = store.delete(generation, &key);
        }
        Err(error) => {
            let _ = store.fail(generation, error);
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/fed_sync/center_watch/publication_test.rs"]
mod tests;
