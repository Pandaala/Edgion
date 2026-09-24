//! Process-lifetime inbound PROXY protocol policy enforcement.
//!
//! Listener sockets are frozen when Pingora starts. This module validates the
//! complete initial Gateway snapshot, prepares one matcher per physical
//! listener, and stops the process if a later snapshot would require different
//! socket semantics.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use edgion_algo::matcher::radix_ip::IpRadixMatcher;
use edgion_resources::listener_key::{resolve_listener_addr, ListenerKey};
use edgion_resources::resources::gateway::Listener;
use edgion_resources::resources::proxy_protocol::{
    CanonicalProxyProtocolTrustPolicy, ProxyProtocolConfigErrorCode, ResolvedInboundProxyProtocol,
};
use edgion_resources::{Gateway, ResourceMeta};
use parking_lot::Mutex;
use pingora_core::listeners::ConnectionFilter;
use pingora_core::protocols::l4::proxy_protocol::ProxyProtocolTrust;
use tokio::sync::watch;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProxyProtocolPolicyDeclaration {
    Disabled,
    Required,
    TrustedSources(Vec<String>),
}

impl From<&CanonicalProxyProtocolTrustPolicy> for ProxyProtocolPolicyDeclaration {
    fn from(policy: &CanonicalProxyProtocolTrustPolicy) -> Self {
        match policy {
            CanonicalProxyProtocolTrustPolicy::Required {} => Self::Required,
            CanonicalProxyProtocolTrustPolicy::TrustedSources { trusted_cidrs } => {
                Self::TrustedSources(trusted_cidrs.clone())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ContributorIdentity {
    gateway_key: String,
    listener_name: String,
}

#[derive(Clone, Debug)]
struct FrozenListenerPolicy {
    declaration: ProxyProtocolPolicyDeclaration,
    contributors: BTreeSet<ContributorIdentity>,
}

// Contributor identity is bounded diagnostic metadata, not part of the
// effective socket fingerprint. Adding or removing a redundant logical
// contributor with the same declaration does not require rebinding the socket.
impl PartialEq for FrozenListenerPolicy {
    fn eq(&self, other: &Self) -> bool {
        self.declaration == other.declaration
    }
}

impl Eq for FrozenListenerPolicy {}

type FrozenPolicySnapshot = BTreeMap<ListenerKey, FrozenListenerPolicy>;

/// Absent keys are `Disabled`: the socket is still bound with no PROXY header
/// parsing. Adding or removing a Disabled-only listener therefore does not
/// change socket semantics and must not take the whole process down. Required
/// and TrustedSources presence/absence remains fatal.
fn declaration_or_disabled(snapshot: &FrozenPolicySnapshot, key: &ListenerKey) -> ProxyProtocolPolicyDeclaration {
    snapshot
        .get(key)
        .map(|policy| policy.declaration.clone())
        .unwrap_or(ProxyProtocolPolicyDeclaration::Disabled)
}

fn equivalent_socket_semantics(left: &FrozenPolicySnapshot, right: &FrozenPolicySnapshot) -> bool {
    left.keys()
        .chain(right.keys())
        .all(|key| declaration_or_disabled(left, key) == declaration_or_disabled(right, key))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyProtocolFatalDiagnostic {
    pub code: &'static str,
    pub gateway_key: Option<String>,
    pub listener_name: Option<String>,
    pub conflicting_gateway_key: Option<String>,
    pub conflicting_listener_name: Option<String>,
    pub listener_key: Option<ListenerKey>,
}

impl fmt::Display for ProxyProtocolFatalDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "inbound PROXY protocol fatal ({})", self.code)?;
        if let Some(gateway_key) = &self.gateway_key {
            write!(f, ", gateway={gateway_key}")?;
        }
        if let Some(listener_name) = &self.listener_name {
            write!(f, ", listener={listener_name}")?;
        }
        if let Some(gateway_key) = &self.conflicting_gateway_key {
            write!(f, ", conflicting_gateway={gateway_key}")?;
        }
        if let Some(listener_name) = &self.conflicting_listener_name {
            write!(f, ", conflicting_listener={listener_name}")?;
        }
        if let Some(listener_key) = &self.listener_key {
            write!(f, ", listener_key={listener_key}")?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct PolicyRuntimeState {
    baseline: Option<FrozenPolicySnapshot>,
    latest: Option<std::result::Result<FrozenPolicySnapshot, ProxyProtocolFatalDiagnostic>>,
    fatal_diagnostic: Option<ProxyProtocolFatalDiagnostic>,
}

pub enum SnapshotDisposition {
    Publish,
    RejectFatal,
}

pub struct ProxyProtocolPolicyRuntime {
    fatal: AtomicBool,
    state: Mutex<PolicyRuntimeState>,
    fatal_tx: watch::Sender<bool>,
}

impl fmt::Debug for ProxyProtocolPolicyRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyProtocolPolicyRuntime")
            .field("fatal", &self.is_fatal())
            .finish_non_exhaustive()
    }
}

impl ProxyProtocolPolicyRuntime {
    pub fn new() -> Self {
        let (fatal_tx, _) = watch::channel(false);
        Self {
            fatal: AtomicBool::new(false),
            state: Mutex::new(PolicyRuntimeState::default()),
            fatal_tx,
        }
    }

    pub fn is_fatal(&self) -> bool {
        self.fatal.load(Ordering::Acquire)
    }

    pub fn fatal_diagnostic(&self) -> Option<ProxyProtocolFatalDiagnostic> {
        self.state.lock().fatal_diagnostic.clone()
    }

    pub fn subscribe_fatal(&self) -> watch::Receiver<bool> {
        self.fatal_tx.subscribe()
    }

    /// Record a prospective Gateway snapshot before its runtime stores are
    /// published. Initial lists establish only `latest`; startup preflight owns
    /// the baseline. Once frozen, a change in PROXY socket semantics is
    /// process-fatal. Disabled-only keys may appear or disappear.
    pub fn observe_snapshot<'a>(&self, gateways: impl IntoIterator<Item = &'a Gateway>) -> SnapshotDisposition {
        if self.is_fatal() {
            return SnapshotDisposition::RejectFatal;
        }
        let prospective = aggregate_snapshot(gateways);
        let mismatch = {
            let mut state = self.state.lock();
            state.latest = Some(prospective.clone());
            match &state.baseline {
                None => return SnapshotDisposition::Publish,
                Some(baseline) => match &prospective {
                    Ok(snapshot) if equivalent_socket_semantics(snapshot, baseline) => {
                        return SnapshotDisposition::Publish;
                    }
                    Ok(snapshot) => transition_diagnostic(baseline, snapshot),
                    Err(diagnostic) => diagnostic.clone(),
                },
            }
        };

        self.trip(mismatch);
        SnapshotDisposition::RejectFatal
    }

    /// Validate and freeze the initial full snapshot. The `latest` equality
    /// check closes the small race between copying ConfigClient data and arming
    /// the runtime guard: a concurrent update causes startup to fail instead of
    /// freezing stale socket policy.
    pub fn preflight_and_freeze<'a>(
        &self,
        gateways: impl IntoIterator<Item = &'a Gateway>,
    ) -> Result<PreparedProxyProtocolPolicies> {
        let snapshot = aggregate_snapshot(gateways).map_err(|diagnostic| anyhow!(diagnostic))?;
        let prepared = prepare_policies(&snapshot)?;

        let mut state = self.state.lock();
        if self.is_fatal() {
            let diagnostic = state
                .fatal_diagnostic
                .clone()
                .unwrap_or_else(|| diagnostic("PolicyFatal", None, None));
            bail!(diagnostic);
        }
        let latest = state
            .latest
            .as_ref()
            .ok_or_else(|| anyhow!("Gateway policy preflight ran before the initial Gateway full snapshot"))?;
        let latest_snapshot = match latest {
            Ok(latest) if equivalent_socket_semantics(latest, &snapshot) => latest.clone(),
            Ok(_) | Err(_) => {
                bail!("Gateway policy snapshot changed while startup preflight was being prepared")
            }
        };
        if let Some(baseline) = &state.baseline {
            if !equivalent_socket_semantics(baseline, &snapshot) {
                bail!("Gateway policy startup baseline was already frozen with different content")
            }
        } else {
            // Use the handler's newest semantically-equivalent snapshot so
            // contributor identities in later diagnostics are not stale.
            state.baseline = Some(latest_snapshot);
        }
        Ok(prepared)
    }

    fn trip(&self, diagnostic: ProxyProtocolFatalDiagnostic) {
        if self
            .fatal
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        {
            let mut state = self.state.lock();
            state.fatal_diagnostic = Some(diagnostic.clone());
        }
        self.fatal_tx.send_replace(true);
        let listener_key = diagnostic
            .listener_key
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        tracing::error!(
            component = "proxy_protocol_policy",
            error_code = diagnostic.code,
            gateway = diagnostic.gateway_key.as_deref().unwrap_or(""),
            listener = diagnostic.listener_name.as_deref().unwrap_or(""),
            conflicting_gateway = diagnostic.conflicting_gateway_key.as_deref().unwrap_or(""),
            conflicting_listener = diagnostic.conflicting_listener_name.as_deref().unwrap_or(""),
            listener_key = %listener_key,
            "Inbound PROXY protocol listener policy changed after startup; stopping the gateway"
        );
    }
}

impl Default for ProxyProtocolPolicyRuntime {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL_POLICY_RUNTIME: OnceLock<Arc<ProxyProtocolPolicyRuntime>> = OnceLock::new();

pub fn init_global_proxy_protocol_policy_runtime() -> Arc<ProxyProtocolPolicyRuntime> {
    GLOBAL_POLICY_RUNTIME
        .get_or_init(|| Arc::new(ProxyProtocolPolicyRuntime::new()))
        .clone()
}

pub fn get_global_proxy_protocol_policy_runtime() -> Arc<ProxyProtocolPolicyRuntime> {
    init_global_proxy_protocol_policy_runtime()
}

#[derive(Clone)]
pub enum PreparedProxyProtocolPolicy {
    Disabled,
    Required,
    TrustedSources {
        trust: Arc<dyn ProxyProtocolTrust>,
        cidr_count: usize,
    },
}

impl fmt::Debug for PreparedProxyProtocolPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => f.write_str("Disabled"),
            Self::Required => f.write_str("Required"),
            Self::TrustedSources { cidr_count, .. } => f
                .debug_struct("TrustedSources")
                .field("cidr_count", cidr_count)
                .finish(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PreparedProxyProtocolPolicies(BTreeMap<ListenerKey, PreparedProxyProtocolPolicy>);

impl PreparedProxyProtocolPolicies {
    pub fn get(&self, listener_key: &ListenerKey) -> Option<&PreparedProxyProtocolPolicy> {
        self.0.get(listener_key)
    }
}

#[derive(Debug)]
struct CidrProxyProtocolTrust(IpRadixMatcher);

impl ProxyProtocolTrust for CidrProxyProtocolTrust {
    fn is_trusted(&self, addr: &std::net::SocketAddr) -> bool {
        self.0.contains(&addr.ip())
    }
}

fn prepare_policies(snapshot: &FrozenPolicySnapshot) -> Result<PreparedProxyProtocolPolicies> {
    let mut prepared = BTreeMap::new();
    for (listener_key, policy) in snapshot {
        let runtime_policy = match &policy.declaration {
            ProxyProtocolPolicyDeclaration::Disabled => PreparedProxyProtocolPolicy::Disabled,
            ProxyProtocolPolicyDeclaration::Required => PreparedProxyProtocolPolicy::Required,
            ProxyProtocolPolicyDeclaration::TrustedSources(trusted_cidrs) => {
                let mut builder = IpRadixMatcher::builder();
                for cidr in trusted_cidrs {
                    builder.insert(cidr, "proxy-protocol-trusted").with_context(|| {
                        format!("failed to index inbound PROXY protocol policy for listener {listener_key}")
                    })?;
                }
                let matcher = builder.build().with_context(|| {
                    format!("failed to build inbound PROXY protocol matcher for listener {listener_key}")
                })?;
                PreparedProxyProtocolPolicy::TrustedSources {
                    trust: Arc::new(CidrProxyProtocolTrust(matcher)),
                    cidr_count: trusted_cidrs.len(),
                }
            }
        };
        prepared.insert(listener_key.clone(), runtime_policy);
    }
    Ok(PreparedProxyProtocolPolicies(prepared))
}

#[allow(clippy::result_large_err)]
fn aggregate_snapshot<'a>(
    gateways: impl IntoIterator<Item = &'a Gateway>,
) -> std::result::Result<FrozenPolicySnapshot, ProxyProtocolFatalDiagnostic> {
    let mut snapshot = BTreeMap::new();
    for gateway in gateways {
        add_gateway_contributions(&mut snapshot, gateway)?;
    }
    Ok(snapshot)
}

#[allow(clippy::result_large_err)]
fn add_gateway_contributions(
    snapshot: &mut FrozenPolicySnapshot,
    gateway: &Gateway,
) -> std::result::Result<(), ProxyProtocolFatalDiagnostic> {
    let gateway_key = gateway.key_name();
    let listeners = gateway.spec.listeners.as_deref().unwrap_or_default();
    let selected = match &gateway.spec.resolved_inbound_proxy_protocol {
        ResolvedInboundProxyProtocol::Disabled => BTreeSet::new(),
        ResolvedInboundProxyProtocol::Ready {
            listener_names, policy, ..
        } => {
            policy
                .validate_transport()
                .map_err(|_| diagnostic("InvalidTransportPolicy", Some(&gateway_key), None))?;
            if listener_names.is_empty() || listener_names.len() > listeners.len() {
                return Err(diagnostic("ImpossibleReadyState", Some(&gateway_key), None));
            }
            let selected: BTreeSet<&str> = listener_names.iter().map(String::as_str).collect();
            if selected.len() != listener_names.len()
                || selected.iter().any(|name| {
                    !listeners
                        .iter()
                        .any(|listener| listener.name == *name && is_tcp_listener(listener))
                })
            {
                return Err(diagnostic("ImpossibleReadyState", Some(&gateway_key), None));
            }
            selected
        }
        ResolvedInboundProxyProtocol::Invalid {
            code, listener_names, ..
        } => {
            return Err(invalid_state_diagnostic(&gateway_key, *code, listener_names.first()));
        }
    };

    let ready_policy = match &gateway.spec.resolved_inbound_proxy_protocol {
        ResolvedInboundProxyProtocol::Ready { policy, .. } => Some(ProxyProtocolPolicyDeclaration::from(policy)),
        ResolvedInboundProxyProtocol::Disabled => None,
        ResolvedInboundProxyProtocol::Invalid { .. } => unreachable!("invalid state returned above"),
    };

    for listener in listeners.iter().filter(|listener| is_tcp_listener(listener)) {
        let declaration = if selected.contains(listener.name.as_str()) {
            ready_policy
                .clone()
                .ok_or_else(|| diagnostic("ImpossibleReadyState", Some(&gateway_key), Some(&listener.name)))?
        } else {
            ProxyProtocolPolicyDeclaration::Disabled
        };
        let listener_key = ListenerKey::new(
            resolve_listener_addr(gateway.spec.addresses.as_ref(), listener.port),
            &listener.protocol,
        );
        let contributor = ContributorIdentity {
            gateway_key: gateway_key.clone(),
            listener_name: listener.name.clone(),
        };

        match snapshot.get_mut(&listener_key) {
            Some(existing) if existing.declaration != declaration => {
                let conflicting = existing.contributors.first();
                return Err(ProxyProtocolFatalDiagnostic {
                    code: "SharedListenerPolicyConflict",
                    gateway_key: Some(gateway_key),
                    listener_name: Some(listener.name.clone()),
                    conflicting_gateway_key: conflicting.map(|identity| identity.gateway_key.clone()),
                    conflicting_listener_name: conflicting.map(|identity| identity.listener_name.clone()),
                    listener_key: Some(listener_key),
                });
            }
            Some(existing) => {
                existing.contributors.insert(contributor);
            }
            None => {
                snapshot.insert(
                    listener_key,
                    FrozenListenerPolicy {
                        declaration,
                        contributors: BTreeSet::from([contributor]),
                    },
                );
            }
        }
    }
    Ok(())
}

fn transition_diagnostic(
    baseline: &FrozenPolicySnapshot,
    prospective: &FrozenPolicySnapshot,
) -> ProxyProtocolFatalDiagnostic {
    let changed_key = baseline
        .keys()
        .chain(prospective.keys())
        .find(|key| declaration_or_disabled(baseline, key) != declaration_or_disabled(prospective, key))
        .cloned();
    let contributor = changed_key
        .as_ref()
        .and_then(|key| prospective.get(key).or_else(|| baseline.get(key)))
        .and_then(|policy| policy.contributors.first());
    ProxyProtocolFatalDiagnostic {
        code: "ListenerPolicyTransition",
        gateway_key: contributor.map(|identity| identity.gateway_key.clone()),
        listener_name: contributor.map(|identity| identity.listener_name.clone()),
        conflicting_gateway_key: None,
        conflicting_listener_name: None,
        listener_key: changed_key,
    }
}

fn invalid_state_diagnostic(
    gateway_key: &str,
    code: ProxyProtocolConfigErrorCode,
    listener_name: Option<&String>,
) -> ProxyProtocolFatalDiagnostic {
    ProxyProtocolFatalDiagnostic {
        code: match code {
            ProxyProtocolConfigErrorCode::AnnotationTooLarge => "AnnotationTooLarge",
            ProxyProtocolConfigErrorCode::MalformedJson => "MalformedJson",
            ProxyProtocolConfigErrorCode::InvalidShape => "InvalidShape",
            ProxyProtocolConfigErrorCode::InvalidListenerSelection => "InvalidListenerSelection",
            ProxyProtocolConfigErrorCode::InvalidPolicy => "InvalidPolicy",
            ProxyProtocolConfigErrorCode::InvalidReference => "InvalidReference",
            ProxyProtocolConfigErrorCode::ReferenceNotFound => "ReferenceNotFound",
            ProxyProtocolConfigErrorCode::ReferenceNotPermitted => "ReferenceNotPermitted",
            ProxyProtocolConfigErrorCode::ReferenceDisabled => "ReferenceDisabled",
            ProxyProtocolConfigErrorCode::ReferenceActiveUnsupported => "ReferenceActiveUnsupported",
            ProxyProtocolConfigErrorCode::ReferenceWrongType => "ReferenceWrongType",
            ProxyProtocolConfigErrorCode::ReferenceInvalidPolicy => "ReferenceInvalidPolicy",
            ProxyProtocolConfigErrorCode::SharedListenerConflict => "SharedListenerConflict",
            ProxyProtocolConfigErrorCode::InternalInvariant => "InternalInvariant",
        },
        gateway_key: Some(gateway_key.to_string()),
        listener_name: listener_name.cloned(),
        conflicting_gateway_key: None,
        conflicting_listener_name: None,
        listener_key: None,
    }
}

fn diagnostic(
    code: &'static str,
    gateway_key: Option<&str>,
    listener_name: Option<&str>,
) -> ProxyProtocolFatalDiagnostic {
    ProxyProtocolFatalDiagnostic {
        code,
        gateway_key: gateway_key.map(str::to_string),
        listener_name: listener_name.map(str::to_string),
        conflicting_gateway_key: None,
        conflicting_listener_name: None,
        listener_key: None,
    }
}

fn is_tcp_listener(listener: &Listener) -> bool {
    matches!(
        listener.protocol.to_ascii_uppercase().as_str(),
        "HTTP" | "HTTPS" | "TCP" | "TLS"
    )
}

/// Always-installed TCP filter. The process-fatal guard is checked before an
/// optional StreamPlugins filter, so no plugin code runs after the socket
/// policy has diverged.
pub struct GuardFirstConnectionFilter {
    runtime: Arc<ProxyProtocolPolicyRuntime>,
    stream_filter: Option<Arc<dyn ConnectionFilter>>,
}

impl GuardFirstConnectionFilter {
    pub fn new(runtime: Arc<ProxyProtocolPolicyRuntime>, stream_filter: Option<Arc<dyn ConnectionFilter>>) -> Self {
        Self { runtime, stream_filter }
    }
}

impl fmt::Debug for GuardFirstConnectionFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuardFirstConnectionFilter")
            .field("fatal", &self.runtime.is_fatal())
            .field("has_stream_filter", &self.stream_filter.is_some())
            .finish()
    }
}

#[async_trait]
impl ConnectionFilter for GuardFirstConnectionFilter {
    async fn should_accept(&self, addr: Option<&std::net::SocketAddr>) -> bool {
        if self.runtime.is_fatal() {
            return false;
        }
        match &self.stream_filter {
            Some(filter) => filter.should_accept(addr).await,
            None => true,
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/runtime/server/proxy_protocol_policy_test.rs"]
mod tests;
