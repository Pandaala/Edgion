use super::*;

fn variable_type(source: &str) -> cst::Type {
    let document = crate::graphql_request::parse_document(&format!("query($v:{source}){{x}}")).unwrap();
    let operation = document
        .definitions()
        .find_map(|definition| match definition {
            cst::Definition::OperationDefinition(operation) => Some(operation),
            _ => None,
        })
        .unwrap();
    operation
        .variable_definitions()
        .unwrap()
        .variable_definitions()
        .next()
        .unwrap()
        .ty()
        .unwrap()
}

#[test]
fn scalar_int_projection_matches_owned_type_parser() {
    for name in ["Int", "int", "INT", "Float", "String", "Boolean", "Custom"] {
        for non_null_items in [false, true] {
            let mut nested = name.to_owned();
            for _ in 0..=8 {
                for source in [nested.clone(), format!("{nested}!")] {
                    let ty = variable_type(&source);
                    let expected = TypeRef::parse(ty.clone()).and_then(|parsed| {
                        if parsed.scalar_int() {
                            Ok(parsed.required())
                        } else {
                            Err(GraphqlError::Invalid)
                        }
                    });
                    assert_eq!(TypeRef::scalar_int_requirement(ty), expected, "{source}");
                }
                nested = if non_null_items {
                    format!("[{nested}!]")
                } else {
                    format!("[{nested}]")
                };
            }
        }
    }
    assert_eq!(TypeRef::scalar_int_requirement(variable_type("Int")), Ok(false));
    assert_eq!(TypeRef::scalar_int_requirement(variable_type("Int!")), Ok(true));
}
