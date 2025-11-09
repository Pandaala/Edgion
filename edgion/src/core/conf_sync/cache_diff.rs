use crate::core::conf_sync::{ServerCache, ClientCache, Versionable};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct CacheDiffItem {
    pub key: u64,
    pub center: serde_json::Value,
    pub hub: serde_json::Value,
}

#[derive(Debug, Default, Clone)]
pub struct CacheDiff {
    pub only_in_center: Vec<u64>,
    pub only_in_hub: Vec<u64>,
    pub differing: Vec<CacheDiffItem>,
}

impl CacheDiff {
    pub fn is_empty(&self) -> bool {
        self.only_in_center.is_empty() && self.only_in_hub.is_empty() && self.differing.is_empty()
    }
}

pub async fn diff_center_hub<T>(center: &ServerCache<T>, hub: &ClientCache<T>) -> CacheDiff
where
    T: Versionable + Clone + Serialize + Send + Sync + 'static,
{
    let center_store = center.get_store();
    let center_data_vec: Vec<T> = {
        let store_guard = center_store.read().await;
        let (data, _version) = store_guard.snapshot_owned();
        data
    };
    let hub_snapshot = hub.list_owned();

    let mut center_map: HashMap<u64, T> = HashMap::new();
    let mut hub_map: HashMap<u64, T> = HashMap::new();

    for item in center_data_vec.into_iter() {
        center_map.insert(item.get_version(), item);
    }

    for item in hub_snapshot.data {
        hub_map.insert(item.get_version(), item);
    }

    let center_keys: HashSet<u64> = center_map.keys().copied().collect();
    let hub_keys: HashSet<u64> = hub_map.keys().copied().collect();

    let only_in_center: Vec<u64> = center_keys.difference(&hub_keys).copied().collect();
    let only_in_hub: Vec<u64> = hub_keys.difference(&center_keys).copied().collect();

    let mut differing = Vec::new();
    for key in center_keys.intersection(&hub_keys) {
        let key = *key;
        let center_value = center_map.get(&key).expect("key present in center_map");
        let hub_value = hub_map.get(&key).expect("key present in hub_map");
        let center_json = serde_json::to_value(center_value).expect("serialize center value");
        let hub_json = serde_json::to_value(hub_value).expect("serialize hub value");

        if center_json != hub_json {
            differing.push(CacheDiffItem {
                key,
                center: center_json,
                hub: hub_json,
            });
        }
    }

    CacheDiff {
        only_in_center,
        only_in_hub,
        differing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::conf_sync::traits::ResourceChange;
    use crate::core::conf_sync::EventDispatch;
    use serde::Serialize;

    #[derive(Debug, Clone, Serialize)]
    struct TestResource {
        version: u64,
        value: &'static str,
    }

    impl Versionable for TestResource {
        fn get_version(&self) -> u64 {
            self.version
        }
    }

    #[tokio::test]
    async fn detects_differences_between_center_and_hub() {
        let mut center = ServerCache::new(10);
        let mut hub = ClientCache::new();

        center.apply_change(
            ResourceChange::InitAdd,
            TestResource {
                version: 1,
                value: "center-only",
            },
            Some(1),
        );

        center.apply_change(
            ResourceChange::InitAdd,
            TestResource {
                version: 2,
                value: "shared-but-different-center",
            },
            Some(2),
        );

        hub.apply_change(
            ResourceChange::InitAdd,
            TestResource {
                version: 3,
                value: "hub-only",
            },
            Some(3),
        );

        hub.apply_change(
            ResourceChange::InitAdd,
            TestResource {
                version: 2,
                value: "shared-but-different-hub",
            },
            Some(2),
        );

        let diff = diff_center_hub(&center, &hub).await;

        assert_eq!(diff.only_in_center, vec![1]);
        assert_eq!(diff.only_in_hub, vec![3]);
        assert_eq!(diff.differing.len(), 1);
        assert_eq!(diff.differing[0].key, 2);
        assert_eq!(
            diff.differing[0]
                .center
                .get("value")
                .and_then(|v| v.as_str()),
            Some("shared-but-different-center")
        );
        assert_eq!(
            diff.differing[0].hub.get("value").and_then(|v| v.as_str()),
            Some("shared-but-different-hub")
        );
    }
}
