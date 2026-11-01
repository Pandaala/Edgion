use super::*;
use crate::plugins::runtime::dye_plan::CompiledDyePlan;
use crate::EdgionHttpContext;
use edgion_resources::resources::edgion_plugins::DyeSuite;

fn plan_declaring(name: &str) -> Arc<CompiledDyePlan> {
    let suite: DyeSuite = yaml_serde::from_str(&format!("request:\n  - name: {name}\n")).unwrap();
    CompiledDyePlan::compile(&suite, "canary").unwrap()
}

/// `PingoraSessionAdapter::strip_request_dye_once` needs a real `Session`,
/// which nothing in this codebase constructs in a unit test (see
/// `PingoraSessionAdapter::new`'s only test-adjacent caller,
/// `preflight_handler.rs`, which has the same "TODO: need a proper
/// Session" gap). Drive the exact same ctx + header sequence the adapter
/// method runs through the pure `strip_request_dye_once_core` instead —
/// this is the adapter method's body verbatim, minus the `Session` wrapper.
fn strip_once(ctx: &mut EdgionHttpContext, req: &mut RequestHeader) {
    let Some(plan) = ctx.current_dye_plan() else {
        return;
    };
    let mut handled = ctx.take_dye_handled_request_names();
    strip_request_dye_once_core(req, &plan, &mut handled);
    ctx.restore_dye_handled_request_names(handled);
}

#[test]
fn strip_request_dye_once_is_once_per_name() {
    let mut ctx = EdgionHttpContext::default();
    let mut req = RequestHeader::build("GET", b"/", None).unwrap();
    req.insert_header("X-T", "client-forged").unwrap();

    // Entry A declares X-T: strip removes the client-supplied value and
    // marks the name handled.
    ctx.install_current_dye_plan(Some(plan_declaring("X-T")));
    strip_once(&mut ctx, &mut req);
    assert!(req.headers.get("X-T").is_none());
    assert_eq!(ctx.dye_handled_request_names(), ["X-T"]);

    // Simulate entry A's own plugin rendering its dye value onto the
    // request between the two installs.
    req.insert_header("X-T", "gateway-rendered").unwrap();

    // Entry B also declares X-T: no second strip — the gateway-rendered
    // value from entry A survives, and the handled list is not duplicated.
    ctx.install_current_dye_plan(Some(plan_declaring("X-T")));
    strip_once(&mut ctx, &mut req);
    assert_eq!(req.headers.get("X-T").unwrap().to_str().unwrap(), "gateway-rendered");
    assert_eq!(ctx.dye_handled_request_names(), ["X-T"]);
}

#[test]
fn strip_request_dye_once_matches_names_case_insensitively() {
    let mut ctx = EdgionHttpContext::default();
    let mut req = RequestHeader::build("GET", b"/", None).unwrap();
    req.insert_header("x-t", "client-forged").unwrap();

    ctx.install_current_dye_plan(Some(plan_declaring("X-T")));
    strip_once(&mut ctx, &mut req);
    assert!(req.headers.get("x-t").is_none());

    req.insert_header("X-T", "gateway-rendered").unwrap();
    // A later entry declaring a differently-cased spelling of the same
    // name still matches the already-handled record.
    ctx.install_current_dye_plan(Some(plan_declaring("x-t")));
    strip_once(&mut ctx, &mut req);
    assert_eq!(req.headers.get("X-T").unwrap().to_str().unwrap(), "gateway-rendered");
}

#[test]
fn strip_request_dye_once_noop_without_installed_plan() {
    let mut ctx = EdgionHttpContext::default();
    let mut req = RequestHeader::build("GET", b"/", None).unwrap();
    req.insert_header("X-T", "client-forged").unwrap();

    // No plan installed (ctx.current_dye_plan is None): nothing stripped.
    strip_once(&mut ctx, &mut req);
    assert_eq!(req.headers.get("X-T").unwrap().to_str().unwrap(), "client-forged");
    assert!(ctx.dye_handled_request_names().is_empty());
}

#[test]
fn strip_request_dye_once_ignores_response_direction_rules() {
    let mut ctx = EdgionHttpContext::default();
    let mut req = RequestHeader::build("GET", b"/", None).unwrap();
    req.insert_header("X-O", "client-forged").unwrap();

    let suite: DyeSuite = yaml_serde::from_str("response:\n  - name: X-O\n").unwrap();
    ctx.install_current_dye_plan(Some(CompiledDyePlan::compile(&suite, "canary").unwrap()));
    strip_once(&mut ctx, &mut req);
    // Response-direction rules never drive the request-side strip.
    assert_eq!(req.headers.get("X-O").unwrap().to_str().unwrap(), "client-forged");
    assert!(ctx.dye_handled_request_names().is_empty());
}
