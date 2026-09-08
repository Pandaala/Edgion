//! OCI image source for Wasm bytecode (`source.oci`).
//!
//! Field semantics align with Istio `WasmPlugin` / Kubernetes image pulls:
//! `image` is `registry/repo:tag` or `registry/repo@sha256:<digest>`; an
//! unset `pullPolicy` behaves as `Always` for `latest` (or a missing tag)
//! and `IfNotPresent` otherwise; a digest reference is always a pin.

use schemars::JsonSchema;

crate::redact::impl_redacted_debug!(WasmOciSpec);
use serde::{Deserialize, Serialize};

use crate::resources::common::SecretObjectReference;

/// Pull policy for an OCI-sourced Wasm module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum OciPullPolicy {
    IfNotPresent,
    Always,
}

/// `source.oci`: pull the Wasm bytecode from an OCI registry.
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WasmOciSpec {
    /// Image reference: `registry/repo:tag` or `registry/repo@sha256:<digest>`.
    /// The registry host is required (no default-registry shorthand).
    pub image: String,

    /// Pull policy. Unset follows the Kubernetes convention: `Always` when
    /// the tag is `latest` (or omitted), `IfNotPresent` otherwise. A digest
    /// reference is always treated as pinned regardless of this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_policy: Option<OciPullPolicy>,

    /// `kubernetes.io/dockerconfigjson` Secret holding the registry
    /// credentials (cross-namespace refs require a ReferenceGrant).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_secret_ref: Option<SecretObjectReference>,

    /// Controller-resolved raw dockerconfigjson content. Never enters the
    /// CRD schema; redacted in admin output; shipped to the gateway over
    /// conf_sync.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_pull_secret: Option<String>,
}

impl WasmOciSpec {
    /// Resolve the effective pull policy for an already-parsed image ref.
    pub fn effective_pull_policy(&self, image: &OciImageRef) -> OciPullPolicy {
        if image.digest.is_some() {
            return OciPullPolicy::IfNotPresent;
        }
        if let Some(p) = self.pull_policy {
            return p;
        }
        match image.tag.as_deref() {
            None | Some("latest") => OciPullPolicy::Always,
            _ => OciPullPolicy::IfNotPresent,
        }
    }
}

/// A parsed OCI image reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciImageRef {
    /// Registry host, optionally with a port (e.g. `registry.internal:5000`).
    pub registry: String,
    /// Repository path (e.g. `wasm/header-demo`).
    pub repository: String,
    /// Tag, when the reference carries one.
    pub tag: Option<String>,
    /// Manifest digest (`sha256:<64 hex>`, lowercase), when pinned.
    pub digest: Option<String>,
}

impl OciImageRef {
    /// Parse `registry/repo[:tag][@sha256:<digest>]`. The registry host is
    /// required: the first path component must contain a `.` or `:` or be
    /// `localhost`, matching the containerd/docker heuristic.
    pub fn parse(image: &str) -> Result<Self, String> {
        let (rest, digest) = match image.split_once('@') {
            Some((r, d)) => {
                let hex_part = d
                    .strip_prefix("sha256:")
                    .ok_or_else(|| format!("digest must use sha256:<64 hex>, got '{d}'"))?;
                if hex_part.len() != 64 || !hex_part.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(format!("digest must be sha256:<64 hex>, got '{d}'"));
                }
                (r, Some(d.to_ascii_lowercase()))
            }
            None => (image, None),
        };
        let (registry, path) = rest
            .split_once('/')
            .ok_or_else(|| format!("'{image}' must include a registry host (e.g. registry.example.com/repo)"))?;
        if !(registry.contains('.') || registry.contains(':') || registry == "localhost") {
            return Err(format!(
                "'{registry}' does not look like a registry host; a full host is required \
                 (e.g. registry.example.com/repo)"
            ));
        }
        let (repository, tag) = match path.rsplit_once(':') {
            Some((repo, t)) if !t.contains('/') => (repo.to_string(), Some(t.to_string())),
            _ => (path.to_string(), None),
        };
        if repository.is_empty()
            || !repository
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-' | b'/'))
        {
            return Err(format!("invalid repository '{repository}' in '{image}'"));
        }
        if let Some(t) = &tag {
            let valid = !t.is_empty()
                && t.len() <= 128
                && t.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
            if !valid {
                return Err(format!("invalid tag '{t}' in '{image}'"));
            }
        }
        Ok(OciImageRef {
            registry: registry.to_string(),
            repository,
            tag,
            digest,
        })
    }

    /// The registry API host actually dialed. `docker.io` (and its legacy
    /// `index.docker.io` alias) serve the registry API from
    /// `registry-1.docker.io`.
    pub fn api_host(&self) -> String {
        match self.registry.as_str() {
            "docker.io" | "index.docker.io" => "registry-1.docker.io".to_string(),
            other => other.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tag_reference() {
        let r = OciImageRef::parse("registry.internal:5000/wasm/header-demo:1.2.0").unwrap();
        assert_eq!(r.registry, "registry.internal:5000");
        assert_eq!(r.repository, "wasm/header-demo");
        assert_eq!(r.tag.as_deref(), Some("1.2.0"));
        assert!(r.digest.is_none());
    }

    #[test]
    fn parse_digest_reference() {
        let d = format!("sha256:{}", "a".repeat(64));
        let r = OciImageRef::parse(&format!("ghcr.io/org/mod@{d}")).unwrap();
        assert_eq!(r.digest.as_deref(), Some(d.as_str()));
        assert!(r.tag.is_none());
    }

    #[test]
    fn parse_localhost_and_bare_repo() {
        assert!(OciImageRef::parse("localhost:5000/demo:1").is_ok());
        let r = OciImageRef::parse("registry.example.com/demo").unwrap();
        assert!(r.tag.is_none() && r.digest.is_none());
    }

    #[test]
    fn parse_rejects_missing_registry_and_bad_digest() {
        assert!(OciImageRef::parse("just-a-repo:tag").is_err());
        assert!(OciImageRef::parse("repo-only").is_err());
        assert!(OciImageRef::parse("reg.io/repo@sha256:short").is_err());
        assert!(OciImageRef::parse("reg.io/repo@md5:abcd").is_err());
        assert!(OciImageRef::parse("reg.io/UPPER/repo:1").is_err());
        assert!(OciImageRef::parse("reg.io/repo:bad tag").is_err());
    }

    #[test]
    fn effective_policy_matrix() {
        let parse = |s: &str| OciImageRef::parse(s).unwrap();
        let spec = |p: Option<OciPullPolicy>| WasmOciSpec {
            image: String::new(),
            pull_policy: p,
            ..Default::default()
        };
        // Digest pins even with an explicit Always.
        let pinned = parse(&format!("r.io/a@sha256:{}", "b".repeat(64)));
        assert_eq!(
            spec(Some(OciPullPolicy::Always)).effective_pull_policy(&pinned),
            OciPullPolicy::IfNotPresent
        );
        // Explicit policy wins for tag refs.
        let tagged = parse("r.io/a:1.0");
        assert_eq!(
            spec(Some(OciPullPolicy::Always)).effective_pull_policy(&tagged),
            OciPullPolicy::Always
        );
        // Unset: latest / no tag => Always; other tags => IfNotPresent.
        assert_eq!(
            spec(None).effective_pull_policy(&parse("r.io/a:latest")),
            OciPullPolicy::Always
        );
        assert_eq!(
            spec(None).effective_pull_policy(&parse("r.io/a")),
            OciPullPolicy::Always
        );
        assert_eq!(spec(None).effective_pull_policy(&tagged), OciPullPolicy::IfNotPresent);
    }

    #[test]
    fn api_host_normalizes_docker_io() {
        assert_eq!(
            OciImageRef::parse("docker.io/org/mod:1").unwrap().api_host(),
            "registry-1.docker.io"
        );
        assert_eq!(OciImageRef::parse("ghcr.io/org/mod:1").unwrap().api_host(), "ghcr.io");
    }

    #[test]
    fn resolved_pull_secret_redacted_under_admin_guard() {
        let spec = WasmOciSpec {
            image: "r.io/a:1".to_string(),
            resolved_pull_secret: Some("{\"auths\":{}}".to_string()),
            ..Default::default()
        };
        let plain = serde_json::to_value(&spec).unwrap();
        assert_eq!(plain["resolvedPullSecret"], "{\"auths\":{}}");
        let _guard = crate::redact::AdminRedactGuard::new();
        let redacted = serde_json::to_value(&spec).unwrap();
        assert_eq!(redacted["resolvedPullSecret"], "[redacted]");
    }

    #[test]
    fn pull_secret_ref_deserializes_camel_case() {
        let spec: WasmOciSpec = serde_json::from_str(
            r#"{"image":"r.io/a:1","pullPolicy":"IfNotPresent","pullSecretRef":{"name":"creds"}}"#,
        )
        .unwrap();
        assert_eq!(spec.pull_policy, Some(OciPullPolicy::IfNotPresent));
        assert_eq!(spec.pull_secret_ref.as_ref().unwrap().name, "creds");
    }
}
