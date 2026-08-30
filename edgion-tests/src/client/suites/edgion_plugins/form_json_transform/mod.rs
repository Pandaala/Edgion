use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const HOST: &str = "form-json-transform-dynamic.example.com";

fn echo_body(text: &str) -> Option<&str> {
    text.split_once("Echo: ").map(|(_, body)| body)
}

pub struct FormJsonTransformDynamicFormTestSuite;

impl TestSuite for FormJsonTransformDynamicFormTestSuite {
    fn name(&self) -> &str {
        "FormJsonTransform Dynamic - Form to JSON"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "form_json_dynamic_form_to_json",
            "The initial direction transforms form input to JSON",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match ctx
                        .http_client
                        .post(format!("http://{}:31180/echo", ctx.target_host))
                        .header("host", HOST)
                        .header("content-type", "application/x-www-form-urlencoded")
                        .body("b=2&a=1")
                        .send()
                        .await
                    {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            if echo_body(&text) == Some(r#"{"a":"1","b":"2"}"#) {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Unexpected echo: {text}"))
                            }
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )]
    }
}

pub struct FormJsonTransformDynamicJsonTestSuite;

impl TestSuite for FormJsonTransformDynamicJsonTestSuite {
    fn name(&self) -> &str {
        "FormJsonTransform Dynamic - JSON to Form"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![TestCase::new(
            "form_json_dynamic_json_to_form",
            "A hot reload changes the direction to JSON-to-form",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match ctx
                        .http_client
                        .post(format!("http://{}:31180/echo", ctx.target_host))
                        .header("host", HOST)
                        .header("content-type", "application/json")
                        .body(r#"{"b":"2","a":"1"}"#)
                        .send()
                        .await
                    {
                        Ok(response) if response.status().as_u16() == 200 => {
                            let text = response.text().await.unwrap_or_default();
                            if echo_body(&text) == Some("a=1&b=2") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Unexpected echo: {text}"))
                            }
                        }
                        Ok(response) => {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )]
    }
}
