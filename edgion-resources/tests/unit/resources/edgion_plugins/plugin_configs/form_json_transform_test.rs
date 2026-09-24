use super::*;

#[test]
fn direction_is_required_and_uses_camel_case() {
    let form_to_json: FormJsonTransformConfig = serde_json::from_str(r#"{"direction":"formToJson"}"#).unwrap();
    assert_eq!(form_to_json.direction, FormJsonDirection::FormToJson);

    let json_to_form: FormJsonTransformConfig = serde_json::from_str(r#"{"direction":"jsonToForm"}"#).unwrap();
    assert_eq!(json_to_form.direction, FormJsonDirection::JsonToForm);

    assert!(serde_json::from_str::<FormJsonTransformConfig>("{}").is_err());
    assert!(serde_json::from_str::<FormJsonTransformConfig>(r#"{"direction":"auto"}"#).is_err());
}
