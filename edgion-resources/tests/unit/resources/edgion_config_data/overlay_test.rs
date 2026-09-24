use super::*;

// ---- toy fixture types ----

#[derive(Debug, Clone, PartialEq)]
struct ToyBase {
    x: String,
    y: u32,
    items: Vec<String>,
}

#[derive(Debug, Clone)]
struct ToyOverride {
    /// Replaces `ToyBase.x` when `Some`; keeps base when `None`.
    x: Option<String>,
    /// Replaces `ToyBase.y` when `Some`; keeps base when `None`.
    y: Option<u32>,
    /// Whole-replaces `ToyBase.items` when `Some`; keeps base list when `None`.
    items: Option<Vec<String>>,
}

/// Two variants to exercise the type-guard path.
enum ToyEntry {
    /// Matches the consumer's expected variant; carries a `ToyOverride`.
    Alpha(ToyOverride),
    /// Wrong variant — the type guard must reject this.
    Beta,
}

/// Type-guard extractor: returns `Some` only for the `Alpha` variant.
fn extract_alpha(entry: &ToyEntry) -> Option<&ToyOverride> {
    match entry {
        ToyEntry::Alpha(o) => Some(o),
        ToyEntry::Beta => None,
    }
}

/// Merge function implementing field-level Option replace + whole-collection replace.
fn merge_toy(base: ToyBase, overlay: &ToyOverride) -> ToyBase {
    ToyBase {
        x: overlay.x.clone().unwrap_or(base.x),
        y: overlay.y.unwrap_or(base.y),
        // Whole-replace: the new Vec completely substitutes the base Vec.
        items: overlay.items.clone().unwrap_or(base.items),
    }
}

fn base() -> ToyBase {
    ToyBase {
        x: "base_x".to_string(),
        y: 10,
        items: vec!["a".to_string(), "b".to_string()],
    }
}

// ---- test cases ----

/// Overlay absent → base returned unchanged, signal = Absent.
#[test]
fn absent_overlay_returns_base() {
    let b = base();
    let (effective, signal) = apply_override(b.clone(), None::<&ToyEntry>, extract_alpha, merge_toy);
    assert_eq!(signal, OverlaySignal::Absent);
    assert_eq!(effective, b);
}

/// Overlay present and type matches → selected fields replaced, signal = Applied.
#[test]
fn matching_overlay_replaces_some_fields() {
    let b = base();
    let ov = ToyEntry::Alpha(ToyOverride {
        x: Some("override_x".to_string()),
        y: None,     // keep base
        items: None, // keep base
    });
    let (effective, signal) = apply_override(b.clone(), Some(&ov), extract_alpha, merge_toy);
    assert_eq!(signal, OverlaySignal::Applied);
    assert_eq!(effective.x, "override_x");
    assert_eq!(effective.y, b.y); // unchanged
    assert_eq!(effective.items, b.items); // unchanged
}

/// Overlay present but wrong variant → type guard rejects → base returned, signal = TypeMismatch.
#[test]
fn type_mismatch_returns_base() {
    let b = base();
    let wrong = ToyEntry::Beta;
    let (effective, signal) = apply_override(b.clone(), Some(&wrong), extract_alpha, merge_toy);
    assert_eq!(signal, OverlaySignal::TypeMismatch);
    assert_eq!(effective, b); // base unchanged
}

/// Collection override: new items whole-replace base items (no element merge).
#[test]
fn collection_overlay_is_whole_replace() {
    let b = base();
    let new_items = vec!["x".to_string(), "y".to_string(), "z".to_string()];
    let ov = ToyEntry::Alpha(ToyOverride {
        x: None,
        y: None,
        items: Some(new_items.clone()),
    });
    let (effective, signal) = apply_override(b.clone(), Some(&ov), extract_alpha, merge_toy);
    assert_eq!(signal, OverlaySignal::Applied);
    // The entire items list is replaced — not merged with base items.
    assert_eq!(effective.items, new_items);
    assert_eq!(effective.x, b.x); // other fields unchanged
}

/// All fields overridden at once.
#[test]
fn all_fields_replaced() {
    let b = base();
    let ov = ToyEntry::Alpha(ToyOverride {
        x: Some("new_x".to_string()),
        y: Some(42),
        items: Some(vec!["only".to_string()]),
    });
    let (effective, signal) = apply_override(b, Some(&ov), extract_alpha, merge_toy);
    assert_eq!(signal, OverlaySignal::Applied);
    assert_eq!(effective.x, "new_x");
    assert_eq!(effective.y, 42);
    assert_eq!(effective.items, vec!["only".to_string()]);
}
