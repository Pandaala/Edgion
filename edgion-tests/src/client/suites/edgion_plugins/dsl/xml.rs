//! Bounded XML wire coverage on the shared DSLBody topology.

use sha2::{Digest, Sha256};
use std::time::Instant;

use super::super::body_plugins::poll_mirrored_body;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};

const HOST: &str = "dsl-xml.example.com";

async fn client(ctx: &TestContext, h2: bool) -> Result<reqwest::Client, String> {
    let address = tokio::net::lookup_host((ctx.target_host.as_str(), ctx.http_port))
        .await
        .map_err(|error| error.to_string())?
        .next()
        .ok_or("gateway address is empty")?;
    let builder = reqwest::Client::builder().no_proxy().resolve(HOST, address);
    let builder = if h2 {
        builder.http2_prior_knowledge()
    } else {
        builder.http1_only()
    };
    builder.build().map_err(|error| error.to_string())
}

fn request(
    client: &reqwest::Client,
    ctx: &TestContext,
    path: &str,
    mode: &str,
    body: String,
) -> reqwest::RequestBuilder {
    client
        .post(format!(
            "http://{HOST}:{}/{}",
            ctx.http_port,
            path.trim_start_matches('/')
        ))
        .header("content-type", "application/xml")
        .header("x-xml-mode", mode)
        .body(body)
}

async fn echoed(response: reqwest::Response, expected: &str, h2: bool) -> Result<(), String> {
    if response.status().as_u16() != 200 || (h2 && response.version() != reqwest::Version::HTTP_2) {
        return Err(format!(
            "unexpected status/version: {} {:?}",
            response.status(),
            response.version()
        ));
    }
    let body = response.text().await.map_err(|error| error.to_string())?;
    let actual = body
        .split_once("Echo: ")
        .map(|(_, body)| body)
        .ok_or("missing echo marker")?;
    if actual != expected {
        return Err(format!(
            "replay mismatch: expected {} bytes, actual {}",
            expected.len(),
            actual.len()
        ));
    }
    Ok(())
}

fn contract() -> TestCase {
    TestCase::new(
        "dsl_xml_extract_construct_and_mapping",
        "XML namespaces, repeated children, explicit mapping and bodyless construction over H1/H2",
        |ctx| {
            Box::pin(async move {
                let start = Instant::now();
                let result = async {
            let soap = "<s:Envelope xmlns:s='urn:soap'><s:Body><s:name id='7'>alice</s:name><s:name>bob</s:name></s:Body></s:Envelope>";
            for h2 in [false, true] {
                let client = client(&ctx, h2).await?;
                echoed(request(&client, &ctx, "echo", "inspect", soap.into()).send().await.map_err(|e| e.to_string())?, soap, h2).await?;
                let response = request(&client, &ctx, "echo", "build", String::new()).header("x-name", "alice & bob").send().await.map_err(|e| e.to_string())?;
                echoed(response, "<request><name>alice &amp; bob</name></request>", h2).await?;
                let response = request(&client, &ctx, "echo", "build-json", "{\"name\":\"alice & bob\"}".into()).send().await.map_err(|e| e.to_string())?;
                echoed(response, "<request><name>alice &amp; bob</name></request>", h2).await?;
                let response = request(&client, &ctx, "echo", "json", "<r><name>alice &amp; bob</name></r>".into()).send().await.map_err(|e| e.to_string())?;
                echoed(response, "{\"nameBase64\":\"YWxpY2UgJiBib2I=\"}", h2).await?;
            }
            Ok::<_, String>(())
        }.await;
                match result {
                    Ok(()) => TestResult::passed(start.elapsed()),
                    Err(error) => TestResult::failed(start.elapsed(), error),
                }
            })
        },
    )
}

fn replay_and_retry() -> TestCase {
    TestCase::new(
        "dsl_xml_memory_spill_replay_and_retry",
        "XML inspection and normalized mutations replay exactly on H1/H2 and retry",
        |ctx| {
            Box::pin(async move {
                let start = Instant::now();
                let result = async {
                    for h2 in [false, true] {
                        let client = client(&ctx, h2).await?;
                        for size in [8, 256 * 1024] {
                            let text = "x".repeat(size);
                            let original = format!("<r><![CDATA[{text}]]></r>");
                            let normalized = format!("<r>{text}</r>");
                            let response = request(&client, &ctx, "echo", "observe", original.clone())
                                .send()
                                .await
                                .map_err(|e| e.to_string())?;
                            echoed(response, &original, h2).await?;
                            let response = request(&client, &ctx, "echo", "normalize", original.clone())
                                .send()
                                .await
                                .map_err(|e| e.to_string())?;
                            echoed(response, &normalized, h2).await?;
                            let id = uuid::Uuid::new_v4();
                            let response = request(
                                &client,
                                &ctx,
                                &format!("retry-sequence/xml-{id}/503,200"),
                                "normalize",
                                original,
                            )
                            .header(
                                "x-expected-body-sha256",
                                hex::encode(Sha256::digest(normalized.as_bytes())),
                            )
                            .send()
                            .await
                            .map_err(|e| e.to_string())?;
                            if response.status().as_u16() != 200
                                || response
                                    .headers()
                                    .get("x-attempt-number")
                                    .and_then(|value| value.to_str().ok())
                                    != Some("2")
                            {
                                return Err(format!(
                                    "XML retry failed: {} {:?}",
                                    response.status(),
                                    response.headers().get("x-attempt-number")
                                ));
                            }
                            response.bytes().await.map_err(|e| e.to_string())?;
                        }
                    }
                    Ok::<_, String>(())
                }
                .await;
                match result {
                    Ok(()) => TestResult::passed(start.elapsed()),
                    Err(error) => TestResult::failed(start.elapsed(), error),
                }
            })
        },
    )
}

fn mirrored_generation() -> TestCase {
    TestCase::new(
        "dsl_xml_mirror_receives_final_generation",
        "XML normalized upstream and mirror share identical bytes over H1/H2",
        |ctx| {
            Box::pin(async move {
                let start = Instant::now();
                let result = async {
                    for h2 in [false, true] {
                        let client = client(&ctx, h2).await?;
                        for size in [8, 256 * 1024] {
                            let marker = format!("dsl-xml-{}", uuid::Uuid::new_v4());
                            let text = "x".repeat(size);
                            // The shared capture endpoint is bounded at 4 KiB. A compact
                            // final generation still verifies replacement of spilled input.
                            let expected = "<v>&amp;</v>";
                            let response = request(
                                &client,
                                &ctx,
                                "mirror/capture",
                                "compact",
                                format!("<r><v><![CDATA[&]]></v><padding>{text}</padding></r>"),
                            )
                            .header("traceparent", crate::access_log_client::traceparent_for_marker(&marker))
                            .send()
                            .await
                            .map_err(|e| e.to_string())?;
                            let digest = hex::encode(Sha256::digest(expected.as_bytes()));
                            if response.status().as_u16() != 200
                                || response
                                    .headers()
                                    .get("x-captured-body-sha256")
                                    .and_then(|v| v.to_str().ok())
                                    != Some(digest.as_str())
                            {
                                return Err("upstream XML generation mismatch".into());
                            }
                            if h2 && response.version() != reqwest::Version::HTTP_2 {
                                return Err("H2 was not negotiated".into());
                            }
                            response.bytes().await.map_err(|e| e.to_string())?;
                            if poll_mirrored_body(&ctx, &marker).await? != expected {
                                return Err("mirror XML generation mismatch".into());
                            }
                        }
                    }
                    Ok::<_, String>(())
                }
                .await;
                match result {
                    Ok(()) => TestResult::passed(start.elapsed()),
                    Err(error) => TestResult::failed(start.elapsed(), error),
                }
            })
        },
    )
}

fn malformed_and_limits() -> TestCase {
    TestCase::new(
        "dsl_xml_security_and_hard_limits",
        "Malicious XML returns nil for explicit denial; structural and output limits fail closed despite Ignore",
        |ctx| {
            Box::pin(async move {
                let start = Instant::now();
                let result = async {
                    let client = client(&ctx, false).await?;
                    for source in [
                        "<!DOCTYPE r><r/>",
                        "<!DOCTYPE r SYSTEM 'file:///etc/passwd'><r/>",
                        "<!DOCTYPE r [<!ENTITY e 'boom'>]><r>&e;</r>",
                        "<r>&unknown;</r>",
                        "<a><b></a></b>",
                        "<?pi!data?><r/>",
                        "<?pi?data?><r/>",
                        "<xi:include xmlns:xi='http://www.w3.org/2001/XInclude' href='http://example.com'/>",
                    ] {
                        let response = request(&client, &ctx, "echo", "normalize", source.into())
                            .send()
                            .await
                            .map_err(|e| e.to_string())?;
                        if response.status().as_u16() != 400
                            || response.text().await.map_err(|e| e.to_string())? != "Invalid XML"
                        {
                            return Err("malformed XML denial mismatch".into());
                        }
                    }
                    for source in [
                        format!("<r>{}</r>", "x".repeat(1024 * 1024)),
                        format!("{}{}", "<r>".repeat(33), "</r>".repeat(33)),
                        format!("<r>{}</r>", "<c/>".repeat(4096)),
                        format!("<r><![CDATA[{}]]></r>", "&".repeat(256 * 1024)),
                    ] {
                        let response = request(&client, &ctx, "echo", "normalize", source)
                            .send()
                            .await
                            .map_err(|e| e.to_string())?;
                        if response.status().as_u16() != 500
                            || response.text().await.map_err(|e| e.to_string())? != "Request body processing failed"
                        {
                            return Err("XML hard-limit fixed denial mismatch".into());
                        }
                    }
                    Ok::<_, String>(())
                }
                .await;
                match result {
                    Ok(()) => TestResult::passed(start.elapsed()),
                    Err(error) => TestResult::failed(start.elapsed(), error),
                }
            })
        },
    )
}

pub(super) fn cases() -> Vec<TestCase> {
    vec![
        contract(),
        replay_and_retry(),
        mirrored_generation(),
        malformed_and_limits(),
    ]
}

pub struct DslXmlDynamicTestSuite {
    phase: &'static str,
}
impl DslXmlDynamicTestSuite {
    pub fn new(phase: &'static str) -> Self {
        Self { phase }
    }
}
impl TestSuite for DslXmlDynamicTestSuite {
    fn name(&self) -> &str {
        "DSL XML dynamic source and physical policy"
    }
    fn test_cases(&self) -> Vec<TestCase> {
        let phase = self.phase;
        vec![TestCase::new(
            format!("dsl_xml_dynamic_{phase}"),
            "XML source and resolved physical policy hot reload without partial publication",
            move |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        let response = ctx
                            .http_client
                            .post(format!("http://{}:{}/echo", ctx.target_host, ctx.http_port))
                            .header("host", "dsl-xml-dynamic.example.com")
                            .body("<r/>")
                            .send()
                            .await
                            .map_err(|e| e.to_string())?;
                        if phase == "capped" {
                            if response.status().as_u16() != 500
                                || response.text().await.map_err(|e| e.to_string())? != "Request body processing failed"
                            {
                                return Err("XML capped output did not fail closed".into());
                            }
                            let response = ctx
                                .http_client
                                .post(format!("http://{}:{}/echo", ctx.target_host, ctx.http_port))
                                .header("host", "dsl-xml-dynamic.example.com")
                                .body(format!("<r>{}</r>", "x".repeat(32)))
                                .send()
                                .await
                                .map_err(|e| e.to_string())?;
                            if response.status().as_u16() != 413 {
                                return Err("physical XML input cap did not reject before parse".into());
                            }
                            response.bytes().await.map_err(|e| e.to_string())?;
                        } else {
                            let tag = if phase == "initial" { "v1" } else { "v2" };
                            echoed(response, &format!("<{tag}>new-value</{tag}>"), false).await?;
                        }
                        Ok::<_, String>(())
                    }
                    .await;
                    match result {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )]
    }
}
