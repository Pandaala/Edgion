use super::*;

#[test]
fn variable_and_secret_policies_are_independent() {
    for (expression, trusted) in [
        ("${header:X-Tenant}", false),
        ("${query:tenant}", false),
        ("${cookie:tenant}", false),
        ("${path}", false),
        ("${uri}", false),
        ("${requestPath}", false),
        ("${requestUri}", false),
        ("${method}", false),
        ("${ctx:tenant}", true),
        ("${remoteIp}", true),
        ("${matchedHttpRoute:name}", true),
        ("${matchedGrpcRoute:namespace}", true),
    ] {
        let template = CompiledTemplate::compile(expression).unwrap();
        for secrets in [SecretPolicy::Deny, SecretPolicy::AllowResolved] {
            assert!(
                TemplateAdmission {
                    variables: VariablePolicy::Request,
                    secrets
                }
                .validate(&template)
                .is_ok(),
                "{expression}"
            );
            assert_eq!(
                TemplateAdmission {
                    variables: VariablePolicy::Trusted,
                    secrets
                }
                .validate(&template)
                .is_ok(),
                trusted,
                "{expression}"
            );
        }
    }
    for variables in [VariablePolicy::Request, VariablePolicy::Trusted] {
        let template = CompiledTemplate::compile("Bearer ${secretRef:ns/token:key}").unwrap();
        assert!(TemplateAdmission {
            variables,
            secrets: SecretPolicy::AllowResolved
        }
        .validate(&template)
        .is_ok());
        assert_eq!(
            TemplateAdmission {
                variables,
                secrets: SecretPolicy::Deny
            }
            .validate(&template),
            Err(TemplateAdmissionError::SecretDenied)
        );
        for expression in [
            "${secretRef}",
            "${secretRef:}",
            "${secretRef:token:key}",
            "${secretRef:ns/token}",
            "${secretRef:ns/token:}",
        ] {
            let template = CompiledTemplate::compile(expression).unwrap();
            assert!(
                matches!(
                    TemplateAdmission {
                        variables,
                        secrets: SecretPolicy::AllowResolved
                    }
                    .validate(&template),
                    Err(TemplateAdmissionError::InvalidSecret(_))
                ),
                "{expression}"
            );
            assert_eq!(
                TemplateAdmission {
                    variables,
                    secrets: SecretPolicy::Deny
                }
                .validate(&template),
                Err(TemplateAdmissionError::SecretDenied)
            );
        }
    }
}

#[test]
fn admission_leaves_variable_shapes_to_the_consumer() {
    for expression in [
        "${header}",
        "${query:}",
        "${ctx}",
        "${path:ignored}",
        "${method:ignored}",
    ] {
        let template = CompiledTemplate::compile(expression).unwrap();
        assert!(TemplateAdmission {
            variables: VariablePolicy::Request,
            secrets: SecretPolicy::Deny
        }
        .validate(&template)
        .is_ok());
    }
}
