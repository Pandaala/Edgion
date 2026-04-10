use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use std::path::PathBuf;

use crate::core::ctl::certs::{self, GenCertsOptions};

const FIELD_MANAGER: &str = "edgion-cli";

#[derive(Debug, Clone, ValueEnum)]
pub enum OutputMode {
    /// Write certificates to local files
    File,
    /// Write certificates to Kubernetes Secrets
    K8s,
}

#[derive(Args, Debug)]
pub struct GenCertsArgs {
    /// Output mode
    #[arg(long, default_value = "file", value_enum)]
    pub output: OutputMode,

    /// Output directory (file mode only)
    #[arg(long, default_value = "certs/grpc")]
    pub output_dir: PathBuf,

    /// Certificate set name — used as a subdirectory under output_dir (file mode)
    /// and as a suffix on Kubernetes Secret names (k8s mode).
    /// Enables managing multiple named cert sets for cert rotation.
    #[arg(long, default_value = "primary")]
    pub cert_name: String,

    /// Kubernetes namespace (k8s mode; auto-detected from ServiceAccount if omitted)
    #[arg(long, short = 'n')]
    pub namespace: Option<String>,

    /// Kubernetes Secret name prefix
    #[arg(long, default_value = "edgion")]
    pub secret_prefix: String,

    /// CA certificate validity in days
    #[arg(long, default_value = "3650")]
    pub ca_days: u32,

    /// Server/client certificate validity in days
    #[arg(long, default_value = "365")]
    pub cert_days: u32,

    /// Additional server SAN DNS names (can be specified multiple times)
    #[arg(long = "server-san")]
    pub server_san: Vec<String>,

    /// Overwrite existing certificates/Secrets without prompting
    #[arg(long)]
    pub force: bool,
}

pub async fn execute(args: &GenCertsArgs) -> Result<()> {
    // Resolve namespace for SAN generation (and k8s mode)
    let namespace = match &args.namespace {
        Some(ns) => Some(ns.clone()),
        None => detect_namespace_from_sa(),
    };

    let opts = GenCertsOptions {
        ca_days: args.ca_days,
        cert_days: args.cert_days,
        server_sans: args.server_san.clone(),
        namespace: namespace.clone(),
    };

    println!("Generating conf_sync mTLS certificates...");
    let bundle = certs::generate_certs(&opts).context("Certificate generation failed")?;
    println!("  CA:     CN=edgion-grpc-ca (valid {} days)", args.ca_days);
    println!("  Server: CN=edgion-controller (valid {} days)", args.cert_days);
    println!("  Client: CN=edgion-gateway (valid {} days)", args.cert_days);

    match args.output {
        OutputMode::File => {
            let out_dir = args.output_dir.join(&args.cert_name);
            write_to_files(&bundle, &out_dir, args.force).await?;
        }
        OutputMode::K8s => {
            let ns = namespace.ok_or_else(|| {
                anyhow::anyhow!(
                    "Cannot detect Kubernetes namespace. Use --namespace to specify, \
                     or run inside a Pod with a ServiceAccount."
                )
            })?;
            write_to_k8s_secrets(&bundle, &ns, &args.secret_prefix, &args.cert_name, args.force).await?;
        }
    }

    Ok(())
}

/// Write certificate bundle to files in the given directory.
async fn write_to_files(bundle: &certs::CertBundle, dir: &PathBuf, force: bool) -> Result<()> {
    if dir.exists() && !force {
        let has_certs =
            dir.join("ca.crt").exists() || dir.join("server.crt").exists() || dir.join("client.crt").exists();
        if has_certs {
            anyhow::bail!(
                "Certificate files already exist in {:?}. Use --force to overwrite.",
                dir
            );
        }
    }

    tokio::fs::create_dir_all(dir)
        .await
        .context("Failed to create output directory")?;

    // Restrict directory permissions (Unix only) — contains private keys
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).await?;
    }

    tokio::fs::write(dir.join("ca.crt"), &bundle.ca_cert_pem).await?;
    tokio::fs::write(dir.join("server.crt"), &bundle.server_cert_pem).await?;
    tokio::fs::write(dir.join("server.key"), &bundle.server_key_pem).await?;
    tokio::fs::write(dir.join("client.crt"), &bundle.client_cert_pem).await?;
    tokio::fs::write(dir.join("client.key"), &bundle.client_key_pem).await?;

    // Restrict private key file permissions (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let key_perms = std::fs::Permissions::from_mode(0o600);
        tokio::fs::set_permissions(dir.join("server.key"), key_perms.clone()).await?;
        tokio::fs::set_permissions(dir.join("client.key"), key_perms).await?;
    }

    println!("\nCertificates written to {:?}:", dir);
    println!("  ca.crt      - CA certificate");
    println!("  server.crt  - Controller server certificate");
    println!("  server.key  - Controller server private key");
    println!("  client.crt  - Gateway client certificate");
    println!("  client.key  - Gateway client private key");

    Ok(())
}

/// Write certificate bundle to Kubernetes Secrets.
async fn write_to_k8s_secrets(
    bundle: &certs::CertBundle,
    namespace: &str,
    prefix: &str,
    cert_name: &str,
    force: bool,
) -> Result<()> {
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use kube::api::{Api, Patch, PatchParams};
    use kube::Client;
    use std::collections::BTreeMap;

    let client = Client::try_default()
        .await
        .context("Failed to create Kubernetes client. Are you running inside a cluster or have a valid kubeconfig?")?;

    let api: Api<Secret> = Api::namespaced(client, namespace);

    let controller_secret_name = format!("{}-controller-tls-{}", prefix, cert_name);
    let gateway_secret_name = format!("{}-gateway-tls-{}", prefix, cert_name);

    // Check existing secrets if not --force
    if !force {
        for name in [&controller_secret_name, &gateway_secret_name] {
            if api.get_opt(name).await?.is_some() {
                anyhow::bail!(
                    "Secret {}/{} already exists. Use --force to overwrite.",
                    namespace,
                    name
                );
            }
        }
    }

    // Build controller Secret
    let controller_secret = Secret {
        metadata: kube::api::ObjectMeta {
            name: Some(controller_secret_name.clone()),
            namespace: Some(namespace.to_string()),
            labels: Some(BTreeMap::from([(
                "app.kubernetes.io/managed-by".to_string(),
                FIELD_MANAGER.to_string(),
            )])),
            ..Default::default()
        },
        data: Some(BTreeMap::from([
            ("ca.crt".to_string(), ByteString(bundle.ca_cert_pem.as_bytes().to_vec())),
            (
                "server.crt".to_string(),
                ByteString(bundle.server_cert_pem.as_bytes().to_vec()),
            ),
            (
                "server.key".to_string(),
                ByteString(bundle.server_key_pem.as_bytes().to_vec()),
            ),
        ])),
        ..Default::default()
    };

    // Build gateway Secret
    let gateway_secret = Secret {
        metadata: kube::api::ObjectMeta {
            name: Some(gateway_secret_name.clone()),
            namespace: Some(namespace.to_string()),
            labels: Some(BTreeMap::from([(
                "app.kubernetes.io/managed-by".to_string(),
                FIELD_MANAGER.to_string(),
            )])),
            ..Default::default()
        },
        data: Some(BTreeMap::from([
            ("ca.crt".to_string(), ByteString(bundle.ca_cert_pem.as_bytes().to_vec())),
            (
                "client.crt".to_string(),
                ByteString(bundle.client_cert_pem.as_bytes().to_vec()),
            ),
            (
                "client.key".to_string(),
                ByteString(bundle.client_key_pem.as_bytes().to_vec()),
            ),
        ])),
        ..Default::default()
    };

    // Write secrets using server-side apply (idempotent create-or-update)
    let pp = PatchParams::apply(FIELD_MANAGER).force();
    api.patch(&controller_secret_name, &pp, &Patch::Apply(controller_secret))
        .await
        .with_context(|| format!("Failed to write Secret {}/{}", namespace, controller_secret_name))?;

    api.patch(&gateway_secret_name, &pp, &Patch::Apply(gateway_secret))
        .await
        .with_context(|| format!("Failed to write Secret {}/{}", namespace, gateway_secret_name))?;

    println!("\nKubernetes Secrets written in namespace '{}':", namespace);
    println!("  {} - ca.crt, server.crt, server.key", controller_secret_name);
    println!("  {} - ca.crt, client.crt, client.key", gateway_secret_name);

    Ok(())
}

/// Try to detect namespace from Kubernetes ServiceAccount mount.
fn detect_namespace_from_sa() -> Option<String> {
    std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
