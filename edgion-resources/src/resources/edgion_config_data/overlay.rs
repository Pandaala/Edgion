//! Generic overlay framework for EdgionConfigData.
//!
//! Implements the `apply_override` contract from the overlay design (§1):
//! - **Type guard first (D6):** verify the overlay's `ConfigEntry` variant matches the expected
//!   variant before applying; mismatch → return base unchanged.
//! - **Field-level replace, collections whole-replace; no deep-merge.**
//! - **Fail-to-base on any rejection** (absent, type mismatch, invalid).

/// Signal returned by [`apply_override`] describing the outcome of overlay resolution.
///
/// Consumers should forward the signal to the appropriate observability channel
/// (access-log `EdgionErrorCode` or a Prometheus counter) when the outcome is not
/// `Applied`. Do **not** emit `tracing::error!` / `tracing::warn!` from the data plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlaySignal {
    /// No overlay entry was provided; base config is used as-is.
    Absent,
    /// An overlay entry was present but its `type` discriminator did not match
    /// the variant the consumer expects (D6 type guard). Base config is used.
    TypeMismatch,
    /// The overlay matched the expected variant and was successfully applied.
    Applied,
}

/// Apply an optional typed overlay on top of a base config, with a type guard.
///
/// # Type guard (D6)
/// `extract` receives the raw overlay entry `E` and returns `Some(&O)` when the
/// overlay's variant tag matches what the consumer expects, or `None` on a
/// type mismatch.  A mismatch causes `apply_override` to return the base
/// unchanged with [`OverlaySignal::TypeMismatch`].
///
/// In production usage `E` will be [`super::ConfigEntry`]; for unit tests (including
/// the toy fixtures below) any enum can stand in.
///
/// # Field-level merge semantics
/// `merge` is only called when the type guard passes.  It receives the base by
/// value and a reference to the typed overlay, and must implement:
/// - For each `Option`-wrapped override field: `Some(v)` replaces the base
///   field; `None` keeps the base field.
/// - Collections (slices, `Vec`, `HashMap`) are replaced **whole** — never
///   element-wise merged.  Selective element patching is a separately-named
///   operation and must never be implicit merge.
///
/// # Parameters
/// - `base` — the complete, safe-default base config (moved into the function).
/// - `overlay_entry` — the raw overlay entry, if one was resolved.
/// - `extract` — type-guard extractor: `(&E) -> Option<&O>`.
/// - `merge` — applies the typed override onto the base: `(B, &O) -> B`.
///
/// # Returns
/// `(effective, signal)`:
/// - `effective` is the merged result or the original base (on absent/mismatch).
/// - `signal` describes what happened (for observability).
pub fn apply_override<B, E, O, ExtractFn, MergeFn>(
    base: B,
    overlay_entry: Option<&E>,
    extract: ExtractFn,
    merge: MergeFn,
) -> (B, OverlaySignal)
where
    ExtractFn: FnOnce(&E) -> Option<&O>,
    MergeFn: FnOnce(B, &O) -> B,
{
    match overlay_entry {
        None => (base, OverlaySignal::Absent),
        Some(entry) => match extract(entry) {
            None => (base, OverlaySignal::TypeMismatch),
            Some(overlay) => {
                let effective = merge(base, overlay);
                (effective, OverlaySignal::Applied)
            }
        },
    }
}

// ===== Unit tests =====
//
// Uses a self-contained toy Base/Override/Entry fixture so the overlay framework can be
// exercised independently of any real EdgionConfigData variant. `RegionRouteOverride` is
// the remaining production user of this framework.

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_config_data/overlay_test.rs"]
mod tests;
