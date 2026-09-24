use super::*;
use crate::conf_mgr::conf_center::traits::CenterApi;

#[tokio::test]
async fn update_preserves_server_fields_and_replaces_operator_fields() {
    let tmp = tempfile::tempdir().expect("temporary directory must be created");
    let storage = FileSystemStorage::new(tmp.path());
    let current = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: route
  namespace: default
  uid: uid-1
  resourceVersion: "42"
  generation: 7
  ownerReferences:
    - name: owner
  finalizers:
    - edgion.io/finalizer
  labels:
    old: label
  annotations:
    old: annotation
spec:
  hostnames: [old.example]
  resolvedListeners:
    - name: runtime-only
status:
  conditions:
    - type: Accepted
      status: "True"
"#;
    storage
        .create_one("HTTPRoute", Some("default"), "route", current.to_string())
        .await
        .expect("seed resource must be created");

    let requested = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: route
  namespace: default
  resourceVersion: "43"
  labels:
    new: label
spec:
  hostnames: [new.example]
status:
  conditions: []
"#;
    storage
        .update_one("HTTPRoute", Some("default"), "route", requested.to_string())
        .await
        .expect("resource update must succeed");

    let stored = storage
        .get_one("HTTPRoute", Some("default"), "route")
        .await
        .expect("updated resource must be readable");
    let stored: serde_yaml::Value = serde_yaml::from_str(&stored).expect("stored resource must be valid YAML");
    assert_eq!(stored["metadata"]["uid"], "uid-1");
    assert_eq!(stored["metadata"]["resourceVersion"], "43");
    assert_eq!(stored["metadata"]["generation"], 7);
    assert_eq!(stored["metadata"]["ownerReferences"][0]["name"], "owner");
    assert_eq!(stored["metadata"]["finalizers"][0], "edgion.io/finalizer");
    assert_eq!(stored["metadata"]["labels"]["new"], "label");
    assert!(stored["metadata"].get("annotations").is_none());
    assert_eq!(stored["spec"]["hostnames"][0], "new.example");
    assert!(stored["spec"].get("resolvedListeners").is_none());
    assert_eq!(stored["status"]["conditions"][0]["type"], "Accepted");
}

#[tokio::test]
async fn concurrent_updates_are_serialized_and_keep_server_state() {
    let tmp = tempfile::tempdir().expect("temporary directory must be created");
    let storage = Arc::new(FileSystemStorage::new(tmp.path()));
    let second_storage = Arc::new(FileSystemStorage::new(tmp.path()));
    assert!(std::ptr::eq(
        storage.update_lock("HTTPRoute", Some("default"), "route"),
        second_storage.update_lock("HTTPRoute", Some("default"), "route")
    ));
    let current = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: route
  namespace: default
  uid: uid-1
  resourceVersion: "42"
  finalizers: [edgion.io/finalizer]
spec:
  hostnames: [old.example]
  resolvedListeners: [{name: runtime-only}]
status:
  conditions: [{type: Accepted, status: "True"}]
"#;
    storage
        .create_one("HTTPRoute", Some("default"), "route", current.to_string())
        .await
        .expect("seed resource must be created");

    let mut tasks = Vec::new();
    for index in 0..32 {
        let storage = if index % 2 == 0 {
            Arc::clone(&storage)
        } else {
            Arc::clone(&second_storage)
        };
        tasks.push(tokio::spawn(async move {
                let requested = format!(
                    "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: route\n  namespace: default\n  resourceVersion: '{}'\n  labels:\n    writer: '{index}'\nspec:\n  hostnames: [route-{index}.example]\n",
                    100 + index
                );
                storage
                    .update_one("HTTPRoute", Some("default"), "route", requested)
                    .await
            }));
    }
    for task in tasks {
        task.await
            .expect("update task must not panic")
            .expect("concurrent update must succeed");
    }

    let stored = storage
        .get_one("HTTPRoute", Some("default"), "route")
        .await
        .expect("updated resource must be readable");
    let stored: serde_yaml::Value = serde_yaml::from_str(&stored).expect("stored resource must be valid YAML");
    assert_eq!(stored["metadata"]["uid"], "uid-1");
    assert_eq!(stored["metadata"]["finalizers"][0], "edgion.io/finalizer");
    assert_eq!(stored["status"]["conditions"][0]["type"], "Accepted");
    assert!(stored["spec"].get("resolvedListeners").is_none());
    let writer = stored["metadata"]["labels"]["writer"]
        .as_str()
        .expect("winning writer label must be present")
        .parse::<u64>()
        .expect("writer label must be numeric");
    assert_eq!(
        stored["metadata"]["resourceVersion"].as_str(),
        Some((100 + writer).to_string().as_str())
    );
}

#[tokio::test]
async fn stale_expected_resource_version_conflicts_without_writing() {
    let tmp = tempfile::tempdir().expect("temporary directory must be created");
    let storage = FileSystemStorage::new(tmp.path());
    let current = "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: route\n  namespace: default\n  resourceVersion: '42'\nspec: {hostnames: [current.example]}\n";
    storage
        .create_one("HTTPRoute", Some("default"), "route", current.to_string())
        .await
        .expect("seed resource must be created");

    let requested =
        "metadata: {name: route, namespace: default, resourceVersion: '43'}\nspec: {hostnames: [stale.example]}\n";
    let error = storage
        .update_one_if_version("HTTPRoute", Some("default"), "route", requested.to_string(), Some("41"))
        .await
        .expect_err("stale update must conflict");
    assert!(matches!(error, ConfWriterError::Conflict(_)));
    let stored = storage.get_one("HTTPRoute", Some("default"), "route").await.unwrap();
    let stored: serde_yaml::Value = serde_yaml::from_str(&stored).unwrap();
    assert_eq!(stored["metadata"]["resourceVersion"], "42");
    assert_eq!(stored["spec"]["hostnames"][0], "current.example");
}

#[tokio::test]
async fn stale_delete_precondition_conflicts_without_removing_file() {
    let tmp = tempfile::tempdir().expect("temporary directory must be created");
    let storage = FileSystemStorage::new(tmp.path());
    let current = "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: route\n  namespace: default\n  resourceVersion: '42'\nspec: {}\n";
    storage
        .create_one("HTTPRoute", Some("default"), "route", current.to_string())
        .await
        .expect("seed resource must be created");

    let error = storage
        .delete_one_if_version("HTTPRoute", Some("default"), "route", Some("41"))
        .await
        .expect_err("stale delete must conflict");
    assert!(matches!(error, ConfWriterError::Conflict(_)));
    assert!(storage.get_one("HTTPRoute", Some("default"), "route").await.is_ok());

    storage
        .delete_one_if_version("HTTPRoute", Some("default"), "route", Some("42"))
        .await
        .expect("matching delete precondition must succeed");
    assert!(matches!(
        storage.get_one("HTTPRoute", Some("default"), "route").await,
        Err(ConfWriterError::NotFound(_))
    ));
}
