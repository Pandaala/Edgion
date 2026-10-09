//! Projects Gateway-attached resources into one dedicated data-plane process.

use edgion_resources::resources::refs::gateway_process_locator::GatewayProcessLocator;
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::prelude_resources::{EdgionAcme, EdgionTls, GRPCRoute, HTTPRoute, TCPRoute, TLSRoute, UDPRoute};
use edgion_resources::resources::common::ParentReference;

pub(super) trait ScopedResource: Clone + Send + Sync + 'static {
    fn project_for_gateway(&self, scope: &str) -> Option<Self>;
    fn project_for_shared(&self, dedicated: &HashSet<String>) -> Option<Self>;
}

pub(super) trait RefreshShared: Send + Sync {
    fn refresh(&self);
}

#[derive(Default)]
pub(crate) struct SharedScopeRegistry {
    dedicated: RwLock<HashSet<String>>,
    active: RwLock<bool>,
    handlers: RwLock<Vec<Weak<dyn RefreshShared>>>,
}

impl SharedScopeRegistry {
    pub(crate) fn snapshot(&self) -> HashSet<String> {
        self.dedicated.read().clone()
    }

    pub(crate) fn replace(&self, dedicated: HashSet<String>) {
        if *self.dedicated.read() == dedicated {
            return;
        }
        *self.dedicated.write() = dedicated;
        for handler in self.handlers.read().iter().filter_map(Weak::upgrade) {
            handler.refresh();
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        *self.active.read()
    }

    pub(crate) fn set_active(&self, active: bool) {
        if *self.active.read() == active {
            return;
        }
        *self.active.write() = active;
        for handler in self.handlers.read().iter().filter_map(Weak::upgrade) {
            handler.refresh();
        }
    }
}

pub(super) struct ScopedHandler<T> {
    inner: Box<dyn ConfHandler<T> + Send + Sync>,
    scope: Option<String>,
    shared: Arc<SharedScopeRegistry>,
    source: RwLock<HashMap<String, Arc<T>>>,
    publication: Mutex<()>,
}

impl<T> ScopedHandler<T> {
    pub(super) fn wrap(
        inner: Box<dyn ConfHandler<T> + Send + Sync>,
        scope: Option<String>,
        shared: Arc<SharedScopeRegistry>,
    ) -> Box<dyn ConfHandler<T> + Send + Sync>
    where
        T: ScopedResource,
    {
        let handler = Arc::new(Self {
            inner,
            scope,
            shared: Arc::clone(&shared),
            source: RwLock::new(HashMap::new()),
            publication: Mutex::new(()),
        });
        let refresh: Arc<dyn RefreshShared> = handler.clone();
        shared.handlers.write().push(Arc::downgrade(&refresh));
        Box::new(ArcHandler(handler))
    }
}

struct ArcHandler<T>(Arc<ScopedHandler<T>>);

impl<T: ScopedResource> ConfHandler<T> for ArcHandler<T> {
    fn full_set(&self, data: &HashMap<String, Arc<T>>) {
        self.0.full_set(data);
    }
    fn partial_update(&self, add: HashMap<String, Arc<T>>, update: HashMap<String, Arc<T>>, remove: HashSet<String>) {
        self.0.partial_update(add, update, remove);
    }
}

impl<T: ScopedResource> RefreshShared for ScopedHandler<T> {
    fn refresh(&self) {
        let _publication = self.publication.lock();
        let source = self.source.read();
        let projected = match self.scope.as_deref() {
            Some(scope) if self.shared.is_active() => project_gateway(&source, scope),
            Some(_) => HashMap::new(),
            None => project_shared(&source, &self.shared.snapshot()),
        };
        self.inner.full_set(&projected);
    }
}

fn project_gateway<T: ScopedResource>(data: &HashMap<String, Arc<T>>, scope: &str) -> HashMap<String, Arc<T>> {
    data.iter()
        .filter_map(|(key, resource)| {
            resource
                .project_for_gateway(scope)
                .map(|value| (key.clone(), Arc::new(value)))
        })
        .collect()
}

fn project_shared<T: ScopedResource>(
    data: &HashMap<String, Arc<T>>,
    dedicated: &HashSet<String>,
) -> HashMap<String, Arc<T>> {
    data.iter()
        .filter_map(|(key, resource)| {
            resource
                .project_for_shared(dedicated)
                .map(|value| (key.clone(), Arc::new(value)))
        })
        .collect()
}

impl<T: ScopedResource> ConfHandler<T> for ScopedHandler<T> {
    fn full_set(&self, data: &HashMap<String, Arc<T>>) {
        let _publication = self.publication.lock();
        let Some(scope) = self.scope.as_deref() else {
            *self.source.write() = data.clone();
            self.inner.full_set(&project_shared(data, &self.shared.snapshot()));
            return;
        };
        *self.source.write() = data.clone();
        if !self.shared.is_active() {
            self.inner.full_set(&HashMap::new());
            return;
        }
        let projected = project_gateway(data, scope);
        self.inner.full_set(&projected);
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<T>>,
        update: HashMap<String, Arc<T>>,
        mut remove: HashSet<String>,
    ) {
        let _publication = self.publication.lock();
        let Some(scope) = self.scope.as_deref() else {
            {
                let mut source = self.source.write();
                for (key, value) in add.iter().chain(update.iter()) {
                    source.insert(key.clone(), Arc::clone(value));
                }
                for key in &remove {
                    source.remove(key);
                }
            }
            let dedicated = self.shared.snapshot();
            let mut project = |data: HashMap<String, Arc<T>>| {
                data.into_iter()
                    .filter_map(|(key, resource)| match resource.project_for_shared(&dedicated) {
                        Some(value) => Some((key, Arc::new(value))),
                        None => {
                            remove.insert(key);
                            None
                        }
                    })
                    .collect()
            };
            let add = project(add);
            let update = project(update);
            self.inner.partial_update(add, update, remove);
            return;
        };
        {
            let mut source = self.source.write();
            for (key, value) in add.iter().chain(update.iter()) {
                source.insert(key.clone(), Arc::clone(value));
            }
            for key in &remove {
                source.remove(key);
            }
        }
        if !self.shared.is_active() {
            self.inner.partial_update(
                HashMap::new(),
                HashMap::new(),
                add.keys().chain(update.keys()).cloned().chain(remove).collect(),
            );
            return;
        }
        let mut project = |data: HashMap<String, Arc<T>>| {
            data.into_iter()
                .filter_map(|(key, resource)| match resource.project_for_gateway(scope) {
                    Some(resource) => Some((key, Arc::new(resource))),
                    None => {
                        remove.insert(key);
                        None
                    }
                })
                .collect()
        };
        let add = project(add);
        let update = project(update);
        self.inner.partial_update(add, update, remove);
    }
}

fn parent_matches(parent: &ParentReference, route_namespace: &str, scope: &str) -> bool {
    let Ok(Some(locator)) = GatewayProcessLocator::from_process(Some(scope), None) else {
        return false;
    };
    parent.group.as_deref().unwrap_or("gateway.networking.k8s.io") == "gateway.networking.k8s.io"
        && parent.kind.as_deref().unwrap_or("Gateway") == "Gateway"
        && parent.namespace.as_deref().unwrap_or(route_namespace) == locator.namespace()
        && parent.name == locator.name()
}

fn retain_parent_refs(parent_refs: &mut Option<Vec<ParentReference>>, route_namespace: &str, scope: &str) -> bool {
    let Some(parent_refs) = parent_refs.as_mut() else {
        return false;
    };
    parent_refs.retain(|parent| parent_matches(parent, route_namespace, scope));
    !parent_refs.is_empty()
}

fn retain_shared_parents(
    parent_refs: &mut Option<Vec<ParentReference>>,
    namespace: &str,
    dedicated: &HashSet<String>,
) -> bool {
    let Some(parents) = parent_refs.as_mut() else {
        return false;
    };
    parents.retain(|parent| {
        if parent.group.as_deref().unwrap_or("gateway.networking.k8s.io") != "gateway.networking.k8s.io"
            || parent.kind.as_deref().unwrap_or("Gateway") != "Gateway"
        {
            return true;
        }
        let key = format!("{}/{}", parent.namespace.as_deref().unwrap_or(namespace), parent.name);
        !dedicated.contains(&key)
    });
    !parents.is_empty()
}

macro_rules! impl_http_route_scope {
    ($ty:ty) => {
        impl ScopedResource for $ty {
            fn project_for_shared(&self, dedicated: &HashSet<String>) -> Option<Self> {
                let mut projected = self.clone();
                let namespace = self.metadata.namespace.as_deref().unwrap_or("default");
                retain_shared_parents(&mut projected.spec.parent_refs, namespace, dedicated).then_some(projected)
            }
            fn project_for_gateway(&self, scope: &str) -> Option<Self> {
                let mut projected = self.clone();
                let namespace = self.metadata.namespace.as_deref().unwrap_or("default");
                retain_parent_refs(&mut projected.spec.parent_refs, namespace, scope).then_some(projected)
            }
        }
    };
}

impl_http_route_scope!(HTTPRoute);
impl_http_route_scope!(GRPCRoute);

macro_rules! impl_l4_route_scope {
    ($ty:ty) => {
        impl ScopedResource for $ty {
            fn project_for_shared(&self, dedicated: &HashSet<String>) -> Option<Self> {
                let mut projected = self.clone();
                let namespace = self.metadata.namespace.as_deref().unwrap_or("default");
                if !retain_shared_parents(&mut projected.spec.parent_refs, namespace, dedicated) {
                    return None;
                }
                let attachments = projected.spec.resolved_listener_attachments.as_mut()?;
                attachments.retain(|attachment| {
                    !dedicated.contains(&format!(
                        "{}/{}",
                        attachment.gateway_namespace, attachment.gateway_name
                    ))
                });
                if attachments.is_empty() {
                    return None;
                }
                let mut keys: Vec<_> = attachments
                    .iter()
                    .map(|attachment| attachment.listener_key.clone())
                    .collect();
                keys.sort();
                keys.dedup();
                projected.spec.resolved_listeners = Some(keys);
                Some(projected)
            }
            fn project_for_gateway(&self, scope: &str) -> Option<Self> {
                let mut projected = self.clone();
                let namespace = self.metadata.namespace.as_deref().unwrap_or("default");
                if !retain_parent_refs(&mut projected.spec.parent_refs, namespace, scope) {
                    return None;
                }
                let attachments = projected.spec.resolved_listener_attachments.as_mut()?;
                attachments.retain(|attachment| {
                    format!("{}/{}", attachment.gateway_namespace, attachment.gateway_name) == scope
                });
                if attachments.is_empty() {
                    return None;
                }
                let mut keys: Vec<_> = attachments
                    .iter()
                    .map(|attachment| attachment.listener_key.clone())
                    .collect();
                keys.sort();
                keys.dedup();
                projected.spec.resolved_listeners = Some(keys);
                Some(projected)
            }
        }
    };
}

impl_l4_route_scope!(TCPRoute);
impl_l4_route_scope!(UDPRoute);

impl ScopedResource for TLSRoute {
    fn project_for_shared(&self, dedicated: &HashSet<String>) -> Option<Self> {
        let mut projected = self.clone();
        let namespace = self.metadata.namespace.as_deref().unwrap_or("default");
        if !retain_shared_parents(&mut projected.spec.parent_refs, namespace, dedicated) {
            return None;
        }
        let listeners = projected.spec.resolved_listeners.as_mut()?;
        listeners.retain(|listener| {
            !dedicated.contains(&format!("{}/{}", listener.gateway_namespace, listener.gateway_name))
        });
        (!listeners.is_empty()).then_some(projected)
    }
    fn project_for_gateway(&self, scope: &str) -> Option<Self> {
        let mut projected = self.clone();
        let namespace = self.metadata.namespace.as_deref().unwrap_or("default");
        if !retain_parent_refs(&mut projected.spec.parent_refs, namespace, scope) {
            return None;
        }
        let listeners = projected.spec.resolved_listeners.as_mut()?;
        listeners.retain(|listener| format!("{}/{}", listener.gateway_namespace, listener.gateway_name) == scope);
        (!listeners.is_empty()).then_some(projected)
    }
}

impl ScopedResource for EdgionTls {
    fn project_for_shared(&self, dedicated: &HashSet<String>) -> Option<Self> {
        let mut projected = self.clone();
        let namespace = self.metadata.namespace.as_deref().unwrap_or("default");
        if !retain_shared_parents(&mut projected.spec.parent_refs, namespace, dedicated) {
            return None;
        }
        let attachments = projected.spec.resolved_listener_attachments.as_mut()?;
        attachments.retain(|attachment| {
            !dedicated.contains(&format!("{}/{}", attachment.gateway_namespace, attachment.gateway_name))
        });
        if attachments.is_empty() {
            return None;
        }
        let mut keys: Vec<_> = attachments
            .iter()
            .map(|attachment| attachment.listener_key.clone())
            .collect();
        keys.sort();
        keys.dedup();
        projected.spec.resolved_listeners = Some(keys);
        Some(projected)
    }
    fn project_for_gateway(&self, scope: &str) -> Option<Self> {
        let mut projected = self.clone();
        let namespace = self.metadata.namespace.as_deref().unwrap_or("default");
        if !retain_parent_refs(&mut projected.spec.parent_refs, namespace, scope) {
            return None;
        }
        let attachments = projected.spec.resolved_listener_attachments.as_mut()?;
        attachments
            .retain(|attachment| format!("{}/{}", attachment.gateway_namespace, attachment.gateway_name) == scope);
        if attachments.is_empty() {
            return None;
        }
        let mut keys: Vec<_> = attachments
            .iter()
            .map(|attachment| attachment.listener_key.clone())
            .collect();
        keys.sort();
        keys.dedup();
        projected.spec.resolved_listeners = Some(keys);
        Some(projected)
    }
}

impl ScopedResource for EdgionAcme {
    fn project_for_shared(&self, dedicated: &HashSet<String>) -> Option<Self> {
        let mut projected = self.clone();
        let attachments = projected.spec.resolved_listener_attachments.as_mut()?;
        attachments.retain(|attachment| {
            !dedicated.contains(&format!("{}/{}", attachment.gateway_namespace, attachment.gateway_name))
        });
        (!attachments.is_empty()).then_some(projected)
    }
    fn project_for_gateway(&self, scope: &str) -> Option<Self> {
        let mut projected = self.clone();
        let attachments = projected.spec.resolved_listener_attachments.as_mut()?;
        attachments
            .retain(|attachment| format!("{}/{}", attachment.gateway_namespace, attachment.gateway_name) == scope);
        (!attachments.is_empty()).then_some(projected)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/conf_sync/conf_client/scoped_handler_test.rs"]
mod tests;
