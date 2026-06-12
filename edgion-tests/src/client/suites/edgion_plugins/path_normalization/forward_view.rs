// PathNormalization Scenario 03 — ForwardView (upstream forwarding decision)
//
// Verifies which path the gateway forwards to upstream for a percent-encoded
// request, based on:
//   (a) default gateway behaviour (no annotation)
//   (b) per-route annotation `edgion.io/forward-raw-path: "true"` — forces raw forward
//   (c) per-route annotation `edgion.io/forward-raw-path: "false"` — forces normalized forward
//
// The test-http backend echoes the path it received in "Path: <value>" text, so
// assertions here verify the actual upstream-visible path, not just route reachability.
//
// ## Globally-scoped options (NOT automatable without gateway restart)
//
// Scenario 04: Global `forward_normalized_path = false` in gateway YAML makes ALL routes
//   forward raw by default.  Manual verification:
//
//   # Set in gateway YAML: path_normalization.forward_normalized_path: false
//   curl -i -H "Host: path-fwd-default.example.com" \
//     "http://127.0.0.1:31180/%61pi/data?x=1"
//   # Expected: backend echoes Path: /%61pi/data (raw — global default is now raw)
//
// Scenario 05: region_route with NO annotation inherits the global setting.
//   If forward_normalized_path = true (default), upstream receives normalized path even
//   when region_route's preserve_raw_path = true is set at the plugin level.  The
//   region_route plugin controls routing decisions, not upstream forwarding; the
//   annotation (or global flag) controls what path upstream sees.
//   Manual verification requires a working region_route fixture — see
//   edgion-tests/integration/conf/EdgionPlugins/RegionRoute/ for the region_route test setup.
//
// Covered by:
//   ForwardView/03_HTTPRoute_default-normalized-forward.yaml
//   ForwardView/03_HTTPRoute_annotation-raw-true.yaml
//   ForwardView/03_HTTPRoute_annotation-norm-false.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::collections::HashMap;
use std::time::Instant;

const HOST_DEFAULT: &str = "path-fwd-default.example.com";
const HOST_RAW: &str = "path-fwd-raw.example.com";
const HOST_NORM: &str = "path-fwd-norm.example.com";
const HOST_REWRITE: &str = "path-fwd-rewrite.example.com";
const HOST_INVALID: &str = "path-fwd-invalid.example.com";

/// Parse the plain-text echo response from test-http.
/// The backend response format is:
///   Method: GET
///   Path: /some/path
///   Headers:
///     Host: ...
fn parse_echo_path(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Path:") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Parse query string from echo response headers or path line.
/// test-http may echo query string as part of the Path field or separately.
fn parse_echo_query(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Query:") {
            return Some(rest.trim().to_string());
        }
    }
    // Query might be appended to path as "Path: /api/data?x=1" — extract after '?'
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Path:") {
            let path = rest.trim();
            if let Some(pos) = path.find('?') {
                return Some(path[pos + 1..].to_string());
            }
        }
    }
    None
}

/// Return a placeholder struct for unused header parsing (mirrors proxy_rewrite pattern).
#[derive(Debug, Default)]
#[allow(dead_code)]
struct EchoInfo {
    path: String,
    headers: HashMap<String, String>,
}

pub struct PathNormalizationForwardViewTestSuite;

impl PathNormalizationForwardViewTestSuite {
    // TC-A: default (no annotation) — upstream receives normalized path
    //
    // Request: /%61pi/data?x=1  (raw: %61 = 'a')
    // Route match: PathPrefix /api (uses normalized /api/data)
    // Forward: normalized /api/data (gateway default)
    // Expected backend path: /api/data
    fn tc_a_default_normalized() -> TestCase {
        TestCase::new(
            "path_norm_fwd_default_normalized",
            "TC-A: /%61pi/data (no annotation) → upstream receives normalized /api/data",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/%61pi/data?x=1", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", HOST_DEFAULT).send().await {
                        Ok(r) => {
                            let status = r.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "expected 200 (PathPrefix /api should match /%61pi after normalization), got {}",
                                        status
                                    ),
                                );
                            }
                            match r.text().await {
                                Ok(body) => {
                                    let echo_path = parse_echo_path(&body);
                                    match echo_path {
                                        Some(ref p) if p == "/api/data" => TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!(
                                                "upstream received normalized path: {} (query preserved: {:?})",
                                                p,
                                                parse_echo_query(&body)
                                            ),
                                        ),
                                        Some(ref p) if p.starts_with("/api/data") => {
                                            // Path may include query string in some echo formats
                                            TestResult::passed_with_message(
                                                start.elapsed(),
                                                format!("upstream received normalized path (with query): {}", p),
                                            )
                                        }
                                        Some(ref p) => TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "expected upstream path /api/data, got: {} (raw was /%61pi/data)",
                                                p
                                            ),
                                        ),
                                        None => TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "could not parse Path from echo response (status 200); body: {}",
                                                body
                                            ),
                                        ),
                                    }
                                }
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("could not read response body: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    }
                })
            },
        )
    }

    // TC-B: annotation forward-raw-path: "true" — upstream receives raw path
    //
    // Request: /%61pi/data?x=1  (raw: %61 = 'a')
    // Route match: PathPrefix /api (uses normalized /api/data)
    // Forward: raw /%61pi/data (annotation overrides global default)
    // Expected backend path: /%61pi/data
    fn tc_b_annotation_raw_true() -> TestCase {
        TestCase::new(
            "path_norm_fwd_annotation_raw_true",
            "TC-B: /%61pi/data (forward-raw-path: true) → upstream receives raw /%61pi/data",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/%61pi/data?x=1", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", HOST_RAW).send().await {
                        Ok(r) => {
                            let status = r.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "expected 200 (PathPrefix /api should match /%61pi after normalization), got {}",
                                        status
                                    ),
                                );
                            }
                            match r.text().await {
                                Ok(body) => {
                                    let echo_path = parse_echo_path(&body);
                                    match echo_path {
                                        Some(ref p) if p == "/%61pi/data" => TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("upstream received raw path: {}", p),
                                        ),
                                        Some(ref p) if p.starts_with("/%61pi/data") => TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("upstream received raw path (with query): {}", p),
                                        ),
                                        Some(ref p) => TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "expected upstream raw path /%61pi/data, got: {} \
                                                 (annotation forward-raw-path: true should preserve encoding)",
                                                p
                                            ),
                                        ),
                                        None => TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "could not parse Path from echo response (status 200); body: {}",
                                                body
                                            ),
                                        ),
                                    }
                                }
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("could not read response body: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    }
                })
            },
        )
    }

    // TC-C: annotation forward-raw-path: "false" — upstream receives normalized path
    //
    // Request: /%61pi/data?x=1  (raw: %61 = 'a')
    // Route match: PathPrefix /api (uses normalized /api/data)
    // Forward: normalized /api/data (annotation explicitly forces normalization)
    //
    // This annotation also overrides region_route's preserve_raw_path = true if both
    // are configured together (documented in fixture YAML).
    //
    // Expected backend path: /api/data
    fn tc_c_annotation_norm_false() -> TestCase {
        TestCase::new(
            "path_norm_fwd_annotation_norm_false",
            "TC-C: /%61pi/data (forward-raw-path: false) → upstream receives normalized /api/data",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/%61pi/data?x=1", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", HOST_NORM).send().await {
                        Ok(r) => {
                            let status = r.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "expected 200 (PathPrefix /api should match /%61pi after normalization), got {}",
                                        status
                                    ),
                                );
                            }
                            match r.text().await {
                                Ok(body) => {
                                    let echo_path = parse_echo_path(&body);
                                    match echo_path {
                                        Some(ref p) if p == "/api/data" => TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!(
                                                "upstream received normalized path: {} \
                                                     (annotation forward-raw-path: false forces normalization)",
                                                p
                                            ),
                                        ),
                                        Some(ref p) if p.starts_with("/api/data") => TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("upstream received normalized path (with query): {}", p),
                                        ),
                                        Some(ref p) => TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "expected upstream normalized path /api/data, got: {} \
                                                 (annotation forward-raw-path: false should force normalization)",
                                                p
                                            ),
                                        ),
                                        None => TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "could not parse Path from echo response (status 200); body: {}",
                                                body
                                            ),
                                        ),
                                    }
                                }
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("could not read response body: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    }
                })
            },
        )
    }

    // TC-D: rewrite filter wins over annotation (spec §14 channel 0 priority)
    //
    // Route has BOTH:
    //   - Gateway API URLRewrite filter (replacePrefixMatch /api → /rewritten)
    //   - edgion.io/forward-raw-path: "true" annotation
    //
    // When the URLRewrite filter executes, session.set_upstream_uri() is called,
    // which sets ctx.uri_finalized = true. The single-point decision then sees
    // uri_finalized and SKIPS the block — annotation has no effect.
    //
    // Request: /%61pi/data?q=1
    // After URLRewrite: /rewritten/data (rewrite is applied; query preserved by pingora)
    // Expected backend path: /rewritten/data (NOT raw /%61pi/data which annotation alone
    // would produce, NOT normalized /api/data — the rewrite result is terminal).
    fn tc_d_rewrite_overrides_annotation() -> TestCase {
        TestCase::new(
            "path_norm_fwd_rewrite_overrides_annotation",
            "TC-D: rewrite filter + forward-raw-path annotation → upstream receives rewrite result (uri_finalized wins over annotation)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/%61pi/data?q=1", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", HOST_REWRITE).send().await {
                        Ok(r) => {
                            let status = r.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "expected 200 (PathPrefix /api should match /%61pi after normalization, then URLRewrite to /rewritten), got {}",
                                        status
                                    ),
                                );
                            }
                            match r.text().await {
                                Ok(body) => match parse_echo_path(&body) {
                                    Some(ref p) if p == "/rewritten/data" || p.starts_with("/rewritten/data") => {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!(
                                                "rewrite wins over annotation: upstream received {} (annotation 'true' was correctly ignored because URLRewrite already finalized the URI)",
                                                p
                                            ),
                                        )
                                    }
                                    Some(ref p) if p.starts_with("/%61pi") => TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "annotation incorrectly applied — got raw {} (rewrite should have won; uri_finalized priority broken)",
                                            p
                                        ),
                                    ),
                                    Some(ref p) if p.starts_with("/api/data") => TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "single-point decision incorrectly applied — got normalized {} (rewrite should have written /rewritten/data first)",
                                            p
                                        ),
                                    ),
                                    Some(ref p) => TestResult::failed(
                                        start.elapsed(),
                                        format!("unexpected upstream path {}, expected /rewritten/data", p),
                                    ),
                                    None => TestResult::failed(
                                        start.elapsed(),
                                        format!("could not parse Path from echo response; body: {}", body),
                                    ),
                                },
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("read response body failed: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    }
                })
            },
        )
    }

    // TC-E: multi-step normalization chains end-to-end to upstream.
    //
    // Request: /%61pi//foo/./bar?q=1
    // Triggers all algorithm steps:
    //   - percent-decode: %61 → 'a'    → /api//foo/./bar
    //   - merge_slashes:  //  → /      → /api/foo/./bar
    //   - dot-segment:    /./ → /      → /api/foo/bar
    //   (strip_path_parameters: no-op, no `;` in input)
    //
    // Default route (HOST_DEFAULT, no annotation), so all 3 steps must propagate
    // through to what the upstream actually receives.
    //
    // Expected backend path: /api/foo/bar (query ?q=1 preserved)
    fn tc_e_multi_step_normalization() -> TestCase {
        TestCase::new(
            "path_norm_fwd_multi_step_normalization",
            "TC-E: /%61pi//foo/./bar (percent + merge + dot-segment) → upstream receives /api/foo/bar",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/%61pi//foo/./bar?q=1", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", HOST_DEFAULT).send().await {
                        Ok(r) => {
                            let status = r.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "expected 200 (PathPrefix /api should match after multi-step normalization), got {}",
                                        status
                                    ),
                                );
                            }
                            match r.text().await {
                                Ok(body) => match parse_echo_path(&body) {
                                    Some(ref p) if p == "/api/foo/bar" || p.starts_with("/api/foo/bar") => {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!(
                                                "all 3 normalization steps propagated: {} (query: {:?})",
                                                p,
                                                parse_echo_query(&body)
                                            ),
                                        )
                                    }
                                    Some(ref p) => TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "expected upstream path /api/foo/bar (raw was /%61pi//foo/./bar), got: {}",
                                            p
                                        ),
                                    ),
                                    None => TestResult::failed(
                                        start.elapsed(),
                                        format!("could not parse Path from echo response; body: {}", body),
                                    ),
                                },
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("read response body failed: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    }
                })
            },
        )
    }

    // TC-F: invalid annotation value falls through to default flow.
    //
    // Route has annotation `edgion.io/forward-raw-path: "yes"` (invalid — only
    // "true" / "false" are accepted). The controller's preparse logs INFO and
    // treats the value as None, which means default flow applies.
    //
    // Request: /%61pi/data?x=1
    // Expected: upstream receives /api/data (normalized — same as no-annotation TC-A),
    //   NOT raw /%61pi/data (which would happen if "yes" were accepted as truthy).
    fn tc_f_invalid_annotation_falls_through() -> TestCase {
        TestCase::new(
            "path_norm_fwd_invalid_annotation_falls_through",
            "TC-F: invalid annotation value 'yes' → falls through to default (normalized forwarding)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/%61pi/data?x=1", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", HOST_INVALID).send().await {
                        Ok(r) => {
                            let status = r.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("expected 200 (route should still match), got {}", status),
                                );
                            }
                            match r.text().await {
                                Ok(body) => match parse_echo_path(&body) {
                                    Some(ref p) if p == "/api/data" || p.starts_with("/api/data") => {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!(
                                                "invalid annotation correctly fell through to default: upstream received {} (NOT raw /%61pi/data which would mean 'yes' was misinterpreted as truthy)",
                                                p
                                            ),
                                        )
                                    }
                                    Some(ref p) if p.starts_with("/%61pi") => TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "annotation 'yes' was incorrectly treated as truthy — got raw {} (should fall through to default normalized)",
                                            p
                                        ),
                                    ),
                                    Some(ref p) => TestResult::failed(
                                        start.elapsed(),
                                        format!("unexpected upstream path {}, expected /api/data", p),
                                    ),
                                    None => TestResult::failed(
                                        start.elapsed(),
                                        format!("could not parse Path from echo response; body: {}", body),
                                    ),
                                },
                                Err(e) => {
                                    TestResult::failed(start.elapsed(), format!("read response body failed: {}", e))
                                }
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for PathNormalizationForwardViewTestSuite {
    fn name(&self) -> &str {
        "PathNormalization (ForwardView — upstream path forwarding decision)"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::tc_a_default_normalized(),
            Self::tc_b_annotation_raw_true(),
            Self::tc_c_annotation_norm_false(),
            Self::tc_d_rewrite_overrides_annotation(),
            Self::tc_e_multi_step_normalization(),
            Self::tc_f_invalid_annotation_falls_through(),
        ]
    }
}
