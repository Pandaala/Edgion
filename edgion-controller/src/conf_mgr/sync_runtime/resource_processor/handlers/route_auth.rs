//! Internal HTTPRoute auth: declaring-field authority and root-only recovery.
use super::super::credential::ca_refs::configmap_ca_material;
use super::super::credential::pipeline::{bound_configmap_status_result, bound_secret_status_result};
use super::super::credential::request_secrets::resolve_request_secrets_readonly;
use super::super::{HandlerContext, ResolvedRefsError, ResourceRef};
use edgion_resources::resources::common::ResolvedSecrets;
use edgion_resources::resources::http_route::{HTTPRouteExternalAuth, HTTPRouteFilter, HTTPRouteRule};
use edgion_resources::resources::refs::route_auth_fields::{
    AuthoredRouteAuthField, HttpRouteAuthRefs, PreparedRouteAuthField, ResolvedExternalAuth,
};
use edgion_resources::resources::refs::route_fields::RouteFilterSlot;
use edgion_resources::resources::refs::{ReferenceError, ReferenceToken};
use edgion_resources::{HTTPRoute, ResourceKind};
use k8s_openapi::api::core::v1::Secret;

fn owner(route: &HTTPRoute) -> ResourceRef {
    ResourceRef::new(
        ResourceKind::HTTPRoute,
        route.metadata.namespace.clone(),
        route.metadata.name.clone().unwrap_or_default(),
    )
}
fn filters_mut(rule: &mut HTTPRouteRule) -> impl Iterator<Item = &mut HTTPRouteFilter> {
    rule.filters.iter_mut().flatten().chain(
        rule.backend_refs
            .iter_mut()
            .flatten()
            .flat_map(|backend| backend.filters.iter_mut().flatten()),
    )
}
fn scrub_filter(filter: &mut HTTPRouteFilter) {
    filter.resolved_external_auth = None;
    if let Some(HTTPRouteExternalAuth::Edgion(config)) = &mut filter.external_auth {
        config.resolved_secrets = None;
        if let Some(tls) = &mut config.conn.tls {
            tls.resolved_ca_certificates = None;
            tls.resolved_client_certificate = None;
        }
    }
}
pub(super) fn scrub_rule(rule: &mut HTTPRouteRule) {
    for filter in filters_mut(rule) {
        scrub_filter(filter);
    }
}
pub(super) fn scrub_route(route: &mut HTTPRoute) {
    for rule in route.spec.rules.iter_mut().flatten() {
        scrub_rule(rule);
    }
    for rule in route.spec.resolved_rules.iter_mut().flatten() {
        scrub_rule(rule);
    }
}

/// Private construction keeps a root wakeup from substituting for field authority.
struct RootReceipt<'a> {
    root: &'a HTTPRoute,
}
impl<'a> RootReceipt<'a> {
    fn new(root: &'a HTTPRoute) -> Result<Self, ReferenceError> {
        HttpRouteAuthRefs::from_route(root)?;
        Ok(Self { root })
    }
    fn register(&self, ctx: &HandlerContext, token: ReferenceToken<'_>) {
        let Ok(reference) = token.validated() else { return };
        let target = reference.target();
        let Some(namespace) = target.scope().namespace() else {
            return;
        };
        if target.group_kind().group() != "" {
            return;
        }
        let key = match target.group_kind().kind() {
            "Secret" => format!("{namespace}/{}", target.name()),
            "ConfigMap" => format!(
                "{}{namespace}/{}",
                super::super::CONFIG_MAP_REF_KEY_PREFIX,
                target.name()
            ),
            _ => return,
        };
        ctx.secret_ref_manager().add_ref(key, owner(self.root));
        if !reference.is_same_namespace() {
            ctx.runtime()
                .cross_ns_ref_mgr
                .add_ref(namespace.to_string(), owner(self.root));
        }
    }
}

#[derive(Clone)]
pub struct AuthDecision {
    slot: RouteFilterSlot,
    stamp: Option<ResolvedExternalAuth>,
    ca: Option<Vec<Secret>>,
    client: Option<Secret>,
    secrets: Option<ResolvedSecrets>,
    pub(super) errors: Vec<ResolvedRefsError>,
}
impl AuthDecision {
    pub(super) fn unavailable(field: &AuthoredRouteAuthField<'_>) -> Self {
        Self {
            slot: field.slot(),
            stamp: Some(field.resolution(Some("HTTPRoute authentication resolution is unavailable".into()))),
            ca: None,
            client: None,
            secrets: None,
            errors: vec![ResolvedRefsError::DecodeError {
                key: "externalAuth",
                detail: "HTTPRoute authentication resolution is unavailable".into(),
            }],
        }
    }
    fn invalid(slot: RouteFilterSlot, error: ReferenceError) -> Self {
        Self {
            slot,
            stamp: None,
            ca: None,
            client: None,
            secrets: None,
            errors: vec![ResolvedRefsError::InvalidReference { error }],
        }
    }
    pub(super) fn slot(&self) -> RouteFilterSlot {
        self.slot
    }
    pub(super) fn apply(self, rule: &mut HTTPRouteRule) {
        let filters = match self.slot.backend() {
            None => rule.filters.as_mut(),
            Some(index) => rule
                .backend_refs
                .as_mut()
                .and_then(|backends| backends.get_mut(index))
                .and_then(|backend| backend.filters.as_mut()),
        };
        let Some(filter) = filters.and_then(|filters| filters.get_mut(self.slot.filter())) else {
            return;
        };
        scrub_filter(filter);
        filter.resolved_external_auth = self.stamp;
        if let Some(HTTPRouteExternalAuth::Edgion(config)) = &mut filter.external_auth {
            config.resolved_secrets = self.secrets;
            if let Some(tls) = &mut config.conn.tls {
                tls.resolved_ca_certificates = self.ca;
                tls.resolved_client_certificate = self.client;
            }
        }
    }
}

pub(super) fn resolve(
    root: &HTTPRoute,
    field: &AuthoredRouteAuthField<'_>,
    ctx: &HandlerContext,
    register: bool,
) -> AuthDecision {
    let receipt = match RootReceipt::new(root) {
        Ok(receipt) => receipt,
        Err(error) => return AuthDecision::invalid(field.slot(), error),
    };
    let declaring_owner = owner(field.route());
    if register {
        for token in field.ca_certificates().chain(field.client_certificate()) {
            receipt.register(ctx, token);
        }
        for reference in field
            .request_secrets()
            .filter(|reference| !reference.data_key().is_empty())
        {
            receipt.register(ctx, *reference.reference());
        }
    }
    let mut errors = Vec::new();
    let mut ca = Vec::new();
    for token in field.ca_certificates() {
        let target = match token.validated() {
            Ok(reference) => reference.target(),
            Err(error) => {
                errors.push(ResolvedRefsError::InvalidReference { error });
                continue;
            }
        };
        let result = if target.group_kind().kind() == "ConfigMap" {
            bound_configmap_status_result(ctx, &declaring_owner, token).and_then(|configmap| {
                configmap_ca_material(&configmap).ok_or(ResolvedRefsError::DecodeError {
                    key: "ca.crt",
                    detail: "CA ConfigMap has no certificate material".into(),
                })
            })
        } else {
            bound_secret_status_result(ctx, &declaring_owner, token).map(|authorized| authorized.secret)
        };
        match result {
            Ok(material) => ca.push(material),
            Err(error) => errors.push(error),
        }
    }
    let client =
        field
            .client_certificate()
            .and_then(|token| match bound_secret_status_result(ctx, &declaring_owner, token) {
                Ok(material) => Some(material.secret),
                Err(error) => {
                    errors.push(error);
                    None
                }
            });
    let (secrets, template_errors) = resolve_request_secrets_readonly(ctx, &declaring_owner, field.request_secrets());
    errors.extend(template_errors);
    let stamp = field
        .resolution((!errors.is_empty()).then(|| "HTTPRoute authentication references are unresolved".to_string()));
    AuthDecision {
        slot: field.slot(),
        stamp: Some(stamp),
        ca: (!ca.is_empty()).then_some(ca),
        client,
        secrets: (!secrets.is_empty()).then_some(ResolvedSecrets(secrets)),
        errors,
    }
}

pub(super) fn decisions(
    route: &HTTPRoute,
    mut resolve: impl FnMut(&AuthoredRouteAuthField<'_>) -> AuthDecision,
) -> Vec<AuthDecision> {
    let Ok(references) = HttpRouteAuthRefs::from_route(route) else {
        return Vec::new();
    };
    references
        .authored_slots()
        .into_iter()
        .map(|slot| {
            let prepared = match PreparedRouteAuthField::from_route(route, slot) {
                Ok(prepared) => prepared,
                Err(error) => return AuthDecision::invalid(slot, error),
            };
            match prepared.bind(route) {
                Ok(field) => resolve(&field),
                Err(error) => AuthDecision::invalid(slot, error),
            }
        })
        .collect()
}

/// Collect every declared field before per-rule drops, then move decisions only
/// into that declaring rule. The original field position never follows a clone.
pub(super) fn by_rule(
    route: &HTTPRoute,
    resolve: impl FnMut(&AuthoredRouteAuthField<'_>) -> AuthDecision,
) -> std::collections::HashMap<usize, Vec<AuthDecision>> {
    let mut result: std::collections::HashMap<usize, Vec<AuthDecision>> = std::collections::HashMap::new();
    for decision in decisions(route, resolve) {
        result.entry(decision.slot.rule()).or_default().push(decision);
    }
    result
}

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/route_auth_test.rs"]
mod tests;
