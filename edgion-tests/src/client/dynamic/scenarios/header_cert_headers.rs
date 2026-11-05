//! Certificate projection owns only the current plugin generation's outputs.

use crate::dynamic::scenario::{PhaseSuite, Scenario, ScenarioRunFuture};
use crate::dynamic::ScenarioCtx;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::port_config::PortKind;
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rcgen::{BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

const NAME: &str = "header-cert-headers";
const NAMESPACE: &str = "edgion-test";
const HOST: &str = "header-cert-headers.example.com";
const SAN: &str = "trusted-client.example.com";
const OWNED_KINDS: &[&str] = &["HTTPRoute", "EdgionPlugins", "EdgionTls", "Secret"];

struct Certificates {
    secret: Value,
    ca_pem: String,
    cn_only: Vec<u8>,
    with_san: Vec<u8>,
}

// Fixtures are generated once per test-client process so all wait_phase retries
// use the same trust generation. Nothing is written to the repository or logs.
static CERTIFICATES: LazyLock<Result<Certificates>> = LazyLock::new(make_certificates);

fn certificates() -> Result<&'static Certificates> {
    CERTIFICATES
        .as_ref()
        .map_err(|_| anyhow::anyhow!("certificate fixture generation failed"))
}

fn make_certificates() -> Result<Certificates> {
    let ca_key = KeyPair::generate()?;
    let mut ca = CertificateParams::new(Vec::<String>::new())?;
    ca.distinguished_name
        .push(DnType::CommonName, "Header projection test CA");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_pem = ca.self_signed(&ca_key)?.pem();
    let issuer = Issuer::from_params(&ca, &ca_key);
    let server_key = KeyPair::generate()?;
    let mut server = CertificateParams::new(vec![HOST.to_string()])?;
    server.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_pem = server.signed_by(&server_key, &issuer)?.pem();
    let client = |sans: Vec<String>| -> Result<Vec<u8>> {
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::new(sans)?;
        params.distinguished_name.push(DnType::CommonName, "ordinary-client");
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let mut pem = params.signed_by(&key, &issuer)?.pem().into_bytes();
        pem.extend_from_slice(key.serialize_pem().as_bytes());
        Ok(pem)
    };
    Ok(Certificates {
        secret: json!({
            "apiVersion":"v1", "kind":"Secret",
            "metadata":{"name":NAME,"namespace":NAMESPACE},
            "type":"kubernetes.io/tls",
            "data":{
                "tls.crt":STANDARD.encode(server_pem),
                "tls.key":STANDARD.encode(server_key.serialize_pem()),
                "ca.crt":STANDARD.encode(&ca_pem)
            }
        }),
        ca_pem,
        cn_only: client(vec![])?,
        with_san: client(vec![SAN.to_string()])?,
    })
}

struct HeaderSuite(Option<&'static str>);

impl TestSuite for HeaderSuite {
    fn name(&self) -> &str {
        "HeaderCertHeadersDynamic"
    }

    fn port_key(&self) -> Option<&str> {
        Some("EdgionTls/mTLS")
    }

    fn port_kind(&self) -> PortKind {
        PortKind::Https
    }

    fn test_cases(&self) -> Vec<TestCase> {
        let destination = self.0;
        vec![TestCase::new(
            "certificate_identity_outputs",
            "Fresh mTLS certificate metadata replaces client identity headers across reload",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let started = Instant::now();
                    let result = match destination {
                        Some(name) => probe_outputs(&ctx, name).await,
                        None => assert_absent(&ctx).await,
                    };
                    match result {
                        Ok(()) => TestResult::passed(started.elapsed()),
                        Err(error) => TestResult::failed(started.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )
        .with_timeout(Duration::from_secs(15))]
    }
}

fn header_values<'a>(body: &'a str, name: &str) -> Vec<&'a str> {
    body.lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
        .collect()
}

async fn probe_outputs(ctx: &TestContext, destination: &str) -> Result<()> {
    let certs = certificates()?;
    let target = tokio::net::lookup_host((ctx.target_host.as_str(), ctx.https_port))
        .await?
        .next()
        .context("mTLS target resolved to no addresses")?;
    for (identity, expected_san) in [
        (Some(certs.cn_only.as_slice()), None),
        (Some(certs.with_san.as_slice()), Some(SAN)),
        (None, None),
    ] {
        // New clients force a fresh handshake on every probe. OptionalMutual
        // verifies both issued client certificates while allowing an anonymous control.
        let mut builder = reqwest::Client::builder()
            .add_root_certificate(reqwest::Certificate::from_pem(certs.ca_pem.as_bytes())?)
            .no_proxy()
            .resolve(HOST, target)
            .timeout(Duration::from_secs(3));
        if let Some(pem) = identity {
            builder = builder.identity(reqwest::Identity::from_pem(pem)?);
        }
        let response = builder
            .build()?
            .get(format!("https://{HOST}:{}/echo", ctx.https_port))
            .header("x-san-a", "forged-a")
            .header("x-san-a", "duplicate-a")
            .header("x-san-b", "forged-b")
            .header("x-san-b", "duplicate-b")
            .header("x-cert-dn", "forged-dn")
            .header("x-cert-fingerprint", "forged-fingerprint")
            .header("x-unrelated", "keep")
            .send()
            .await?;
        ensure!(
            response.status().as_u16() == 200,
            "unexpected status {}",
            response.status()
        );
        let body = response.text().await?;
        let values = header_values(&body, destination);
        if let Some(expected) = expected_san {
            ensure!(
                values == [expected],
                "certificate SAN was not the sole configured output"
            );
        } else {
            ensure!(values.is_empty(), "unverified configured SAN reached the upstream");
        }
        let unowned = if destination == "x-san-a" { "x-san-b" } else { "x-san-a" };
        ensure!(header_values(&body, unowned).len() == 2, "unowned header lines changed");
        ensure!(
            header_values(&body, "x-unrelated") == ["keep"],
            "unrelated header changed"
        );
        let dn = header_values(&body, "x-cert-dn");
        let fingerprint = header_values(&body, "x-cert-fingerprint");
        if identity.is_some() {
            ensure!(
                dn.len() == 1 && dn[0].contains("ordinary-client"),
                "verified DN missing"
            );
            ensure!(
                fingerprint.len() == 1 && fingerprint[0] != "forged-fingerprint",
                "verified fingerprint missing"
            );
        } else {
            ensure!(
                dn.is_empty() && fingerprint.is_empty(),
                "anonymous request retained certificate identity"
            );
            ensure!(
                header_values(&body, "x-consumer-username") == ["anonymous"],
                "anonymous marker missing"
            );
        }
    }
    Ok(())
}

async fn assert_absent(ctx: &TestContext) -> Result<()> {
    let base = std::env::var("EDGION_TEST_GATEWAY_ADMIN_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| format!("http://{}:12001", ctx.target_host));
    for kind in ["HTTPRoute", "EdgionPlugins", "EdgionTls"] {
        let mut request = ctx
            .http_client
            .get(format!("{}/configclient/{kind}/list", base.trim_end_matches('/')))
            .timeout(Duration::from_secs(3));
        if let Ok(token) = std::env::var("EDGION_GATEWAY_ADMIN_TOKEN") {
            request = request.bearer_auth(token);
        }
        let body: Value = request.send().await?.error_for_status()?.json().await?;
        ensure!(body["success"] == true, "gateway resource listing failed");
        let resources = body["data"]
            .as_array()
            .context("gateway resource list has no data array")?;
        ensure!(
            !resources.iter().any(|resource| {
                resource["metadata"]["name"] == NAME && resource["metadata"]["namespace"] == NAMESPACE
            }),
            "owned {kind} has not been removed from the gateway"
        );
    }
    Ok(())
}

async fn apply(ctx: &ScenarioCtx, resource: &Value) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    crate::dynamic::apply::apply_yaml_documents(
        &client,
        Path::new("header-cert-headers-runtime.yaml"),
        &yaml_serde::to_string(resource)?,
    )
    .await
}

async fn set_projection(ctx: &ScenarioCtx, destination: &str) -> Result<()> {
    apply(ctx, &json!({
        "apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
        "metadata":{"name":NAME,"namespace":NAMESPACE},
        "spec":{"requestPlugins":[{"type":"HeaderCertAuth","config":{
            "mode":"Connection", "skipConsumerLookup":true, "allowAnonymous":true,
            "upstreamHeaders":{"dnHeader":"x-cert-dn","sanHeader":destination,"fingerprintHeader":"x-cert-fingerprint"}
        }}]}
    })).await
}

async fn setup(ctx: &ScenarioCtx) -> Result<()> {
    apply(ctx, &certificates()?.secret).await?;
    apply(
        ctx,
        &json!({
            "apiVersion":"edgion.io/v1", "kind":"EdgionTls",
            "metadata":{"name":NAME,"namespace":NAMESPACE,"annotations":{"edgion.io/expose-client-cert":"true"}},
            "spec":{
                "parentRefs":[{"name":"mtls-test-gateway","namespace":NAMESPACE,"sectionName":"https-mtls"}],
                "hosts":[HOST], "secretRef":{"name":NAME,"namespace":NAMESPACE},
                "clientAuth":{"mode":"OptionalMutual","caSecretRef":{"name":NAME,"namespace":NAMESPACE},"verifyDepth":1}
            }
        }),
    )
    .await?;
    set_projection(ctx, "x-san-a").await?;
    apply(ctx, &json!({
        "apiVersion":"gateway.networking.k8s.io/v1", "kind":"HTTPRoute",
        "metadata":{"name":NAME,"namespace":NAMESPACE},
        "spec":{
            "parentRefs":[{"name":"mtls-test-gateway","namespace":NAMESPACE,"sectionName":"https-mtls"}],
            "hostnames":[HOST], "rules":[{
                "filters":[{"type":"ExtensionRef","extensionRef":{"group":"edgion.io","kind":"EdgionPlugins","name":NAME}}],
                "backendRefs":[{"name":"test-http","port":30001}]
            }]
        }
    })).await
}

async fn cleanup(ctx: &ScenarioCtx) -> Result<()> {
    let client = crate::dynamic::apply::build_client(&ctx.controller_url)?;
    let mut failures = Vec::new();
    for kind in OWNED_KINDS {
        match client.delete(kind, Some(NAMESPACE), NAME).await {
            Ok(response) if response.status().is_success() || response.status().as_u16() == 404 => {}
            Ok(response) => failures.push(format!("{kind} cleanup status {}", response.status())),
            Err(_) => failures.push(format!("{kind} cleanup request failed")),
        }
    }
    if let Err(error) = ctx.wait_phase(SCENARIO, "absent", Duration::from_secs(20)).await {
        failures.push(format!("{error:#}"));
    }
    ensure!(failures.is_empty(), "{}", failures.join("; "));
    Ok(())
}

fn absent() -> Box<dyn TestSuite> {
    Box::new(HeaderSuite(None))
}
fn initial() -> Box<dyn TestSuite> {
    Box::new(HeaderSuite(Some("x-san-a")))
}
fn updated() -> Box<dyn TestSuite> {
    Box::new(HeaderSuite(Some("x-san-b")))
}
static PHASES: &[PhaseSuite] = &[
    PhaseSuite {
        name: "absent",
        factory: absent,
    },
    PhaseSuite {
        name: "initial",
        factory: initial,
    },
    PhaseSuite {
        name: "updated",
        factory: updated,
    },
];

fn run(ctx: ScenarioCtx) -> ScenarioRunFuture {
    Box::pin(async move {
        let result = async {
            cleanup(&ctx).await?;
            setup(&ctx).await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(20)).await?;
            set_projection(&ctx, "x-san-b").await?;
            ctx.wait_phase(SCENARIO, "updated", Duration::from_secs(20)).await?;
            set_projection(&ctx, "x-san-a").await?;
            ctx.wait_phase(SCENARIO, "initial", Duration::from_secs(20)).await
        }
        .await;
        let cleanup = cleanup(&ctx).await;
        match (result, cleanup) {
            (Ok(()), cleanup) => cleanup,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(cleanup)) => Err(anyhow::anyhow!("{error:#}; cleanup also failed: {cleanup:#}")),
        }
    })
}

pub static SCENARIO: &Scenario = &Scenario {
    id: "HeaderCertHeadersDynamic",
    conf_path: "EdgionPlugins/HeaderCertAuth",
    run,
    phases: PHASES,
};

#[cfg(test)]
mod tests {
    use super::*;
    use x509_parser::extensions::GeneralName;

    #[test]
    fn generated_clients_share_trust_but_only_one_has_a_san() {
        let fixtures = make_certificates().unwrap();
        let (_, ca_pem) = x509_parser::pem::parse_x509_pem(fixtures.ca_pem.as_bytes()).unwrap();
        let (_, ca) = x509_parser::parse_x509_certificate(&ca_pem.contents).unwrap();
        for (pem, expected_san) in [(&fixtures.cn_only, false), (&fixtures.with_san, true)] {
            let (_, pem) = x509_parser::pem::parse_x509_pem(pem).unwrap();
            let (_, leaf) = x509_parser::parse_x509_certificate(&pem.contents).unwrap();
            leaf.verify_signature(Some(ca.public_key())).unwrap();
            assert_eq!(
                leaf.subject().iter_common_name().next().unwrap().as_str().unwrap(),
                "ordinary-client"
            );
            let san = leaf.subject_alternative_name().unwrap();
            if expected_san {
                assert_eq!(san.unwrap().value.general_names, [GeneralName::DNSName(SAN)]);
            } else {
                assert!(san.is_none());
            }
        }
    }

    #[test]
    fn echo_header_observation_retains_duplicate_lines_and_matches_names_case_insensitively() {
        let body = "Headers:\n  X-San-A: forged\n  x-san-a: duplicate\n  x-unrelated: keep\n";
        assert_eq!(header_values(body, "x-san-a"), ["forged", "duplicate"]);
        assert!(header_values(body, "x-san-b").is_empty());
        assert_eq!(header_values(body, "x-unrelated"), ["keep"]);
    }
}
