//! Host (kernel) FIPS-mode detection.
//!
//! Edgion links BoringSSL statically, so the host's FIPS mode has no effect on
//! the crypto executing in this process — `fips_module_available()` is fixed at
//! build time. This module answers a different, purely informational question:
//! is the *node* in FIPS mode? The answer drives two things: a posture
//! guardrail (a non-FIPS binary on a FIPS node is a WARN) and the `auto`
//! default of the approved cipher/curve policy.
//!
//! See skills/04-review/tls/regression-guards/boring-v5-fips-build-verification.md.
//!
//! Known limitation: under VM- or sandbox-isolated runtimes (Kata, Firecracker,
//! gVisor) this file reflects the guest kernel, not the physical node. The read
//! succeeds and returns a valid-but-wrong value, so no state here can catch it.
//! Operators on such runtimes must set `fips_policy` explicitly.

use std::path::Path;

/// Whether the host kernel reports FIPS mode.
///
/// Three states, not a bool: a path that exists but cannot be read (`/proc`
/// masked by a hardened `securityContext`) must not be indistinguishable from a
/// plain non-FIPS host. Hardened nodes are exactly the high-assurance FIPS
/// deployments this feature targets, and a silent `false` there would disable
/// both the guardrail and the auto policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFipsState {
    /// The kernel reports FIPS mode.
    Enabled,
    /// The kernel reports non-FIPS mode, or the file does not exist (non-FIPS
    /// kernel, or a non-Linux host).
    Disabled,
    /// The file could not be read. We could not determine the host's state.
    Unknown,
}

/// The kernel's FIPS-mode flag. Under the default namespace-sharing container
/// runtimes (runc/containerd) this reflects the host kernel.
const DEFAULT_FIPS_PROC_PATH: &str = "/proc/sys/crypto/fips_enabled";

/// Test-only override for the detection path. Production-unsafe: it is the sole
/// input to detection, so pointing it at an empty file suppresses the posture
/// guardrail entirely. Honored only under integration-testing mode; see
/// `detect_host_fips`.
pub const FIPS_HOST_PATH_ENV: &str = "EDGION_FIPS_HOST_PATH";

/// Whether the flag file's contents mean "FIPS mode on". The kernel writes
/// exactly `1` or `0` plus a newline.
pub fn parse_fips_enabled(contents: &str) -> bool {
    contents.trim() == "1"
}

/// Read the FIPS flag at `path`. A missing file is a definite `Disabled`; any
/// other read error is `Unknown`.
pub fn detect_host_fips_at(path: &Path) -> HostFipsState {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            if parse_fips_enabled(&contents) {
                HostFipsState::Enabled
            } else {
                HostFipsState::Disabled
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(
                target: "startup",
                path = %path.display(),
                "host FIPS flag absent; treating host as non-FIPS"
            );
            HostFipsState::Disabled
        }
        Err(e) => {
            tracing::warn!(
                target: "startup",
                path = %path.display(),
                error = %e,
                "could not read the host FIPS flag; host FIPS state is unknown. \
                 The posture guardrail and the fips_policy=auto default are \
                 both inert until this is resolved. Set fips_policy explicitly \
                 to remove the ambiguity."
            );
            HostFipsState::Unknown
        }
    }
}

/// Detect the host FIPS state, consulting `FIPS_HOST_PATH_ENV` only when
/// `allow_path_override` is set (integration-testing mode).
///
/// The override is double-gated because, unlike `EDGION_FIPS_POLICY` — which
/// only feeds the enforcement atomic and cannot suppress the WARN or the
/// `fips_host_enabled` gauge — this variable is the sole input to detection.
/// Ungated, it would be a one-line kill switch for the entire posture signal.
pub fn detect_host_fips(allow_path_override: bool) -> HostFipsState {
    let override_path = std::env::var(FIPS_HOST_PATH_ENV).ok();

    match (override_path, allow_path_override) {
        (Some(path), true) => {
            tracing::warn!(
                target: "startup",
                env = FIPS_HOST_PATH_ENV,
                path = %path,
                "host FIPS detection path is overridden; this is a test-only mode"
            );
            detect_host_fips_at(Path::new(&path))
        }
        (Some(path), false) => {
            tracing::warn!(
                target: "startup",
                env = FIPS_HOST_PATH_ENV,
                path = %path,
                "refusing the host FIPS detection path override: it requires \
                 integration-testing mode. Reading the real kernel flag instead."
            );
            detect_host_fips_at(Path::new(DEFAULT_FIPS_PROC_PATH))
        }
        (None, _) => detect_host_fips_at(Path::new(DEFAULT_FIPS_PROC_PATH)),
    }
}

/// The posture guardrail predicate: a FIPS node running a binary that does not
/// run BoringSSL in FIPS mode. `Unknown` never fires it — an
/// undetermined host is not evidence of a violation.
pub fn host_fips_mismatch(host: HostFipsState, module_available: bool) -> bool {
    matches!(host, HostFipsState::Enabled) && !module_available
}

#[cfg(test)]
#[path = "../../tests/unit/tls/fips_host_test.rs"]
mod tests;
