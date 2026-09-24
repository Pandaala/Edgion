use super::*;

/// Persist status with an atomic precondition on the raw input resourceVersion.
///
/// Uses DynamicObject to avoid the Scope type constraint issue.
/// The `ApiResource` uses the selected GatewayClass transport when supplied;
/// other kinds use K's `#[kube]`-derived GVK and plural metadata. All resource
/// kinds inherit the guard. A conflict ends this attempt; recovery uses
/// subsequent source events/requeues, never a fresh version attached to stale status.
#[cfg(test)]
pub(super) async fn persist_k8s_status<K>(
    client: &Client,
    api_scope: &ApiScope,
    namespace: Option<&str>,
    name: &str,
    resource_version: Option<&str>,
    status_value: &serde_json::Value,
) -> Result<(), StatusWriteError>
where
    K: Resource + Clone + Debug + DeserializeOwned + Serialize,
    K::DynamicType: Default,
{
    persist_k8s_status_version::<K>(client, api_scope, None, namespace, name, resource_version, status_value).await
}
