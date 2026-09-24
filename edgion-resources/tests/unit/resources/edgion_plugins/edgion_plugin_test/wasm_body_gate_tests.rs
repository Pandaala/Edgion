use super::*;
use crate::resources::edgion_plugins::plugin_configs::WasmConfig;

/// Wasm has no intrinsic `body_requirement()` (the guest's hooks are only
/// known after module load), but it MUST still accept an operator `body:`
/// block at the request stage so the gateway's body gate can wrap it —
/// otherwise a `body:` entry on a Wasm request plugin is rejected at
/// admission and the request-buffer-mode feature is unreachable.
/// `body_requirement()` itself stays `None` for Wasm (it is config-only, no
/// operator input); the opt-in is expressed solely through `accepts_body()`.
/// Wasm has no response buffer mode, so it must NOT accept a body: block at
/// the response stage.
#[test]
fn wasm_accepts_operator_body_requirement() {
    let p = EdgionPlugin::Wasm(WasmConfig::default());
    assert!(
        p.body_requirement(BodyStage::Request).is_none(),
        "Wasm has no intrinsic body requirement (config-only signal)"
    );
    assert!(
        p.accepts_body(BodyStage::Request),
        "Wasm must accept an operator body: block at the request stage despite having no intrinsic requirement"
    );
    assert!(
        !p.accepts_body(BodyStage::Response),
        "Wasm has no response buffer mode, so it must not accept a body: block at the response stage"
    );
}
