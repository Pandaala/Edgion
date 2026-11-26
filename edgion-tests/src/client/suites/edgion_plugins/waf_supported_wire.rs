//! Exact product CRS profile exercised through the canonical shared topology.
use super::json_wire::{verify_structured_with_user_agent, StructuredBody};
use super::*;
const VERSION: &str = "supported-crs";
const SUPPORTED_HOST: &str = "waf-supported.example.com";
const GUARD_ID: u64 = 9900302;
struct Vector {
    name: &'static str,
    content_type: &'static str,
    body: &'static str,
    path: &'static str,
    rule: Option<u64>,
    malformed: bool,
}
fn vectors(strict: bool) -> Vec<Vector> {
    [
 ("json-clean","application/json",r#"{"probe":"safe"}"#,"/waf-wire",None,false),
 ("xml-clean","application/xml","<r a='safe'>safe</r>","/waf-wire",None,false),
 ("json-php","application/json",r#"{"probe":"phar://payload"}"#,"/waf-wire",Some(933200),false),
 ("json-name","application/json",r#"{"phar://payload":"safe"}"#,"/waf-wire",Some(933200),false),
 ("json-escaped","application/json",r#"{"user":{"name":"\u0070har\u003a//payload"}}"#,"/waf-wire",Some(933200),false),
 ("xml-text","application/xml","<r>phar://payload</r>","/waf-wire",Some(933200),false),
 ("xml-attribute","application/xml","<r a='phar://payload'/>","/waf-wire",Some(933200),false),
 ("xml-entity","application/xml","<r>&#112;har://payload</r>","/waf-wire",Some(933200),false),
 ("json-java","application/json",r#"{"probe":"springframework.context.support.FileSystemXmlApplicationContext"}"#,"/waf-wire",strict.then_some(944260),false),
 ("json-duplicate-first-hit","application/json",r#"{"probe":"springframework.context.support.FileSystemXmlApplicationContext","probe":"safe"}"#,"/waf-wire",strict.then_some(944260),false),
 ("query","application/json","{}","/waf-wire?probe=phar%3A%2F%2Fpayload",Some(933200),false),
 ("form","application/x-www-form-urlencoded","probe=phar%3A%2F%2Fpayload","/waf-wire",Some(933200),false),
 ("multipart","multipart/form-data; boundary=crs-boundary","--crs-boundary\r\nContent-Disposition: form-data; name=\"probe\"\r\n\r\nphar://payload\r\n--crs-boundary--\r\n","/waf-wire",Some(933200),false),
 ("json-malformed","application/json",r#"{"probe":"safe""#,"/waf-wire",None,true),
 ("xml-malformed","application/xml","<r>safe</wrong>","/waf-wire",None,true),
 ].into_iter().map(|(name,content_type,body,path,rule,malformed)|Vector{name,content_type,body,path,rule,malformed}).collect()
}
fn status(v: &Vector, detection: bool) -> u16 {
    if detection {
        200
    } else if v.malformed {
        400
    } else if v.rule.is_some() {
        403
    } else {
        200
    }
}
async fn logged(ctx: &TestContext, v: &Vector, strict: bool, detection: bool) -> Result<(), String> {
    let trace = format!("waf-supported-{}", uuid::Uuid::new_v4());
    let run = async {
        let path = v.path.replace("/waf-wire", "/echo");
        let response = ctx
            .http_client
            .post(format!("http://{}:{}{path}", ctx.target_host, ctx.http_port))
            .timeout(Duration::from_secs(10))
            .header("host", SUPPORTED_HOST)
            .header("x-waf-version", VERSION)
            .header("content-type", v.content_type)
            .header("user-agent", "edgion-profile-test")
            .header("access_log", "test_store")
            .header("traceparent", crate::access_log_client::traceparent_for_marker(&trace))
            .body(v.body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let actual = response.status().as_u16();
        response.bytes().await.map_err(|e| e.to_string())?;
        let log = ctx
            .access_log_client()
            .get_access_log_with_retry(&trace, 15, 200)
            .await
            .map_err(|e| e.to_string())?;
        let waf = &log.data["waf"];
        if actual != status(v, detection)
            || log.status_code() != Some(u64::from(status(v, detection)))
            || waf["mode"] != if detection { "detection_only" } else { "on" }
        {
            return Err(format!(
                "{} status/mode mismatch: wire={actual}, expected={}, waf={waf}",
                v.name,
                status(v, detection)
            ));
        }
        let findings = waf["findings"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        if findings.iter().any(|f| f["rule_id"] == GUARD_ID) {
            return Err("test-owned publication guard matched".into());
        }
        if v.malformed {
            if findings
                .iter()
                .any(|f| matches!(f["rule_id"].as_u64(), Some(933200 | 944260)))
                || waf["findings_omitted"].as_u64().unwrap_or(0) != 0
            {
                return Err("malformed safe input committed provisional attack findings".into());
            }
            if detection {
                // PL4 checks the original raw body after malformed fallback;
                // JSON/XML punctuation triggers original 920273, then 949110.
                for id in [920273, 949110] {
                    let present = findings
                        .iter()
                        .any(|f| f["rule_id"] == id && f["phase"] == "request_body");
                    if present != strict {
                        return Err(format!("malformed raw fallback rule {id} differs from selected PL"));
                    }
                }
                if !waf["terminal_error"].is_null()
                    || !waf["intervention"].is_null()
                    || waf["phases"]["request_body"].is_null()
                {
                    return Err("DetectionOnly malformed fallback did not complete phase2".into());
                }
            } else if waf["terminal_error"]["code"] != "body_processing"
                || waf["terminal_error"]["phase"] != "request_body"
                || waf["outcome"] != "error"
            {
                return Err(format!("malformed failure was not parser failure: {waf}"));
            }
        }
        if !v.malformed {
            if !waf["terminal_error"].is_null() || waf["findings_omitted"].as_u64().unwrap_or(0) != 0 {
                return Err(format!("{} valid request had terminal error/omissions: {waf}", v.name));
            }
            if let Some(id) = v.rule {
                for expected in [id, 949110] {
                    if !findings
                        .iter()
                        .any(|f| f["rule_id"] == expected && f["phase"] == "request_body")
                    {
                        return Err(format!("{} missing original {expected}: {waf}", v.name));
                    }
                }
            } else if findings.iter().any(|f| f["rule_id"] == 949110) {
                return Err(format!("{} unexpected anomaly threshold: {waf}", v.name));
            }
            if v.rule.is_some()
                && !detection
                && (waf["intervention"]["rule_id"] != 949110 || waf["outcome"] != "blocked")
            {
                return Err("missing original threshold intervention".into());
            }
            if detection && !waf["intervention"].is_null() {
                return Err("DetectionOnly disrupted request".into());
            }
        }
        Ok(())
    };
    let result = tokio::time::timeout(Duration::from_secs(15), run)
        .await
        .unwrap_or_else(|_| Err("supported trace assertion timed out".into()));
    let cleanup = tokio::time::timeout(Duration::from_secs(5), cleanup_response_deny_access_log(ctx, &trace))
        .await
        .unwrap_or_else(|_| Err("supported trace cleanup timed out".into()));
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(e), Ok(())) | (Ok(()), Err(e)) => Err(e),
        (Err(e), Err(c)) => Err(format!("{e}; cleanup={c}")),
    }
}
async fn status_probe(ctx: &TestContext, path: &str, strict: bool, detection: bool, dummy: bool) -> Result<(), String> {
    let trace = format!("waf-supported-response-{}", uuid::Uuid::new_v4());
    let run = async {
        let mut request = ctx
            .http_client
            .get(format!("http://{}:{}{path}", ctx.target_host, ctx.http_port))
            .timeout(Duration::from_secs(10))
            .header("host", SUPPORTED_HOST)
            .header("x-waf-version", VERSION)
            .header(
                "user-agent",
                if dummy {
                    "Apache (internal dummy connection)"
                } else {
                    "edgion-profile-test"
                },
            )
            .header("access_log", "test_store")
            .header("traceparent", crate::access_log_client::traceparent_for_marker(&trace));
        let k8s_peer = if dummy
            && std::env::var("EDGION_TEST_K8S_MODE").is_ok_and(|mode| matches!(mode.as_str(), "true" | "1" | "yes"))
        {
            Some(crate::suites::k8s_peer_identity::current()?)
        } else {
            None
        };
        if dummy {
            request = request.header("cookie", "probe=phar://payload");
            if k8s_peer.is_some() {
                // The fixture already trusts this physical Pod; only RemoteIp
                // is resolved from this header. Never replace DirectPeerIp.
                request = request.header("x-forwarded-for", "127.0.0.1");
            }
        }
        let response = request
            .version(reqwest::Version::HTTP_11)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if dummy && response.version() != reqwest::Version::HTTP_11 {
            return Err("Apache probe must use downstream H1".into());
        }
        let actual = response.status().as_u16();
        let body = response.bytes().await.map_err(|e| e.to_string())?;
        let expected = if dummy {
            if detection {
                404
            } else {
                403
            }
        } else if strict && !detection {
            403
        } else {
            500
        };
        let log = ctx
            .access_log_client()
            .get_access_log_with_retry(&trace, 15, 200)
            .await
            .map_err(|e| e.to_string())?;
        let waf = &log.data["waf"];
        if actual != expected || log.status_code() != Some(u64::from(expected)) || !waf["terminal_error"].is_null() {
            return Err(format!("status probe {path}: expected {expected}, got {actual}; {waf}"));
        }
        if waf["mode"] != if detection { "detection_only" } else { "on" }
            || waf["findings_omitted"].as_u64().unwrap_or(0) != 0
        {
            return Err("status probe mode/omissions mismatch".into());
        }
        if dummy {
            let remote = log.data["request_info"]["remote_ip"]
                .as_str()
                .ok_or("Apache probe remote IP missing")?
                .parse::<std::net::IpAddr>()
                .map_err(|e| e.to_string())?;
            if let Some(peer) = &k8s_peer {
                let direct = log.data["request_info"]["direct_peer_ip"]
                    .as_str()
                    .ok_or("Apache probe physical peer missing")?
                    .parse::<std::net::IpAddr>()
                    .map_err(|error| error.to_string())?;
                if direct != peer.ip || remote != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) {
                    return Err("Apache probe must retain the actual Pod peer and trusted loopback RemoteIp".into());
                }
            }
            if !remote.is_loopback() || log.data["request_info"]["path"] != "/" {
                return Err("Apache probe did not satisfy loopback/root conditions".into());
            }
        }
        let findings = waf["findings"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        if findings.iter().any(|f| f["rule_id"] == GUARD_ID) {
            return Err("status probe version guard matched".into());
        }
        if !dummy {
            if strict && !detection {
                if waf["intervention"]["rule_id"] != 959100
                    || waf["intervention"]["phase"] != "response_body"
                    || waf["intervention"]["status"] != 403
                {
                    return Err("missing original response-status intervention".into());
                }
            } else if !waf["intervention"].is_null() {
                return Err("unexpected response-status intervention".into());
            }
        }
        for (id, phase, hit) in if dummy {
            vec![(933200, "request_body", true), (949110, "request_body", true)]
        } else {
            vec![(950100, "response_headers", strict), (959100, "response_body", strict)]
        } {
            if findings.iter().any(|f| f["rule_id"] == id && f["phase"] == phase) != hit {
                return Err(format!("status probe missing/unexpected {id}/{phase}: {waf}"));
            }
        }
        if !dummy
            && (detection || !strict)
            && ((path.contains("status-empty") && !body.is_empty())
                || (!path.contains("status-empty") && body.as_ref() != b"Status: 500"))
        {
            return Err("backend 500 bytes changed".into());
        }
        Ok(())
    };
    let result = tokio::time::timeout(Duration::from_secs(15), run)
        .await
        .unwrap_or_else(|_| Err("status probe timed out".into()));
    let cleanup = tokio::time::timeout(Duration::from_secs(5), cleanup_response_deny_access_log(ctx, &trace))
        .await
        .unwrap_or_else(|_| Err("status trace cleanup timed out".into()));
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(e), Ok(())) | (Ok(()), Err(e)) => Err(e),
        (Err(e), Err(c)) => Err(format!("{e}; {c}")),
    }
}
async fn latency(ctx: &TestContext, strict: bool, detection: bool) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    for (name, ctype, body, attack) in [
        ("json-clean", "application/json", r#"{"probe":"safe"}"#, false),
        ("json-attack", "application/json", r#"{"probe":"phar://payload"}"#, true),
        ("xml-clean", "application/xml", "<r a='safe'>safe</r>", false),
        ("xml-attack", "application/xml", "<r a='phar://payload'/>", true),
    ] {
        let expected = if attack && !detection { 403 } else { 200 };
        let mut samples = Vec::with_capacity(31);
        // Reuse the client; timed requests do not enable the access-log test store.
        for index in 0..35 {
            if Instant::now() >= deadline {
                return Err("HTTP latency sampling exceeded 30s budget".into());
            }
            let start = Instant::now();
            let response = ctx
                .http_client
                .post(format!("http://{}:{}/echo", ctx.target_host, ctx.http_port))
                .timeout(Duration::from_secs(10))
                .header("host", SUPPORTED_HOST)
                .header("x-waf-version", VERSION)
                .header("content-type", ctype)
                .header("user-agent", "edgion-profile-test")
                .body(body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let actual = response.status().as_u16();
            response.bytes().await.map_err(|e| e.to_string())?;
            if actual != expected {
                return Err(format!("HTTP preflight/sample outcome changed: {name} {actual}"));
            }
            if (3..34).contains(&index) {
                samples.push(start.elapsed().as_nanos());
            }
        }
        println!(
            "SUPPORTED_HTTP_LATENCY {}",
            serde_json::json!({"profile":if strict{"strict"}else{"standard"},"mode":if detection{"detectionOnly"}else{"on"},"case":name,"body_bytes":body.len(),"expected_status":expected,"warmups":3,"samples_ns":samples,"transport":"downstream-H1/upstream-H1","includes":"HTTP, Gateway callback, upstream echo and drain; System is not selected for live Gateway"})
        );
    }
    Ok(())
}
pub(super) fn matrix(strict: bool, detection: bool, measure: bool) -> TestCase {
    TestCase::new(
        "waf_supported_crs_wire",
        "exact supported CRS profile: original findings, H1/H2, publication and response-header/status policy",
        move |ctx: TestContext| {
            Box::pin(async move {
                let started = Instant::now();
                let run = async {
                    let cases = vectors(strict);
                    for v in &cases {
                        if started.elapsed() > Duration::from_secs(120) {
                            return Err("supported matrix exceeded work deadline".into());
                        }
                        for upstream_h2 in [false, true] {
                            for downstream_h2 in [false, true] {
                                verify_structured_with_user_agent(
                                    &ctx,
                                    upstream_h2,
                                    downstream_h2,
                                    VERSION,
                                    v.path,
                                    StructuredBody {
                                        content_type: v.content_type,
                                        bytes: v.body.as_bytes(),
                                    },
                                    status(v, detection),
                                    Some("edgion-profile-test"),
                                )
                                .await
                                .map_err(|e| format!("{}: {e}", v.name))?;
                            }
                        }
                        logged(&ctx, v, strict, detection).await?;
                    }
                    for path in ["/status/500", "/status-empty/500"] {
                        if started.elapsed() > Duration::from_secs(120) {
                            return Err("status checks exceeded work deadline".into());
                        }
                        status_probe(&ctx, path, strict, detection, false).await?;
                    }
                    status_probe(&ctx, "/", strict, detection, true).await?;
                    if measure {
                        if started.elapsed() > Duration::from_secs(170) {
                            return Err("latency admission exceeded work deadline".into());
                        }
                        latency(&ctx, strict, detection).await?;
                    }
                    println!(
                        "Supported CRS {} {}: {} vectors, {} transport checks, {} logged checks; HTTP samples={}",
                        if strict { "strict" } else { "standard" },
                        if detection { "DetectionOnly" } else { "On" },
                        cases.len(),
                        cases.len() * 4,
                        cases.len() + 3,
                        if measure { 124 } else { 0 }
                    );
                    Ok::<(), String>(())
                };
                match run.await {
                    Ok(()) => TestResult::passed(started.elapsed()),
                    Err(e) => TestResult::failed(started.elapsed(), e),
                }
            })
        },
    )
    .with_timeout(Duration::from_secs(240))
}
