//! Shared dye-header (traffic tagging) application helpers.
//!
//! Selection-driven plugins call `strip_request_dye_headers` at the top of
//! `run_request` (after their kill-switch / protocol-gating checks) so a
//! client-supplied header with a configured dye name never reaches the
//! backend on any path, and `apply_dye_headers` exactly once at their
//! selection-success point. See
//! `docs/superpowers/specs/2026-07-10-dye-headers-unification-design.md`.

use edgion_resources::resources::edgion_plugins::plugin_configs::{DyeHeader, DyeHeaders};

use super::log::la;
use super::{PluginLog, PluginSession};

/// Value for one dye entry: the explicit static value, or the plugin's
/// resolved selection value when omitted.
fn effective_value<'a>(entry: &'a DyeHeader, resolved: &'a str) -> &'a str {
    entry.value.as_deref().unwrap_or(resolved)
}

/// Apply dye headers after a successful selection. `resolved` is the plugin's
/// selection value (profile name, `[ns/]name`, `domain:port`, endpoint addr,
/// region name), substituted for every entry with `value: None`.
///
/// Entries are applied in config order, `request` list first. Writes use the
/// session's insert (replace-all) semantics, so a later same-name entry wins.
/// A write failure is pushed to the `PluginLog` and never blocks the request.
pub fn apply_dye_headers(session: &mut dyn PluginSession, cfg: &DyeHeaders, resolved: &str, log: &mut PluginLog) {
    for entry in &cfg.request {
        if session
            .set_request_header(&entry.name, effective_value(entry, resolved))
            .is_err()
        {
            // "err:" prefix keeps the entry greppable alongside push_err output;
            // the name pinpoints which configured entry failed (the value may be
            // a runtime-resolved string with invalid header bytes).
            log.push_fmt("err: dye request header set failed: {}", la![entry.name.as_str()]);
        }
    }
    for entry in &cfg.response {
        if session
            .set_response_header(&entry.name, effective_value(entry, resolved))
            .is_err()
        {
            log.push_fmt("err: dye response header set failed: {}", la![entry.name.as_str()]);
        }
    }
}

/// Remove every request-direction dye header name from the incoming request,
/// so a client-supplied value can never pass through to the backend on a
/// non-dyed path (fallback / reject / no-route).
pub fn strip_request_dye_headers(session: &mut dyn PluginSession, cfg: &DyeHeaders) {
    for entry in &cfg.request {
        let _ = session.remove_request_header(&entry.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, value: Option<&str>) -> DyeHeader {
        DyeHeader {
            name: name.to_string(),
            value: value.map(|v| v.to_string()),
        }
    }

    #[test]
    fn effective_value_uses_resolved_when_value_omitted() {
        assert_eq!(effective_value(&entry("X-Release-Track", None), "canary"), "canary");
    }

    #[test]
    fn effective_value_prefers_explicit_static_value() {
        assert_eq!(
            effective_value(&entry("X-Served-By", Some("edge-1")), "canary"),
            "edge-1"
        );
    }
}
