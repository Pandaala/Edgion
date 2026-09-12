//! Wire assertions for first-wins configured header names and effective precedence.

use super::HttpMatchTestSuite;
use crate::framework::{TestCase, TestContext, TestResult};
use anyhow::{ensure, Result};
use std::time::{Duration, Instant};

impl HttpMatchTestSuite {
    pub(crate) async fn check_duplicate_header_request(
        ctx: &TestContext,
        host: &str,
        path: &str,
        headers: &[(&str, &str)],
        backend_port: Option<u16>,
    ) -> Result<()> {
        let mut request = ctx
            .http_client
            .get(format!("{}{path}", ctx.http_url()))
            .header("host", host)
            .timeout(Duration::from_secs(3));
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request.send().await?;
        let expected_status = if backend_port.is_some() { 200 } else { 404 };
        ensure!(
            response.status().as_u16() == expected_status,
            "{path} with {headers:?}: expected {expected_status}, got {}",
            response.status()
        );
        if let Some(port) = backend_port {
            ensure!(
                response.text().await?.starts_with(&format!("Server: 0.0.0.0:{port}\n")),
                "{path} with {headers:?}: expected backend {port}"
            );
        }
        Ok(())
    }

    pub(super) fn duplicate_header_cases() -> Vec<TestCase> {
        type Probe = (&'static str, &'static [(&'static str, &'static str)], Option<u16>);
        let groups: &[(&str, &[Probe])] = &[
            (
                "duplicate_header_first_wins",
                &[
                    ("/echo/first", &[("x-mode", "one")], Some(30001)),
                    ("/echo/first", &[("x-mode", "two")], None),
                    ("/echo/first", &[], None),
                    ("/echo/reversed", &[("x-mode", "two")], Some(30002)),
                    ("/echo/reversed", &[("x-mode", "one")], None),
                ],
            ),
            (
                "duplicate_header_priority",
                &[("/echo/priority", &[("x-mode", "one")], Some(30001))],
            ),
            (
                "duplicate_header_distinct_names_and",
                &[
                    ("/echo/and", &[("x-mode", "one"), ("x-tenant", "blue")], Some(30001)),
                    ("/echo/and", &[("x-mode", "one")], None),
                    ("/echo/and", &[("x-tenant", "blue")], None),
                    ("/echo/and", &[("x-mode", "one"), ("x-tenant", "red")], None),
                ],
            ),
            (
                "duplicate_header_regex_alignment",
                &[
                    ("/echo/regex", &[("x-mode", "one"), ("x-version", "v12")], Some(30001)),
                    ("/echo/regex", &[("x-mode", "one"), ("x-version", "bad")], None),
                    ("/echo/regex", &[("x-mode", "one")], None),
                    ("/echo/regex-first", &[("x-mode", "v12")], Some(30002)),
                    ("/echo/regex-first", &[("x-mode", "other")], None),
                ],
            ),
            (
                "duplicate_header_independent_or_matches",
                &[
                    ("/echo/or", &[("x-mode", "one")], Some(30001)),
                    ("/echo/or", &[("x-mode", "two")], Some(30001)),
                    ("/echo/or", &[("x-mode", "ignored")], None),
                    ("/echo/or", &[], None),
                ],
            ),
        ];
        groups
            .iter()
            .map(|(name, probes)| {
                let probes = probes.to_vec();
                TestCase::new(
                    *name,
                    "Configured duplicate header wire regression",
                    move |ctx: TestContext| {
                        let probes = probes.clone();
                        Box::pin(async move {
                            let start = Instant::now();
                            for (path, headers, port) in probes {
                                if let Err(error) = Self::check_duplicate_header_request(
                                    &ctx,
                                    "duplicate-header.example.com",
                                    path,
                                    headers,
                                    port,
                                )
                                .await
                                {
                                    return TestResult::failed(start.elapsed(), error.to_string());
                                }
                            }
                            TestResult::passed(start.elapsed())
                        })
                    },
                )
                .with_timeout(Duration::from_secs(20))
            })
            .collect()
    }
}
