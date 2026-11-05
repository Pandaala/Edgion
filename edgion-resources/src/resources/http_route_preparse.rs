//! Runtime-only pre-parsed structures for internal processing.
//! The enclosing route fields that hold these values carry `#[serde(skip)]`,
//! so they never appear in the conf_sync payload.

use super::http_route::MirrorTuning;
use super::HTTPRoute;
use crate::path_normalization::{normalize::canonicalize_path_prefix, normalize_config_path};

/// Pre-parse methods for HTTPRoute.
impl HTTPRoute {
    /// Normalize path matcher values and parse per-rule annotations
    /// (`forward-raw-path`, `retry-non-idempotent`).
    ///
    /// This method should be called after deserializing HTTPRoute from YAML/JSON.
    ///
    /// # Incomplete by itself
    ///
    /// `preparse()` only covers path-matcher normalization and the annotations
    /// above. It does **not** parse timeouts or retry configurations. A fully
    /// pre-processed `HTTPRoute` requires all three calls:
    ///
    /// ```ignore
    /// route.preparse();
    /// route.parse_timeouts();
    /// route.parse_retry();
    /// ```
    ///
    /// Production code paths go through `ResourceMeta::pre_parse()` (implemented
    /// via `impl_resource_meta!` in `types/resource/meta/impls.rs`), which chains
    /// all three. Prefer `pre_parse()` over calling `preparse()` directly unless
    /// you intentionally want only the subset above.
    pub fn preparse(&mut self) {
        // Normalize path matcher values before any other processing so downstream
        // logic always sees the canonical path representation.
        self.normalize_path_matchers();

        // Parse `edgion.io/forward-raw-path` annotation. Three-state:
        // "true" / "false" / absent. Invalid values logged INFO and treated as None.
        let parsed_forward_raw_path: Option<bool> = self
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(crate::constants::annotations::edgion::FORWARD_RAW_PATH))
            .and_then(|v| match v.as_str() {
                "true" => Some(true),
                "false" => Some(false),
                other => {
                    tracing::info!(
                        route_namespace = %self.metadata.namespace.as_deref().unwrap_or("default"),
                        route_name = %self.metadata.name.as_deref().unwrap_or(""),
                        annotation = %crate::constants::annotations::edgion::FORWARD_RAW_PATH,
                        value = %other,
                        "ignored: expected \"true\" or \"false\""
                    );
                    None
                }
            });

        // Parse `edgion.io/retry-non-idempotent` annotation. Opt-in: "true"
        // re-enables replaying non-idempotent request bodies on the
        // reused-connection retry path; any other value (or absence) keeps the
        // safe default where non-idempotent methods are not retried there.
        let parsed_allow_non_idempotent_retry: Option<bool> = self
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(crate::constants::annotations::edgion::RETRY_NON_IDEMPOTENT))
            .and_then(|v| match v.as_str() {
                "true" => Some(true),
                "false" => Some(false),
                other => {
                    tracing::info!(
                        route_namespace = %self.metadata.namespace.as_deref().unwrap_or("default"),
                        route_name = %self.metadata.name.as_deref().unwrap_or(""),
                        annotation = %crate::constants::annotations::edgion::RETRY_NON_IDEMPOTENT,
                        value = %other,
                        "ignored: expected \"true\" or \"false\""
                    );
                    None
                }
            });

        // Parse `edgion.io/mirror-*` tuning annotations into a MirrorTuning,
        // applied to every RequestMirror filter under this route. These knobs
        // live in annotations because the vendored Gateway API CRD silently
        // prunes undeclared filter fields in K8s conf-center mode.
        let parsed_mirror_tuning = self.parse_mirror_tuning_annotations();

        if let Some(rules) = self.spec.rules.as_mut() {
            for rule in rules.iter_mut() {
                rule.parsed_forward_raw_path = parsed_forward_raw_path;
                rule.parsed_allow_non_idempotent_retry = parsed_allow_non_idempotent_retry;
                rule.parsed_mirror_tuning = parsed_mirror_tuning.clone();
            }
        }
    }

    /// Parse the `edgion.io/mirror-*` tuning annotations. Returns `None` when
    /// no mirror annotation is present (defaults apply). Each invalid value is
    /// logged at INFO and falls back to that field's default, matching the
    /// lenient handling of the sibling route annotations.
    fn parse_mirror_tuning_annotations(&self) -> Option<MirrorTuning> {
        use crate::constants::annotations::edgion;

        let annotations = self.metadata.annotations.as_ref()?;

        let keys = [
            edgion::MIRROR_CONNECT_TIMEOUT_MS,
            edgion::MIRROR_WRITE_TIMEOUT_MS,
            edgion::MIRROR_CHANNEL_FULL_TIMEOUT_MS,
            edgion::MIRROR_MAX_BUFFERED_CHUNKS,
            edgion::MIRROR_MAX_BUFFERED_BYTES,
            edgion::MIRROR_LOG,
            edgion::MIRROR_MAX_CONCURRENT,
        ];
        if !keys.iter().any(|k| annotations.contains_key(*k)) {
            return None;
        }

        let route_namespace = self.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = self.metadata.name.as_deref().unwrap_or("");

        // Parse one annotation value; absent or invalid falls back to `default`.
        macro_rules! parse_or_default {
            ($key:expr, $default:expr, $ty:ty) => {
                match annotations.get($key) {
                    None => $default,
                    Some(v) => match v.parse::<$ty>() {
                        Ok(parsed) => parsed,
                        Err(_) => {
                            tracing::info!(
                                route_namespace = %route_namespace,
                                route_name = %route_name,
                                annotation = %$key,
                                value = %v,
                                "ignored: invalid value, using default"
                            );
                            $default
                        }
                    },
                }
            };
        }

        let defaults = MirrorTuning::default();
        let mut tuning = MirrorTuning {
            connect_timeout_ms: parse_or_default!(edgion::MIRROR_CONNECT_TIMEOUT_MS, defaults.connect_timeout_ms, u64),
            write_timeout_ms: parse_or_default!(edgion::MIRROR_WRITE_TIMEOUT_MS, defaults.write_timeout_ms, u64),
            channel_full_timeout_ms: parse_or_default!(
                edgion::MIRROR_CHANNEL_FULL_TIMEOUT_MS,
                defaults.channel_full_timeout_ms,
                u64
            ),
            max_buffered_chunks: parse_or_default!(
                edgion::MIRROR_MAX_BUFFERED_CHUNKS,
                defaults.max_buffered_chunks,
                usize
            ),
            max_buffered_bytes: annotations
                .get(edgion::MIRROR_MAX_BUFFERED_BYTES)
                .cloned()
                .unwrap_or(defaults.max_buffered_bytes),
            mirror_log: parse_or_default!(edgion::MIRROR_LOG, defaults.mirror_log, bool),
            max_concurrent: parse_or_default!(edgion::MIRROR_MAX_CONCURRENT, defaults.max_concurrent, usize),
        };
        if let Err(error) = tuning.resolve_limits() {
            tracing::info!(
                route_namespace = %route_namespace,
                route_name = %route_name,
                error = %error,
                "ignored: invalid mirror capacities, using defaults for invalid fields"
            );
            tuning.normalize_limits();
        }
        Some(tuning)
    }

    /// Parse and pre-process timeout configurations for all rules
    ///
    /// This method is called during route loading (in pre_parse) to avoid runtime parsing overhead.
    /// It parses timeout strings into Duration objects and stores them in rule.parsed_timeouts.
    pub fn parse_timeouts(&mut self) {
        if let Some(rules) = self.spec.rules.as_mut() {
            for rule in rules.iter_mut() {
                // Parse timeouts for each rule
                if let Some(timeouts) = &rule.timeouts {
                    rule.parsed_timeouts = crate::resources::http_route::ParsedRouteTimeouts::from_config(timeouts);

                    if rule.parsed_timeouts.is_some() {
                        tracing::debug!("Parsed route-level timeouts for HTTPRoute rule");
                    }
                }
            }
        }
    }

    /// Normalize path matcher values for all rules at load time.
    ///
    /// For `Exact` and `PathPrefix` match types, applies [`normalize_config_path`] so the
    /// stored value inhabits the same normalized space as the request-side path produced by
    /// Group E (which reads `ctx.path()`, the already-normalized current path).
    ///
    /// `RegularExpression` patterns are left unchanged — regex patterns cannot be
    /// machine-normalized; correctness is the operator's responsibility.
    pub fn normalize_path_matchers(&mut self) {
        let route_key = format!(
            "{}/{}",
            self.metadata.namespace.as_deref().unwrap_or("default"),
            self.metadata.name.as_deref().unwrap_or("")
        );

        let path_cfg = crate::path_normalization::get_path_normalization_config();
        if let Some(rules) = self.spec.rules.as_mut() {
            for rule in rules.iter_mut() {
                let Some(matches) = rule.matches.as_mut() else {
                    continue;
                };
                for m in matches.iter_mut() {
                    let Some(path) = m.path.as_mut() else {
                        continue;
                    };
                    // Only normalize Exact and PathPrefix; skip RegularExpression and unknown types.
                    let match_type = path.match_type.as_deref().unwrap_or("PathPrefix");
                    if match_type == "Exact" || match_type == "PathPrefix" {
                        if let Some(value) = path.value.as_mut() {
                            let mut normalized = normalize_config_path(value, path_cfg);
                            if match_type == "PathPrefix" {
                                canonicalize_path_prefix(&mut normalized);
                            }
                            if normalized != *value {
                                tracing::info!(
                                    route = %route_key,
                                    match_type = %match_type,
                                    original = %value,
                                    normalized = %normalized,
                                    "HTTPRoute path matcher value normalized at load time"
                                );
                                *value = normalized;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Parse and pre-process retry configurations for all rules
    pub fn parse_retry(&mut self) {
        let route_key = format!(
            "{}/{}",
            self.metadata.namespace.as_deref().unwrap_or("default"),
            self.metadata.name.as_deref().unwrap_or("")
        );

        if let Some(rules) = self.spec.rules.as_mut() {
            for rule in rules.iter_mut() {
                let Some(retry) = &rule.retry else {
                    continue;
                };

                let attempts = retry.attempts.map(|a| a.max(0) as u32);

                let backoff = retry.backoff.as_deref().and_then(|s| {
                    crate::duration::parse_duration(s)
                        .map_err(|e| {
                            tracing::warn!(
                                route = %route_key,
                                value = %s,
                                error = %e,
                                "Invalid retry backoff duration, backoff disabled"
                            );
                        })
                        .ok()
                });

                let codes = retry.codes.as_ref().map(|codes| {
                    let mut seen = [false; 200];
                    let mut parsed = Vec::with_capacity(codes.len().min(seen.len()));
                    for &code in codes {
                        // Retry status codes are restricted to 400..=599 to match
                        // the upstream Gateway API HTTPRouteRetry CRD validation.
                        // Informational (1xx) and success/redirect (2xx/3xx) codes
                        // are never a meaningful retry trigger and a 1xx code here
                        // would otherwise let retry-on-status misfire on an h1
                        // informational header.
                        if (400..=599).contains(&code) {
                            let index = (code - 400) as usize;
                            if !seen[index] {
                                seen[index] = true;
                                parsed.push(code as u16);
                            }
                        } else {
                            tracing::warn!(code, "Invalid HTTP status code in retry.codes, ignoring");
                        }
                    }
                    parsed
                });

                if attempts.is_some() || backoff.is_some() || codes.is_some() {
                    rule.parsed_retry = Some(crate::resources::http_route::ParsedRouteRetry {
                        attempts,
                        backoff,
                        codes,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/resources/http_route_preparse_test.rs"]
mod tests;
