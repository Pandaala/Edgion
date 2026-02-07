//! ACME Service - Background orchestrator for certificate issuance and renewal
//!
//! The AcmeService is the central coordinator that:
//! 1. Receives notifications from EdgionAcmeHandler when resources change
//! 2. Drives the full ACME flow (account → order → challenge → certificate)
//! 3. Creates/updates K8s Secrets with issued certificates
//! 4. Creates/updates EdgionTls resources for Gateway TLS termination
//! 5. Pushes HTTP-01 challenge tokens via CRD spec (propagated to Gateway via gRPC)
//! 6. Schedules periodic renewal checks
//!
//! ## Design
//!
//! - Global singleton via `Mutex<Option<...>>` + `mpsc::channel` (re-initializable)
//! - In-memory tracking of in-flight operations (prevents duplicate processing)
//! - K8s CRD as the source of truth (challenge tokens, status, account)
//! - Account credentials persisted in a dedicated K8s Secret

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use kube::api::{Api, Patch, PatchParams};
use kube::Client;
use tokio::sync::mpsc;

use super::acme_client::{AcmeClient, AcmeCertificateResult};
use super::dns_provider::create_dns_provider;
use crate::types::resources::edgion_acme::{
    AcmeCertPhase, AcmeChallengeType, ActiveHttpChallenge, EdgionAcme, EdgionAcmeStatus,
};

// ============================================================================
// Global singleton (re-initializable for controller re-election)
// ============================================================================

static ACME_SERVICE: Mutex<Option<AcmeServiceHandle>> = Mutex::new(None);

/// Handle to send commands to the ACME service
struct AcmeServiceHandle {
    tx: mpsc::Sender<AcmeCommand>,
}

/// Commands that can be sent to the ACME service
#[derive(Debug)]
enum AcmeCommand {
    /// An EdgionAcme resource was created or updated
    ResourceChanged { key: String },
    /// Trigger a renewal check for a specific resource
    RenewalCheck { key: String },
}

/// Initialize and start the global ACME service.
///
/// Called during controller startup (after caches are ready).
/// Must call `stop_acme_service()` before calling this again (e.g. on re-election).
///
/// On startup, the service performs a full scan of all EdgionAcme resources
/// to recover renewal timers that were lost during controller restart.
pub fn start_acme_service(client: Client) {
    let mut guard = ACME_SERVICE.lock().unwrap();
    if guard.is_some() {
        tracing::warn!(
            component = "acme_service",
            "ACME service already running, call stop_acme_service() first"
        );
        return;
    }

    let (tx, rx) = mpsc::channel::<AcmeCommand>(256);
    *guard = Some(AcmeServiceHandle { tx });
    drop(guard); // Release lock before spawning tasks

    let service = AcmeServiceWorker::new(client.clone());
    tokio::spawn(async move {
        service.run(rx).await;
    });

    // Spawn startup scan: re-schedule renewal checks for all existing EdgionAcme resources.
    // This recovers timers lost during controller restart.
    let startup_client = client;
    tokio::spawn(async move {
        // Wait a bit for processors to finish initial sync
        tokio::time::sleep(Duration::from_secs(10)).await;
        scan_all_acme_resources(startup_client).await;
    });

    tracing::info!(component = "acme_service", "ACME service started");
}

/// Stop the global ACME service.
///
/// Called alongside `PROCESSOR_REGISTRY.clear_registry()` when the controller
/// shuts down or loses leadership. Dropping the `Sender` causes the worker's
/// `recv()` to return `None`, making it exit its event loop cleanly.
///
/// After this call, `start_acme_service()` can be called again on re-election.
pub fn stop_acme_service() {
    let mut guard = ACME_SERVICE.lock().unwrap();
    if guard.take().is_some() {
        tracing::info!(component = "acme_service", "ACME service stopped");
    }
    // Dropping the AcmeServiceHandle drops the mpsc::Sender, which closes
    // the channel. The worker loop will observe `recv() => None` and exit.
}

/// Startup scan: enumerate all EdgionAcme resources in the cluster and
/// trigger a `ResourceChanged` event for each one.
///
/// This ensures that:
/// - `Ready` certs get their renewal timers re-established
/// - `Pending`/`Failed` certs get retried
/// - Nothing is silently forgotten after a controller restart
async fn scan_all_acme_resources(client: Client) {
    use kube::api::ListParams;

    tracing::info!(component = "acme_service", "Starting full scan of EdgionAcme resources");

    let api: Api<EdgionAcme> = Api::all(client);
    match api.list(&ListParams::default()).await {
        Ok(list) => {
            let count = list.items.len();
            for acme in list.items {
                let ns = acme.metadata.namespace.as_deref().unwrap_or("default");
                let name = acme.metadata.name.as_deref().unwrap_or_default();
                let key = format!("{}/{}", ns, name);
                notify_resource_changed(key);
            }
            tracing::info!(
                component = "acme_service",
                count = count,
                "Startup scan complete, triggered check for all EdgionAcme resources"
            );
        }
        Err(e) => {
            tracing::error!(
                component = "acme_service",
                error = %e,
                "Failed to list EdgionAcme resources during startup scan"
            );
        }
    }
}

/// Clone the current sender from the global handle.
/// Returns `None` if the ACME service has not been started.
fn get_sender() -> Option<mpsc::Sender<AcmeCommand>> {
    let guard = ACME_SERVICE.lock().unwrap();
    guard.as_ref().map(|h| h.tx.clone())
}

/// Notify the ACME service that a resource has changed.
///
/// Called from `EdgionAcmeHandler::parse()` after processing.
/// Non-blocking: uses `try_send` to avoid blocking the processor worker.
pub fn notify_resource_changed(key: String) {
    if let Some(tx) = get_sender() {
        if let Err(e) = tx.try_send(AcmeCommand::ResourceChanged { key: key.clone() }) {
            tracing::warn!(
                component = "acme_service",
                key = %key,
                error = %e,
                "Failed to notify ACME service (channel full or closed)"
            );
        }
    }
}

/// Schedule a renewal check for a resource.
fn schedule_renewal_check(key: String, delay: Duration) {
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        if let Some(tx) = get_sender() {
            let _ = tx.send(AcmeCommand::RenewalCheck { key }).await;
        }
    });
}

// ============================================================================
// Retry policy constants
// ============================================================================

/// Maximum number of consecutive retry attempts before giving up.
/// After reaching this limit, the resource stays in `Failed` state until
/// the user edits the CRD (which resets the retry counter).
const MAX_RETRY_ATTEMPTS: u32 = 5;

/// Compute exponential backoff duration for a given attempt.
///
/// Formula: `base * 4^attempt`
///
/// Example with base=300s (MAX_RETRY_ATTEMPTS=5):
///   attempt 0 → 300s   (5min)
///   attempt 1 → 1200s  (20min)
///   attempt 2 → 4800s  (80min)
///   attempt 3 → 19200s (5.3h)
///   attempt 4 → 76800s (21.3h)  ← last retry, then give up
fn exponential_backoff(base_secs: u64, attempt: u32) -> Duration {
    let multiplier = 4u64.saturating_pow(attempt);
    let backoff = base_secs.saturating_mul(multiplier);
    Duration::from_secs(backoff)
}

// ============================================================================
// ACME Service Worker
// ============================================================================

/// The background worker that processes ACME commands
struct AcmeServiceWorker {
    client: Client,
    /// Keys of resources currently being processed (prevents duplicate work)
    processing: Arc<RwLock<HashSet<String>>>,
    /// Consecutive failure count per resource key (for exponential backoff).
    /// Reset to 0 on success or when the user edits the CRD (ResourceChanged).
    retry_tracker: Arc<RwLock<HashMap<String, u32>>>,
}

impl AcmeServiceWorker {
    fn new(client: Client) -> Self {
        Self {
            client,
            processing: Arc::new(RwLock::new(HashSet::new())),
            retry_tracker: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn run(self, mut rx: mpsc::Receiver<AcmeCommand>) {
        tracing::info!(component = "acme_service", "Worker started, waiting for commands");

        while let Some(cmd) = rx.recv().await {
            match cmd {
                AcmeCommand::ResourceChanged { key } => {
                    // User or system changed the CRD: reset retry counter
                    self.retry_tracker.write().unwrap().remove(&key);
                    self.handle_resource_check(&key).await;
                }
                AcmeCommand::RenewalCheck { key } => {
                    // Timer-driven check (renewal or retry): don't reset counter
                    self.handle_resource_check(&key).await;
                }
            }
        }

        tracing::info!(component = "acme_service", "Worker stopped (channel closed)");
    }

    /// Core resource check — shared by ResourceChanged and RenewalCheck.
    ///
    /// Fetches the resource from K8s, checks phase/expiry, and spawns the
    /// ACME flow if needed. All retry/backoff logic is handled via timers,
    /// never via the K8s resource queue.
    async fn handle_resource_check(&self, key: &str) {
        // Skip if already processing
        if self.processing.read().unwrap().contains(key) {
            return;
        }

        // Parse key into namespace/name
        let (namespace, name) = match parse_resource_key(key) {
            Some(v) => v,
            None => {
                tracing::error!(component = "acme_service", key = %key, "Invalid resource key");
                return;
            }
        };

        // Fetch current resource from K8s
        let api: Api<EdgionAcme> = Api::namespaced(self.client.clone(), &namespace);
        let acme = match api.get(&name).await {
            Ok(a) => a,
            Err(e) => {
                tracing::debug!(
                    component = "acme_service",
                    key = %key,
                    error = %e,
                    "Failed to get EdgionAcme resource (may have been deleted)"
                );
                // Resource deleted — clean up retry tracker
                self.retry_tracker.write().unwrap().remove(key);
                return;
            }
        };

        // Check current phase
        let phase = acme.status.as_ref().map(|s| &s.phase).unwrap_or(&AcmeCertPhase::Pending);
        match phase {
            AcmeCertPhase::Issuing | AcmeCertPhase::Renewing => {
                return; // Already in progress
            }
            AcmeCertPhase::Ready => {
                if !self.needs_renewal(&acme) {
                    // Not time to renew yet — schedule next check
                    let check_interval = acme.spec.renewal.check_interval;
                    schedule_renewal_check(key.to_string(), Duration::from_secs(check_interval));
                    return;
                }
                // Fall through to start renewal
            }
            AcmeCertPhase::Failed => {
                // Check retry limit before attempting again
                let attempt = *self.retry_tracker.read().unwrap().get(key).unwrap_or(&0);
                if attempt >= MAX_RETRY_ATTEMPTS {
                    tracing::warn!(
                        component = "acme_service",
                        key = %key,
                        attempts = attempt,
                        "Retry limit reached ({}), not retrying. Edit the CRD to reset.",
                        MAX_RETRY_ATTEMPTS
                    );
                    return;
                }
                // Fall through to retry issuance
            }
            AcmeCertPhase::Pending => {
                // Fresh resource, start issuance
            }
        }

        // Check validation errors — don't waste retries on config problems
        if let Some(status) = &acme.status {
            if let Some(ref reason) = status.last_failure_reason {
                if reason.starts_with("EdgionAcme:") {
                    tracing::debug!(
                        component = "acme_service",
                        key = %key,
                        reason = %reason,
                        "Skipping: resource has validation errors"
                    );
                    return;
                }
            }
        }

        // Mark as processing
        self.processing.write().unwrap().insert(key.to_string());

        // Spawn the ACME flow task
        let client = self.client.clone();
        let processing = self.processing.clone();
        let retry_tracker = self.retry_tracker.clone();
        let key_owned = key.to_string();
        let is_renewal = *phase == AcmeCertPhase::Ready;
        let fail_backoff_base = acme.spec.renewal.fail_backoff;

        tokio::spawn(async move {
            let result = process_acme_resource(client.clone(), &acme, is_renewal).await;

            match result {
                Ok(()) => {
                    tracing::info!(
                        component = "acme_service",
                        key = %key_owned,
                        "ACME flow completed successfully"
                    );
                    // Success: clear retry counter
                    retry_tracker.write().unwrap().remove(&key_owned);
                }
                Err(e) => {
                    // Increment retry counter
                    let attempt = {
                        let mut tracker = retry_tracker.write().unwrap();
                        let count = tracker.entry(key_owned.clone()).or_insert(0);
                        *count += 1;
                        *count
                    };

                    tracing::error!(
                        component = "acme_service",
                        key = %key_owned,
                        error = %e,
                        attempt = attempt,
                        max_attempts = MAX_RETRY_ATTEMPTS,
                        "ACME flow failed"
                    );

                    // Update status to Failed
                    if let Some((ns, name)) = parse_resource_key(&key_owned) {
                        let api: Api<EdgionAcme> = Api::namespaced(client, &ns);
                        let _ = update_acme_status(
                            &api,
                            &name,
                            AcmeCertPhase::Failed,
                            |status| {
                                status.last_failure_reason = Some(format!("{:#}", e));
                                status.last_failure_time =
                                    Some(chrono::Utc::now().to_rfc3339());
                            },
                        )
                        .await;
                    }

                    // Schedule retry with exponential backoff (if under limit)
                    if attempt < MAX_RETRY_ATTEMPTS {
                        let backoff = exponential_backoff(fail_backoff_base, attempt - 1);
                        tracing::info!(
                            component = "acme_service",
                            key = %key_owned,
                            attempt = attempt,
                            backoff_secs = backoff.as_secs(),
                            "Scheduling retry with exponential backoff"
                        );
                        schedule_renewal_check(key_owned.clone(), backoff);
                    } else {
                        tracing::error!(
                            component = "acme_service",
                            key = %key_owned,
                            "Giving up after {} failed attempts. Edit CRD to reset.",
                            MAX_RETRY_ATTEMPTS
                        );
                    }
                }
            }

            // Remove from processing set
            processing.write().unwrap().remove(&key_owned);
        });
    }

    /// Check if a certificate needs renewal based on expiry time
    fn needs_renewal(&self, acme: &EdgionAcme) -> bool {
        let status = match &acme.status {
            Some(s) => s,
            None => return true, // No status means no cert
        };

        let not_after = match &status.certificate_not_after {
            Some(s) => s,
            None => return true, // No expiry means no cert
        };

        let expiry = match chrono::DateTime::parse_from_rfc3339(not_after) {
            Ok(dt) => dt,
            Err(_) => return true, // Can't parse expiry
        };

        let renew_before = chrono::Duration::days(acme.spec.renewal.renew_before_days as i64);
        let renew_at = expiry - renew_before;

        chrono::Utc::now() >= renew_at
    }
}

// ============================================================================
// ACME Flow - The core certificate issuance/renewal logic
// ============================================================================

/// Process a single EdgionAcme resource through the full ACME flow
async fn process_acme_resource(
    client: Client,
    acme: &EdgionAcme,
    is_renewal: bool,
) -> Result<()> {
    let namespace = acme.metadata.namespace.as_deref().unwrap_or("default");
    let name = acme.metadata.name.as_deref().context("EdgionAcme has no name")?;
    let key = format!("{}/{}", namespace, name);

    let api: Api<EdgionAcme> = Api::namespaced(client.clone(), namespace);

    // 1. Update status to Issuing/Renewing
    let phase = if is_renewal {
        AcmeCertPhase::Renewing
    } else {
        AcmeCertPhase::Issuing
    };
    update_acme_status(&api, name, phase, |_| {}).await?;

    // 2. Create or restore ACME account
    let acme_client = get_or_create_acme_account(&client, acme).await?;

    // 3. Execute challenge-specific flow and obtain certificate
    let cert_result = match acme.spec.challenge.challenge_type {
        AcmeChallengeType::Http01 => {
            execute_http01_flow(&client, &api, name, &acme_client, acme).await?
        }
        AcmeChallengeType::Dns01 => {
            execute_dns01_flow(&client, &acme_client, acme).await?
        }
    };

    // 4. Store certificate in K8s Secret
    let secret_ns = acme.get_secret_namespace();
    create_or_update_cert_secret(
        &client,
        &secret_ns,
        &acme.spec.storage.secret_name,
        &cert_result,
        &key,
    )
    .await?;

    // 5. Create/update EdgionTls if enabled
    if acme.spec.auto_edgion_tls.enabled {
        create_or_update_edgion_tls(&client, acme).await?;
    }

    // 6. Parse certificate info for status
    let cert_info = parse_cert_info(&cert_result.certificate_pem);

    // 7. Update status to Ready
    update_acme_status(&api, name, AcmeCertPhase::Ready, |status| {
        status.certificate_serial = cert_info.serial.clone();
        status.certificate_not_after = cert_info.not_after.clone();
        status.last_renewal_time = Some(chrono::Utc::now().to_rfc3339());
        status.last_failure_reason = None;
        status.last_failure_time = None;
        status.secret_name = Some(acme.spec.storage.secret_name.clone());
        if acme.spec.auto_edgion_tls.enabled {
            status.edgion_tls_name = Some(acme.get_edgion_tls_name());
        }
        status.account_uri = Some(acme_client.account_id().to_string());
    })
    .await?;

    // 8. Schedule renewal check
    let check_interval = acme.spec.renewal.check_interval;
    schedule_renewal_check(
        format!("{}/{}", namespace, name),
        Duration::from_secs(check_interval),
    );

    tracing::info!(
        component = "acme_service",
        key = %key,
        secret = %acme.spec.storage.secret_name,
        not_after = ?cert_info.not_after,
        "Certificate issued and stored successfully"
    );

    Ok(())
}

// ============================================================================
// HTTP-01 Challenge Flow
// ============================================================================

/// Propagation wait time for Gateway to load challenge tokens via gRPC (seconds).
///
/// Timeline: Controller patches CRD → K8s watcher fires (~100ms) →
/// ResourceProcessor → ServerCache → gRPC watch push (~100ms) →
/// Gateway AcmeConfHandler → ChallengeStore.
/// Total: typically < 2s, using 5s for safety margin.
const CHALLENGE_PROPAGATION_WAIT_SECS: u64 = 5;

async fn execute_http01_flow(
    _client: &Client,
    api: &Api<EdgionAcme>,
    name: &str,
    acme_client: &AcmeClient,
    acme: &EdgionAcme,
) -> Result<AcmeCertificateResult> {
    tracing::info!(
        component = "acme_service",
        name = %name,
        domains = ?acme.spec.domains,
        "Starting HTTP-01 challenge flow"
    );

    // Phase 1: Create order and extract challenge tokens (does NOT notify ACME server yet)
    let (pending, mut order_ctx) = acme_client
        .prepare_http01_order(&acme.spec.domains)
        .await
        .context("Failed to prepare HTTP-01 ACME order")?;

    if pending.is_empty() {
        tracing::info!(
            component = "acme_service",
            name = %name,
            "All authorizations already valid, skipping challenge"
        );
    } else {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Phase 2: Push challenge tokens to CRD → K8s watcher → ServerCache → gRPC → Gateway
        let challenges: Vec<ActiveHttpChallenge> = pending
            .iter()
            .map(|p| ActiveHttpChallenge {
                domain: p.domain.clone(),
                token: p.token.clone(),
                key_authorization: p.key_authorization.clone(),
                expire_at: now_secs + 600, // 10 minutes
            })
            .collect();

        tracing::info!(
            component = "acme_service",
            name = %name,
            challenge_count = challenges.len(),
            "Pushing HTTP-01 challenge tokens to CRD"
        );

        patch_active_challenges(api, name, Some(&challenges)).await?;

        // Phase 3: Wait for Gateway to receive tokens via gRPC watch stream
        tracing::debug!(
            component = "acme_service",
            name = %name,
            wait_secs = CHALLENGE_PROPAGATION_WAIT_SECS,
            "Waiting for challenge tokens to propagate to Gateway"
        );
        tokio::time::sleep(Duration::from_secs(CHALLENGE_PROPAGATION_WAIT_SECS)).await;

        // Phase 4: NOW tell the ACME server to start validation
        // (Gateway is ready to serve challenge responses at this point)
        acme_client
            .activate_http01_challenges(&mut order_ctx)
            .await
            .context("Failed to activate HTTP-01 challenges")?;

        tracing::info!(
            component = "acme_service",
            name = %name,
            "ACME server notified, waiting for challenge validation"
        );
    }

    // Phase 5: Poll for validation, finalize order, retrieve certificate
    let cert_result = acme_client
        .complete_http01_order(order_ctx, &acme.spec.domains, &acme.spec.key_type)
        .await;

    // Phase 6: ALWAYS clear challenge tokens from CRD, regardless of success or failure.
    // Otherwise stale tokens remain in Gateway ChallengeStore until expire_at (up to 600s).
    if let Err(e) = patch_active_challenges(api, name, None).await {
        tracing::warn!(
            component = "acme_service",
            name = %name,
            error = %e,
            "Failed to clear challenge tokens (non-fatal)"
        );
    }

    cert_result.context("Failed to complete HTTP-01 ACME order")
}

// ============================================================================
// DNS-01 Challenge Flow
// ============================================================================

async fn execute_dns01_flow(
    _client: &Client,
    acme_client: &AcmeClient,
    acme: &EdgionAcme,
) -> Result<AcmeCertificateResult> {
    let name = acme.metadata.name.as_deref().unwrap_or("unknown");
    let dns01_config = acme
        .spec
        .challenge
        .dns01
        .as_ref()
        .context("DNS-01 config is required")?;

    tracing::info!(
        component = "acme_service",
        name = %name,
        domains = ?acme.spec.domains,
        provider = %dns01_config.provider,
        "Starting DNS-01 challenge flow"
    );

    // Phase 1: Extract credentials and prepare order (does NOT notify ACME server yet)
    let credentials = extract_dns_credentials(acme)?;
    let dns_provider = create_dns_provider(&dns01_config.provider, &credentials)?;

    let (pending, mut order_ctx) = acme_client
        .prepare_dns01_order(&acme.spec.domains)
        .await
        .context("Failed to prepare DNS-01 ACME order")?;

    // Phase 2: Create DNS TXT records for each challenge
    let mut created_records: Vec<(String, String)> = Vec::new();
    for challenge in &pending {
        dns_provider
            .create_txt_record(&challenge.domain, &challenge.digest)
            .await
            .context(format!(
                "Failed to create DNS TXT record for {}",
                challenge.domain
            ))?;
        created_records.push((challenge.domain.clone(), challenge.digest.clone()));
    }

    // Phase 3: Wait for DNS propagation before notifying ACME server.
    // Use `propagation_timeout` (default 120s) as the wait duration, NOT `propagation_check_interval`.
    // DNS TXT records can take time to propagate globally; waiting too briefly causes validation failure.
    let wait_secs = dns01_config.propagation_timeout.max(10);
    tracing::info!(
        component = "acme_service",
        name = %name,
        wait_secs = wait_secs,
        "Waiting for DNS propagation before activating challenges"
    );
    tokio::time::sleep(Duration::from_secs(wait_secs)).await;

    // Phase 4: NOW tell the ACME server to start validation
    // (DNS records have been created and had time to propagate)
    acme_client
        .activate_dns01_challenges(&mut order_ctx)
        .await
        .context("Failed to activate DNS-01 challenges")?;

    tracing::info!(
        component = "acme_service",
        name = %name,
        "ACME server notified, waiting for DNS-01 challenge validation"
    );

    // Phase 5: Poll for validation, finalize order, retrieve certificate
    let cert_result = acme_client
        .complete_dns01_order(order_ctx, &acme.spec.domains, &acme.spec.key_type)
        .await;

    // Phase 6: Clean up DNS records (even if order failed)
    for (domain, value) in &created_records {
        if let Err(e) = dns_provider.remove_txt_record(domain, value).await {
            tracing::warn!(
                component = "acme_service",
                domain = %domain,
                error = %e,
                "Failed to remove DNS TXT record (non-fatal)"
            );
        }
    }

    cert_result.context("Failed to complete DNS-01 ACME order")
}

/// Extract DNS credentials from the resolved Secret in the EdgionAcme spec
fn extract_dns_credentials(acme: &EdgionAcme) -> Result<HashMap<String, String>> {
    let secret = acme
        .spec
        .dns_credential_secret
        .as_ref()
        .context("DNS credential Secret not resolved yet")?;

    let mut credentials = HashMap::new();

    if let Some(ref data) = secret.data {
        for (key, value) in data {
            credentials.insert(
                key.clone(),
                String::from_utf8(value.0.clone())
                    .unwrap_or_else(|_| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &value.0)),
            );
        }
    }

    Ok(credentials)
}

// ============================================================================
// ACME Account Management
// ============================================================================

/// Account Secret name convention: `{EdgionAcme.name}-acme-account`
fn account_secret_name(acme_name: &str) -> String {
    format!("{}-acme-account", acme_name)
}

/// Get or create an ACME account for the given resource
async fn get_or_create_acme_account(
    client: &Client,
    acme: &EdgionAcme,
) -> Result<AcmeClient> {
    let namespace = acme.metadata.namespace.as_deref().unwrap_or("default");
    let acme_name = acme.metadata.name.as_deref().context("EdgionAcme has no name")?;
    let secret_name = account_secret_name(acme_name);
    let secret_api: Api<Secret> = Api::namespaced(client.clone(), namespace);

    // Try to restore from existing account Secret
    match secret_api.get(&secret_name).await {
        Ok(secret) => {
            if let Some(ref data) = secret.data {
                if let Some(creds_bytes) = data.get("credentials") {
                    let creds_json = String::from_utf8(creds_bytes.0.clone())
                        .context("Account credentials Secret contains invalid UTF-8")?;
                    let creds: instant_acme::AccountCredentials =
                        serde_json::from_str(&creds_json)
                            .context("Failed to deserialize account credentials")?;

                    tracing::info!(
                        component = "acme_service",
                        name = %acme_name,
                        "Restored ACME account from Secret"
                    );

                    return AcmeClient::from_credentials(creds).await;
                }
            }
            tracing::warn!(
                component = "acme_service",
                name = %acme_name,
                "Account Secret exists but has no credentials, creating new account"
            );
        }
        Err(kube::Error::Api(resp)) if resp.code == 404 => {
            tracing::info!(
                component = "acme_service",
                name = %acme_name,
                "No existing ACME account, creating new one"
            );
        }
        Err(e) => {
            tracing::warn!(
                component = "acme_service",
                name = %acme_name,
                error = %e,
                "Failed to get account Secret, creating new account"
            );
        }
    }

    // Create new account
    let eab_kid = acme.spec.external_account_binding.as_ref().map(|e| e.key_id.as_str());
    let eab_hmac = acme
        .spec
        .external_account_binding
        .as_ref()
        .map(|e| e.hmac_key.as_str());

    let (acme_client, credentials) = AcmeClient::new(
        &acme.spec.server,
        &acme.spec.email,
        eab_kid,
        eab_hmac,
    )
    .await
    .context("Failed to create ACME account")?;

    // Persist account credentials to K8s Secret
    let creds_json =
        serde_json::to_string(&credentials).context("Failed to serialize account credentials")?;

    let account_secret = Secret {
        metadata: kube::api::ObjectMeta {
            name: Some(secret_name.clone()),
            namespace: Some(namespace.to_string()),
            labels: Some(BTreeMap::from([
                ("edgion.io/managed-by".to_string(), "acme".to_string()),
                (
                    "edgion.io/acme-resource".to_string(),
                    acme_name.to_string(),
                ),
            ])),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            "credentials".to_string(),
            ByteString(creds_json.into_bytes()),
        )])),
        type_: Some("Opaque".to_string()),
        ..Default::default()
    };

    let pp = PatchParams::apply("edgion-acme-controller");
    secret_api
        .patch(&secret_name, &pp, &Patch::Apply(account_secret))
        .await
        .context("Failed to persist ACME account credentials")?;

    tracing::info!(
        component = "acme_service",
        name = %acme_name,
        account_id = %acme_client.account_id(),
        "New ACME account created and persisted"
    );

    Ok(acme_client)
}

// ============================================================================
// K8s Resource Operations
// ============================================================================

/// Patch the EdgionAcme CRD's activeChallenges field
async fn patch_active_challenges(
    api: &Api<EdgionAcme>,
    name: &str,
    challenges: Option<&[ActiveHttpChallenge]>,
) -> Result<()> {
    let patch = serde_json::json!({
        "spec": {
            "activeChallenges": challenges
        }
    });

    let pp = PatchParams::apply("edgion-acme-controller");
    api.patch(name, &pp, &Patch::Merge(patch))
        .await
        .context("Failed to patch activeChallenges")?;

    Ok(())
}

/// Update EdgionAcme status using Server-Side Apply.
///
/// SSA avoids the read-modify-write race condition by letting the K8s API
/// server atomically merge fields. Only the fields we explicitly set will
/// be managed; other controllers' fields are preserved.
async fn update_acme_status<F>(
    api: &Api<EdgionAcme>,
    name: &str,
    phase: AcmeCertPhase,
    modify: F,
) -> Result<()>
where
    F: FnOnce(&mut EdgionAcmeStatus),
{
    let mut status = EdgionAcmeStatus::default();
    status.phase = phase;
    modify(&mut status);

    let patch = serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionAcme",
        "metadata": { "name": name },
        "status": status
    });

    let pp = PatchParams::apply("edgion-acme-controller").force();
    // Try status subresource first, fall back to main resource patch
    match api.patch_status(name, &pp, &Patch::Apply(patch.clone())).await {
        Ok(_) => {}
        Err(_) => {
            // Status subresource may not be available, patch the main resource
            api.patch(name, &pp, &Patch::Apply(patch))
                .await
                .context("Failed to update EdgionAcme status")?;
        }
    }

    Ok(())
}

/// Create or update the K8s TLS Secret with the issued certificate
async fn create_or_update_cert_secret(
    client: &Client,
    namespace: &str,
    name: &str,
    cert: &AcmeCertificateResult,
    acme_key: &str,
) -> Result<()> {
    let api: Api<Secret> = Api::namespaced(client.clone(), namespace);

    let secret = Secret {
        metadata: kube::api::ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            labels: Some(BTreeMap::from([
                ("edgion.io/managed-by".to_string(), "acme".to_string()),
                ("edgion.io/acme-resource".to_string(), acme_key.to_string()),
            ])),
            ..Default::default()
        },
        data: Some(BTreeMap::from([
            (
                "tls.crt".to_string(),
                ByteString(cert.certificate_pem.as_bytes().to_vec()),
            ),
            (
                "tls.key".to_string(),
                ByteString(cert.private_key_pem.as_bytes().to_vec()),
            ),
        ])),
        type_: Some("kubernetes.io/tls".to_string()),
        ..Default::default()
    };

    let pp = PatchParams::apply("edgion-acme-controller");
    api.patch(name, &pp, &Patch::Apply(secret))
        .await
        .context("Failed to create/update TLS certificate Secret")?;

    tracing::info!(
        component = "acme_service",
        namespace = %namespace,
        secret = %name,
        "Certificate Secret created/updated"
    );

    Ok(())
}

/// Create or update the EdgionTls resource for the issued certificate
async fn create_or_update_edgion_tls(
    client: &Client,
    acme: &EdgionAcme,
) -> Result<()> {
    let namespace = acme.metadata.namespace.as_deref().unwrap_or("default");
    let tls_name = acme.get_edgion_tls_name();
    let secret_ns = acme.get_secret_namespace();

    // Build parent_refs
    let parent_refs = acme.spec.auto_edgion_tls.parent_refs.clone();

    // Build EdgionTls JSON for server-side apply
    let edgion_tls = serde_json::json!({
        "apiVersion": "edgion.io/v1",
        "kind": "EdgionTls",
        "metadata": {
            "name": tls_name,
            "namespace": namespace,
            "labels": {
                "edgion.io/managed-by": "acme",
                "edgion.io/acme-resource": acme.metadata.name.as_deref().unwrap_or("unknown"),
            }
        },
        "spec": {
            "hosts": acme.spec.domains,
            "secretRef": {
                "name": acme.spec.storage.secret_name,
                "namespace": secret_ns,
            },
            "parentRefs": parent_refs,
        }
    });

    // Use dynamic API for EdgionTls
    let gvk = kube::api::GroupVersionKind::gvk("edgion.io", "v1", "EdgionTls");
    let ar = kube::api::ApiResource::from_gvk(&gvk);
    let api: Api<kube::api::DynamicObject> =
        Api::namespaced_with(client.clone(), namespace, &ar);

    let pp = PatchParams::apply("edgion-acme-controller");
    api.patch(&tls_name, &pp, &Patch::Apply(edgion_tls))
        .await
        .context("Failed to create/update EdgionTls")?;

    tracing::info!(
        component = "acme_service",
        namespace = %namespace,
        edgion_tls = %tls_name,
        "EdgionTls resource created/updated"
    );

    Ok(())
}

// ============================================================================
// Certificate Parsing
// ============================================================================

struct CertInfo {
    serial: Option<String>,
    not_after: Option<String>,
}

/// Parse basic certificate info from PEM
///
/// Note: `not_after` is formatted as RFC 3339 to match `needs_renewal()` which
/// uses `chrono::DateTime::parse_from_rfc3339()`.
fn parse_cert_info(pem: &str) -> CertInfo {
    match x509_parser::pem::parse_x509_pem(pem.as_bytes()) {
        Ok((_, pem_obj)) => match pem_obj.parse_x509() {
            Ok(cert) => {
                let not_after = {
                    let asn1_time = cert.validity().not_after;
                    // Convert ASN1 timestamp to chrono DateTime, then to RFC 3339
                    chrono::DateTime::from_timestamp(asn1_time.timestamp(), 0)
                        .map(|dt| dt.to_rfc3339())
                };
                CertInfo {
                    serial: Some(cert.serial.to_str_radix(16)),
                    not_after,
                }
            }
            Err(_) => CertInfo {
                serial: None,
                not_after: None,
            },
        },
        Err(_) => CertInfo {
            serial: None,
            not_after: None,
        },
    }
}

// ============================================================================
// Utility
// ============================================================================

/// Parse a "namespace/name" resource key
fn parse_resource_key(key: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = key.splitn(2, '/').collect();
    if parts.len() == 2 {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        // Fallback: treat as name in "default" namespace
        Some(("default".to_string(), key.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ================================================================
    // exponential_backoff
    // ================================================================

    #[test]
    fn test_exponential_backoff_sequence() {
        // base=300s, multiplier=4^attempt
        assert_eq!(exponential_backoff(300, 0), Duration::from_secs(300));      // 5min
        assert_eq!(exponential_backoff(300, 1), Duration::from_secs(1200));     // 20min
        assert_eq!(exponential_backoff(300, 2), Duration::from_secs(4800));     // 80min
        assert_eq!(exponential_backoff(300, 3), Duration::from_secs(19200));    // 5.3h
        assert_eq!(exponential_backoff(300, 4), Duration::from_secs(76800));    // 21.3h
    }

    #[test]
    fn test_exponential_backoff_saturates() {
        // Very large attempt should not overflow (saturating_pow + saturating_mul)
        let result = exponential_backoff(300, 100);
        assert!(result.as_secs() > 0); // Should not be zero/wrap
    }

    // ================================================================
    // parse_resource_key
    // ================================================================

    #[test]
    fn test_parse_resource_key_normal() {
        let (ns, name) = parse_resource_key("default/my-acme").unwrap();
        assert_eq!(ns, "default");
        assert_eq!(name, "my-acme");
    }

    #[test]
    fn test_parse_resource_key_with_slash_in_name() {
        // splitn(2, '/') means only split on the first '/'
        let (ns, name) = parse_resource_key("ns/name/with/slashes").unwrap();
        assert_eq!(ns, "ns");
        assert_eq!(name, "name/with/slashes");
    }

    #[test]
    fn test_parse_resource_key_no_namespace() {
        let (ns, name) = parse_resource_key("just-a-name").unwrap();
        assert_eq!(ns, "default");
        assert_eq!(name, "just-a-name");
    }

    // ================================================================
    // parse_cert_info
    // ================================================================

    #[test]
    fn test_parse_cert_info_invalid_pem() {
        let info = parse_cert_info("not a pem");
        assert!(info.serial.is_none());
        assert!(info.not_after.is_none());
    }

    #[test]
    fn test_parse_cert_info_valid_self_signed() {
        // Generate a self-signed cert for testing
        let key_pair =
            rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let params = rcgen::CertificateParams::new(vec!["test.example.com".to_string()]).unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        let pem = cert.pem();

        let info = parse_cert_info(&pem);
        assert!(info.serial.is_some(), "Should have serial");
        assert!(info.not_after.is_some(), "Should have not_after");

        // Verify not_after is valid RFC 3339 (parseable by chrono)
        let not_after_str = info.not_after.unwrap();
        let parsed = chrono::DateTime::parse_from_rfc3339(&not_after_str);
        assert!(
            parsed.is_ok(),
            "not_after should be valid RFC 3339, got: {}",
            not_after_str
        );
    }
}
