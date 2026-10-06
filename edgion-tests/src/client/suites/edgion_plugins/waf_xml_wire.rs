//! XML wire, canonical replay and source-generation pinning in the shared topology.
use super::json_wire::{verify_structured, PausedStructuredUpload, StructuredBody};
use super::*;

pub(crate) async fn paused(ctx: &TestContext) -> Result<PausedStructuredUpload, String> {
    PausedStructuredUpload::start(ctx, "application/xml", "xml-v1", b"<r>blo&#x", b"63;ked</r>", 406).await
}
pub(super) fn matrix(version: &'static str, detection: bool) -> TestCase {
    TestCase::new(
        if detection {
            "waf_xml_detection_wire"
        } else if version == "xml-v1" {
            "waf_xml_v1_wire"
        } else {
            "waf_xml_v2_wire"
        },
        "XML selection rejects before upstream or replays exact bytes over H1/H2",
        move |ctx: TestContext| {
            Box::pin(async move {
                let started = Instant::now();
                let run = async {
                    let marker = if version == "xml-v2" { "forbidden" } else { "blocked" };
                    let text = format!("<r>{marker}</r>");
                    let attr = format!("<r mode='{marker}'/>");
                    let cdata = format!("<r><![CDATA[{marker}]]></r>");
                    let malformed = format!("<r>{marker}</wrong>");
                    let opt_in = format!("{version}-opt-in");
                    let denied = if detection { 200 } else { 406 };
                    let large = format!("<r>{}</r>", "x".repeat(192 * 1024));
                    for downstream_h2 in [false, true] {
                        for upstream_h2 in [false, true] {
                            for (sent_version, path, content_type, body, status) in [
                                (version, "/waf-wire", "application/xml", b"<r>safe</r>".as_slice(), 200),
                                (version, "/waf-wire", "application/xml", text.as_bytes(), denied),
                                (version, "/waf-wire", "application/xml", attr.as_bytes(), 200),
                                (opt_in.as_str(), "/waf-wire", "application/xml", attr.as_bytes(), denied),
                                (version, "/waf-wire", "text/plain", text.as_bytes(), denied),
                                (version, "/waf-wire", "application/xml", cdata.as_bytes(), denied),
                                (
                                    version,
                                    "/waf-wire",
                                    "application/xml",
                                    malformed.as_bytes(),
                                    if detection { 200 } else { 500 },
                                ),
                                (
                                    version,
                                    "/waf-wire",
                                    "application/xml",
                                    b"<r a='quoted > value'><![CDATA[<!DOCTYPE harmless>]]></r>",
                                    200,
                                ),
                                (version, "/waf-wire", "application/xml", large.as_bytes(), 200),
                                (version, "/waf-wire", "application/xml", b"", 200),
                                (
                                    version,
                                    "/waf-wire",
                                    "application/xml",
                                    b" ",
                                    if detection { 200 } else { 500 },
                                ),
                                (
                                    version,
                                    "/waf-wire?q=query-deny",
                                    "application/xml",
                                    text.as_bytes(),
                                    if detection { 200 } else { 409 },
                                ),
                            ] {
                                verify_structured(
                                    &ctx,
                                    upstream_h2,
                                    downstream_h2,
                                    sent_version,
                                    path,
                                    StructuredBody {
                                        content_type,
                                        bytes: body,
                                    },
                                    status,
                                )
                                .await?;
                            }
                            if version == "xml-v2" {
                                verify_structured(
                                    &ctx,
                                    upstream_h2,
                                    downstream_h2,
                                    version,
                                    "/waf-wire",
                                    StructuredBody {
                                        content_type: "application/xml",
                                        bytes: b"<r>blocked</r>",
                                    },
                                    200,
                                )
                                .await?;
                            }
                            if !detection {
                                verify_structured(
                                    &ctx,
                                    upstream_h2,
                                    downstream_h2,
                                    "stale-xml",
                                    "/waf-wire",
                                    StructuredBody {
                                        content_type: "application/xml",
                                        bytes: b"<r/>",
                                    },
                                    451,
                                )
                                .await?;
                            }
                        }
                    }
                    if version == "xml-v1" && !detection {
                        paused(&ctx).await?.finish().await?;
                        paused(&ctx).await?.abort().await?;
                        verify_structured(
                            &ctx,
                            false,
                            false,
                            version,
                            "/waf-wire",
                            StructuredBody {
                                content_type: "application/xml",
                                bytes: b"<r>safe</r>",
                            },
                            200,
                        )
                        .await?;
                        PausedStructuredUpload::start(
                            &ctx,
                            "application/xml",
                            "xml-v1-opt-in",
                            b"<r a='blo&#",
                            b"99;ked'/>",
                            406,
                        )
                        .await?
                        .finish()
                        .await?;
                    }
                    Ok::<(), String>(())
                };
                match tokio::time::timeout(Duration::from_secs(90), run).await {
                    Ok(Ok(())) => TestResult::passed(started.elapsed()),
                    Ok(Err(error)) => TestResult::failed(started.elapsed(), error),
                    Err(_) => {
                        TestResult::failed(started.elapsed(), "XML wire matrix exceeded its deadline".to_string())
                    }
                }
            })
        },
    )
}
