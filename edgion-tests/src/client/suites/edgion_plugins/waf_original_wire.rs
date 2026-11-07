//! Original CRS predicates, scoring and enforcement through the shared topology.

use super::json_wire::{verify_structured, StructuredBody};
use super::*;

pub(crate) const ORIGINAL_DIGEST: &str = "1eddcdfa0cbaa2299e1e2ddbdb44d2eb6a1c0c6f388226ecd4b6c078167923af";
const VERSION: &str = "original-crs";
const ATTACK_IDS: [u64; 3] = [933200, 944260, 920370];

struct Vector {
    name: &'static str,
    content_type: &'static str,
    body: String,
    rule: Option<u64>,
}

fn vectors() -> Vec<Vector> {
    let mut vectors = [
        ("json-clean", "application/json", r#"{"probe":"clean"}"#, None),
        ("xml-clean", "application/xml", "<r value='clean'>safe</r>", None),
        (
            "json-php",
            "application/json",
            r#"{"probe":"phar://file"}"#,
            Some(933200),
        ),
        (
            "json-java",
            "application/json",
            r#"{"probe":"springframework.context.support.FileSystemXmlApplicationContext"}"#,
            Some(944260),
        ),
        ("xml-php-text", "application/xml", "<r>phar://file</r>", Some(933200)),
        (
            "xml-java-text",
            "application/xml",
            "<r>springframework.context.support.FileSystemXmlApplicationContext</r>",
            Some(944260),
        ),
        (
            "xml-php-attribute",
            "application/xml",
            "<r value='phar://file'/>",
            Some(933200),
        ),
        (
            "xml-java-attribute",
            "application/xml",
            "<r value='springframework.context.support.FileSystemXmlApplicationContext'/>",
            Some(944260),
        ),
        (
            "json-php-name",
            "application/json",
            r#"{"phar://file":"clean"}"#,
            Some(933200),
        ),
        (
            "json-php-escaped-nested",
            "application/json",
            r#"{"user":{"name":"\u0070har\u003a//file"}}"#,
            Some(933200),
        ),
        (
            "xml-php-entity",
            "application/xml",
            "<r>&#112;har://file</r>",
            Some(933200),
        ),
        (
            "json-java-duplicate-first-hit",
            "application/json",
            r#"{"probe":"springframework.context.support.FileSystemXmlApplicationContext","probe":"clean"}"#,
            Some(944260),
        ),
    ]
    .into_iter()
    .map(|(name, content_type, body, rule)| Vector {
        name,
        content_type,
        body: body.to_string(),
        rule,
    })
    .collect::<Vec<_>>();
    for (name, length, rule) in [
        ("json-length-boundary", 400, None),
        ("json-length-over", 401, Some(920370)),
        ("json-length-scratch-spill", 2048, Some(920370)),
    ] {
        vectors.push(Vector {
            name,
            content_type: "application/json",
            body: format!("{{\"probe\":\"{}\"}}", "a".repeat(length)),
            rule,
        });
    }
    vectors
}

fn status(vector: &Vector, detection: bool) -> u16 {
    if vector.rule.is_some() && !detection {
        403
    } else {
        200
    }
}

async fn verify_logged(ctx: &TestContext, vector: &Vector, detection: bool) -> Result<(), String> {
    let trace = format!("waf-original-{}", uuid::Uuid::new_v4());
    let result = async {
        let response = ctx
            .http_client
            .post(format!("http://{}:{}/test/waf-dynamic", ctx.target_host, ctx.http_port))
            .timeout(Duration::from_secs(10))
            .header("host", HOST)
            .header("x-waf-version", VERSION)
            .header("content-type", vector.content_type)
            .header("access_log", "test_store")
            .header("traceparent", crate::access_log_client::traceparent_for_marker(&trace))
            .body(vector.body.clone())
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let wire_status = response.status().as_u16();
        response.bytes().await.map_err(|error| error.to_string())?;
        let log = ctx
            .access_log_client()
            .get_access_log_with_retry(&trace, 15, 200)
            .await
            .map_err(|error| error.to_string())?;
        let waf = &log.data["waf"];
        let expected_status = status(vector, detection);
        if wire_status != expected_status
            || log.status_code() != Some(u64::from(expected_status))
            || waf["mode"] != if detection { "detection_only" } else { "on" }
            || !waf["terminal_error"].is_null()
            || waf["findings_omitted"].as_u64().unwrap_or(0) != 0
            || waf["phases"]["request_body"]["omitted_findings"].as_u64() != Some(0)
        {
            return Err(format!(
                "{} original profile status/mode/error/omission mismatch",
                vector.name
            ));
        }
        let findings = waf["findings"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        // Loggable guards cannot silently establish a structured hit in DetectionOnly.
        if findings
            .iter()
            .any(|finding| matches!(finding["rule_id"].as_u64(), Some(9900202 | 9900203)))
        {
            return Err(format!("{} version or raw-source guard matched", vector.name));
        }
        for id in ATTACK_IDS {
            let found = findings
                .iter()
                .any(|finding| finding["rule_id"].as_u64() == Some(id) && finding["phase"] == "request_body");
            if found != (vector.rule == Some(id)) {
                return Err(format!(
                    "{} original finding {id} expected {}, got {found}",
                    vector.name,
                    vector.rule == Some(id)
                ));
            }
        }
        let scored = findings
            .iter()
            .any(|finding| finding["rule_id"].as_u64() == Some(949110) && finding["phase"] == "request_body");
        if scored != vector.rule.is_some() {
            return Err(format!("{} original anomaly evaluation finding mismatch", vector.name));
        }
        if vector.rule.is_some() && !detection {
            if waf["intervention"]["rule_id"].as_u64() != Some(949110)
                || waf["intervention"]["phase"] != "request_body"
                || waf["intervention"]["status"].as_u64() != Some(403)
                || waf["outcome"] != "blocked"
            {
                return Err(format!("{} missing original anomaly intervention", vector.name));
            }
        } else if !waf["intervention"].is_null()
            || waf["outcome"] != if vector.rule.is_some() { "detected" } else { "allowed" }
        {
            return Err(format!("{} expected non-disruptive original outcome", vector.name));
        }
        Ok(())
    };
    let result = tokio::time::timeout(Duration::from_secs(15), result)
        .await
        .unwrap_or_else(|_| Err("original access-log assertion timed out".into()));
    let cleanup = tokio::time::timeout(Duration::from_secs(5), cleanup_response_deny_access_log(ctx, &trace))
        .await
        .unwrap_or_else(|_| Err("original access-log cleanup timed out".into()));
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; {cleanup}")),
    }
}

pub(super) fn matrix(detection: bool) -> TestCase {
    TestCase::new(
        "waf_original_crs_structured_wire",
        "unchanged CRS findings, scoring, body preservation and denial through Controller/Gateway",
        move |ctx: TestContext| {
            Box::pin(async move {
                let started = Instant::now();
                let run = async {
                    let vectors = vectors();
                    for vector in &vectors {
                        for upstream_h2 in [false, true] {
                            for downstream_h2 in [false, true] {
                                if started.elapsed() > Duration::from_secs(90) {
                                    return Err("original CRS matrix exceeded its work deadline".to_string());
                                }
                                verify_structured(
                                    &ctx,
                                    upstream_h2,
                                    downstream_h2,
                                    VERSION,
                                    "/waf-wire",
                                    StructuredBody {
                                        content_type: vector.content_type,
                                        bytes: vector.body.as_bytes(),
                                    },
                                    status(vector, detection),
                                )
                                .await
                                .map_err(|error| format!("{}: {error}", vector.name))?;
                            }
                        }
                        if started.elapsed() > Duration::from_secs(90) {
                            return Err("original CRS matrix exceeded its work deadline".to_string());
                        }
                        verify_logged(&ctx, vector, detection).await?;
                    }
                    println!(
                        "Original CRS mode {}: {} vectors, {} transport assertions, {} access-log assertions",
                        if detection { "DetectionOnly" } else { "On" },
                        vectors.len(),
                        vectors.len() * 4,
                        vectors.len()
                    );
                    Ok::<(), String>(())
                };
                // Access-log requests have their own timeout followed by always-run trace cleanup.
                // Avoid cancelling that cleanup with a surrounding matrix timeout.
                match run.await {
                    Ok(()) => TestResult::passed(started.elapsed()),
                    Err(error) => TestResult::failed(started.elapsed(), error),
                }
            })
        },
    )
    .with_timeout(Duration::from_secs(180))
}
