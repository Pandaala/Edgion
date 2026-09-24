//! Conditional filter wrappers for condition-based plugin execution
//!
//! This module provides wrapper types that add condition evaluation to any filter.
//! Conditions are evaluated before the inner filter runs, allowing plugins to be
//! skipped based on runtime context (headers, path, time, probability, etc.)

use async_trait::async_trait;

use std::sync::Arc;
use std::time::Duration;

use super::dye_plan::CompiledDyePlan;
use super::log::PluginLog;
use super::traits::{
    PluginSession, RequestFilter, UpstreamResponse, UpstreamResponseBodyFilter, UpstreamResponseFilter,
};
use crate::plugins::runtime::conditions::{CompiledPluginConditions, EvaluationResult, PluginConditions};
use edgion_resources::filters::PluginRunningResult;

/// Wrapper that adds condition evaluation to a RequestFilter
///
/// Before running the inner filter, conditions are evaluated:
/// - If conditions is `None`, or the container has neither `skip` nor `run`,
///   the inner filter runs unconditionally. Explicit empty `allOf`/`anyOf`
///   sets are rejected by canonical validation before runtime publication.
/// - If conditions evaluate to Skip, the filter is skipped and the reason is
///   recorded in [`PluginLog::cond_skip`].
/// - If conditions evaluate to Run, the inner filter executes normally
pub struct ConditionalRequestFilter {
    inner: Box<dyn RequestFilter>,
    conditions: Option<CompiledPluginConditions>,
    alias: Option<String>,
    dye_plan: Option<Arc<CompiledDyePlan>>,
}

impl ConditionalRequestFilter {
    pub(crate) fn bind_rate_limit_dependencies(&mut self, aliases: &std::collections::HashMap<&str, usize>) {
        if let Some(conditions) = &mut self.conditions {
            conditions.bind_rate_limit_dependencies(aliases);
        }
    }
    /// Create a new conditional request filter
    pub fn new(inner: Box<dyn RequestFilter>, conditions: Option<PluginConditions>) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias: None,
            dye_plan: None,
        }
    }

    /// Create a wrapper that also carries an operator-defined alias and an
    /// optional compiled dye plan.
    pub fn with_alias_and_dye(
        inner: Box<dyn RequestFilter>,
        conditions: Option<PluginConditions>,
        alias: Option<String>,
        dye_plan: Option<Arc<CompiledDyePlan>>,
    ) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias,
            dye_plan,
        }
    }
}

#[async_trait]
impl RequestFilter for ConditionalRequestFilter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    fn dye_plan(&self) -> Option<&Arc<CompiledDyePlan>> {
        self.dye_plan.as_ref()
    }

    fn suppress_automatic_log(&self) -> bool {
        self.inner.suppress_automatic_log()
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if let Some(conditions) = &self.conditions {
            if conditions.should_evaluate() {
                let eval = match conditions.evaluate_detail_async(session, log).await {
                    Ok(eval) => eval,
                    Err(result) => return result,
                };
                if eval.result == EvaluationResult::Skip {
                    if let Some(cause) = eval.cause {
                        log.set_cond_skip(format!("{}:{cause}", eval.action));
                    }
                    return PluginRunningResult::Nothing;
                }
            }
        }
        self.inner.run_request(session, log).await
    }
}

/// Wrapper that adds condition evaluation to an UpstreamResponseFilter (sync)
pub struct ConditionalUpstreamResponseFilter {
    inner: Box<dyn UpstreamResponseFilter>,
    conditions: Option<CompiledPluginConditions>,
    alias: Option<String>,
    dye_plan: Option<Arc<CompiledDyePlan>>,
}

impl ConditionalUpstreamResponseFilter {
    /// Create a new conditional upstream response filter
    pub fn new(inner: Box<dyn UpstreamResponseFilter>, conditions: Option<PluginConditions>) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias: None,
            dye_plan: None,
        }
    }

    /// Create a wrapper that also carries an operator-defined alias and an
    /// optional compiled dye plan.
    pub fn with_alias_and_dye(
        inner: Box<dyn UpstreamResponseFilter>,
        conditions: Option<PluginConditions>,
        alias: Option<String>,
        dye_plan: Option<Arc<CompiledDyePlan>>,
    ) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias,
            dye_plan,
        }
    }
}

impl UpstreamResponseFilter for ConditionalUpstreamResponseFilter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    fn dye_plan(&self) -> Option<&Arc<CompiledDyePlan>> {
        self.dye_plan.as_ref()
    }

    fn suppress_automatic_log(&self) -> bool {
        self.inner.suppress_automatic_log()
    }

    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        if let Some(conditions) = &self.conditions {
            if conditions.should_evaluate() {
                let eval = conditions.evaluate_detail(session, log);
                if eval.result == EvaluationResult::Skip {
                    if let Some(cause) = eval.cause {
                        log.set_cond_skip(format!("{}:{cause}", eval.action));
                    }
                    return PluginRunningResult::Nothing;
                }
            }
        }
        self.inner.run_upstream_response_filter(session, log)
    }
}

/// Wrapper that adds condition evaluation to an UpstreamResponseBodyFilter (sync)
///
/// Before running the inner filter, conditions are evaluated:
/// - If conditions is `None`, or the container has neither `skip` nor `run`,
///   the inner filter runs unconditionally. Explicit empty `allOf`/`anyOf`
///   sets are rejected by canonical validation before runtime publication.
/// - If conditions evaluate to Skip, the filter is skipped and returns `None`
///   without recording a per-chunk log.
/// - If conditions evaluate to Run, the inner filter executes normally
pub struct ConditionalUpstreamResponseBodyFilter {
    inner: Box<dyn UpstreamResponseBodyFilter>,
    conditions: Option<CompiledPluginConditions>,
    alias: Option<String>,
    dye_plan: Option<Arc<CompiledDyePlan>>,
}

impl ConditionalUpstreamResponseBodyFilter {
    /// Create a new conditional upstream response body filter
    pub fn new(inner: Box<dyn UpstreamResponseBodyFilter>, conditions: Option<PluginConditions>) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias: None,
            dye_plan: None,
        }
    }

    /// Create a wrapper that also carries an operator-defined alias and an
    /// optional compiled dye plan.
    pub fn with_alias_and_dye(
        inner: Box<dyn UpstreamResponseBodyFilter>,
        conditions: Option<PluginConditions>,
        alias: Option<String>,
        dye_plan: Option<Arc<CompiledDyePlan>>,
    ) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias,
            dye_plan,
        }
    }
}

impl UpstreamResponseBodyFilter for ConditionalUpstreamResponseBodyFilter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    fn dye_plan(&self) -> Option<&Arc<CompiledDyePlan>> {
        self.dye_plan.as_ref()
    }

    fn run_upstream_response_body_filter(
        &self,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
        session: &mut dyn PluginSession,
    ) -> Option<Duration> {
        // Body stage runs per chunk and does not surface PluginLog entries, so we
        // use the log-less evaluate variant. Skip-cause is intentionally not
        // recorded; emitting it per chunk would explode log volume.
        if let Some(conditions) = &self.conditions {
            if conditions.should_evaluate() && conditions.evaluate(session) == EvaluationResult::Skip {
                return None;
            }
        }

        self.inner
            .run_upstream_response_body_filter(body, end_of_stream, session)
    }
}

/// Wrapper that adds condition evaluation to an UpstreamResponse (async)
pub struct ConditionalUpstreamResponse {
    inner: Box<dyn UpstreamResponse>,
    conditions: Option<CompiledPluginConditions>,
    alias: Option<String>,
    dye_plan: Option<Arc<CompiledDyePlan>>,
}

impl ConditionalUpstreamResponse {
    /// Create a new conditional upstream response handler
    pub fn new(inner: Box<dyn UpstreamResponse>, conditions: Option<PluginConditions>) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias: None,
            dye_plan: None,
        }
    }

    /// Create a wrapper that also carries an operator-defined alias and an
    /// optional compiled dye plan.
    pub fn with_alias_and_dye(
        inner: Box<dyn UpstreamResponse>,
        conditions: Option<PluginConditions>,
        alias: Option<String>,
        dye_plan: Option<Arc<CompiledDyePlan>>,
    ) -> Self {
        Self {
            inner,
            conditions: conditions.map(CompiledPluginConditions::new),
            alias,
            dye_plan,
        }
    }
}

#[async_trait]
impl UpstreamResponse for ConditionalUpstreamResponse {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    fn dye_plan(&self) -> Option<&Arc<CompiledDyePlan>> {
        self.dye_plan.as_ref()
    }

    fn suppress_automatic_log(&self) -> bool {
        self.inner.suppress_automatic_log()
    }

    async fn run_upstream_response(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if let Some(conditions) = &self.conditions {
            if conditions.should_evaluate() {
                let eval = match conditions.evaluate_detail_async(session, log).await {
                    Ok(eval) => eval,
                    Err(result) => return result,
                };
                if eval.result == EvaluationResult::Skip {
                    if let Some(cause) = eval.cause {
                        log.set_cond_skip(format!("{}:{cause}", eval.action));
                    }
                    return PluginRunningResult::Nothing;
                }
            }
        }
        self.inner.run_upstream_response(session, log).await
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/conditional_filter_test.rs"]
mod tests;
