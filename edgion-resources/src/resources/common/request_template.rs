//! Canonical compiled request templates and configuration-time admission.

/// Known template variable types
#[derive(Debug, Clone)]
pub enum TemplateVarType {
    Header,
    Query,
    Cookie,
    Ctx,
    /// Current normalized path (without query string).
    Path,
    /// Current normalized full URI (current path + current query).
    Uri,
    /// Entry-time raw path (frozen, immune to rewrites).
    RequestPath,
    /// Entry-time raw full URI (frozen).
    RequestUri,
    Method,
    RemoteIp,
    /// Matched HTTPRoute identity/metadata `${matchedHttpRoute:name|namespace|label.<k>|annotation.<k>}`.
    MatchedHttpRoute,
    /// Matched GRPCRoute identity/metadata `${matchedGrpcRoute:...}`.
    MatchedGrpcRoute,
    /// Kubernetes Secret reference `${secretRef:ns/name:key}`. Only admitted on
    /// outbound surfaces with an explicit resolved-Secret capability. The resolved
    /// value is carried inline by the controller (never read at gateway runtime
    /// from a global store).
    SecretRef,
}

impl TemplateVarType {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "header" => Ok(Self::Header),
            "query" => Ok(Self::Query),
            "cookie" => Ok(Self::Cookie),
            "ctx" => Ok(Self::Ctx),
            "path" => Ok(Self::Path),
            "uri" => Ok(Self::Uri),
            "requestPath" => Ok(Self::RequestPath),
            "requestUri" => Ok(Self::RequestUri),
            "method" => Ok(Self::Method),
            "remoteIp" => Ok(Self::RemoteIp),
            "matchedHttpRoute" => Ok(Self::MatchedHttpRoute),
            "matchedGrpcRoute" => Ok(Self::MatchedGrpcRoute),
            "secretRef" => Ok(Self::SecretRef),
            _ => Err(format!("unknown variable type: {}", s)),
        }
    }
}

/// Pre-compiled template segment
#[derive(Debug, Clone)]
pub enum TemplateSegment {
    /// Literal text, output as-is
    Literal(String),
    /// Variable reference, resolved at runtime
    Variable {
        var_type: TemplateVarType,
        var_name: Option<String>,
    },
}

/// Pre-compiled template for safe evaluation.
///
/// By compiling the template at config-load time, runtime evaluation only
/// iterates over fixed segments and never re-parses `${…}` markers.
/// This prevents template injection: user-controlled values that contain
/// `${…}` are emitted as literal text instead of being expanded.
#[derive(Debug, Clone)]
pub struct CompiledTemplate {
    segments: Vec<TemplateSegment>,
    /// Estimated result length for pre-allocating the output buffer
    hint_len: usize,
}

const VAR_HINT_BYTES: usize = 32;

impl CompiledTemplate {
    /// Compile a template string into segments.
    /// Returns Err if the template has unclosed `${` or an unknown variable type.
    pub fn compile(template: &str) -> Result<Self, String> {
        let mut segments = Vec::new();
        let mut hint_len: usize = 0;
        let mut remaining = template;

        while let Some(start) = remaining.find("${") {
            // Push literal before the `${`
            if start > 0 {
                let lit = remaining[..start].to_string();
                hint_len = hint_len.saturating_add(lit.len());
                segments.push(TemplateSegment::Literal(lit));
            }

            let after_start = &remaining[start + 2..];
            match after_start.find('}') {
                Some(end) => {
                    let var_expr = &after_start[..end];
                    if var_expr.is_empty() {
                        return Err("empty variable expression: ${}".to_string());
                    }
                    let (var_type_str, var_name) = if let Some(colon_pos) = var_expr.find(':') {
                        (&var_expr[..colon_pos], Some(var_expr[colon_pos + 1..].to_string()))
                    } else {
                        (var_expr, None)
                    };
                    let var_type = TemplateVarType::parse(var_type_str)?;
                    hint_len = hint_len.saturating_add(VAR_HINT_BYTES);
                    segments.push(TemplateSegment::Variable { var_type, var_name });
                    remaining = &after_start[end + 1..];
                }
                None => {
                    return Err(format!("unclosed template variable at: ${{{}", after_start));
                }
            }
        }

        // Push trailing literal
        if !remaining.is_empty() {
            hint_len = hint_len.saturating_add(remaining.len());
            segments.push(TemplateSegment::Literal(remaining.to_string()));
        }

        Ok(CompiledTemplate { segments, hint_len })
    }

    /// Iterate over the compiled segments
    pub fn segments(&self) -> &[TemplateSegment] {
        &self.segments
    }

    /// Estimated output length for pre-allocating buffers
    pub fn hint_len(&self) -> usize {
        self.hint_len
    }
}

/// Parsed components of a `${secretRef:ns/name:key}` variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRefParts {
    pub namespace: String,
    pub name: String,
    pub key: String,
}

/// Parse the `var_name` of a `SecretRef` segment (the raw `"ns/name:key"`
/// string, already stripped of the `secretRef:` prefix by `compile`).
///
/// Requires an explicit namespace and a non-empty key. The map key used to
/// carry the resolved value is the unparsed `var_name` itself; this function is
/// only for the controller, which needs the split components to fetch the Secret.
pub fn parse_secret_ref(var_name: &str) -> Result<SecretRefParts, String> {
    let (path, key) = var_name
        .rsplit_once(':')
        .ok_or_else(|| format!("secretRef '{}' is missing ':key' (expected ns/name:key)", var_name))?;
    if key.is_empty() {
        return Err(format!("secretRef '{}' has an empty key", var_name));
    }
    let (namespace, name) = path.split_once('/').ok_or_else(|| {
        format!(
            "secretRef '{}' is missing 'namespace/' (expected ns/name:key)",
            var_name
        )
    })?;
    if namespace.is_empty() {
        return Err(format!("secretRef '{}' has an empty namespace", var_name));
    }
    if name.is_empty() {
        return Err(format!("secretRef '{}' has an empty name", var_name));
    }
    Ok(SecretRefParts {
        namespace: namespace.to_string(),
        name: name.to_string(),
        key: key.to_string(),
    })
}

/// Collect the raw `var_name`s of every `SecretRef` segment in a compiled
/// template (e.g. `"ns/name:key"`). These are the canonical `resolved_secrets`
/// map keys.
pub fn secret_ref_var_names(tpl: &CompiledTemplate) -> Vec<String> {
    tpl.segments()
        .iter()
        .filter_map(|seg| match seg {
            TemplateSegment::Variable {
                var_type: TemplateVarType::SecretRef,
                var_name: Some(name),
            } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// True if the template contains ANY `secretRef` segment (including a malformed
/// bare `${secretRef}` with no `ns/name:key`).
pub fn template_has_secret_ref(tpl: &CompiledTemplate) -> bool {
    tpl.segments().iter().any(|seg| {
        matches!(
            seg,
            TemplateSegment::Variable {
                var_type: TemplateVarType::SecretRef,
                ..
            }
        )
    })
}

/// Validate every `secretRef` segment in a compiled template: each must be a
/// well-formed `${secretRef:ns/name:key}` (explicit ns + key). A bare
/// `${secretRef}` (no name) is rejected. Returns the first error.
pub fn validate_secret_ref_segments(tpl: &CompiledTemplate) -> Result<(), String> {
    for seg in tpl.segments() {
        if let TemplateSegment::Variable {
            var_type: TemplateVarType::SecretRef,
            var_name,
        } = seg
        {
            match var_name {
                Some(name) => {
                    parse_secret_ref(name)?;
                }
                None => return Err("secretRef requires the form ${secretRef:ns/name:key}".to_string()),
            }
        }
    }
    Ok(())
}

/// Request variables admitted by a template surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariablePolicy {
    Request,
    /// Context, resolved remote IP, and matched-route metadata only.
    /// Their trust still depends on the producer and configured proxy boundary.
    Trusted,
}

/// Secret access is independent of the request-variable profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretPolicy {
    Deny,
    AllowResolved,
}

#[derive(Debug, Clone, Copy)]
pub struct TemplateAdmission {
    pub variables: VariablePolicy,
    pub secrets: SecretPolicy,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TemplateAdmissionError {
    SecretDenied,
    InvalidSecret(String),
    ClientControlledVariable,
}

impl std::fmt::Display for TemplateAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SecretDenied => f.write_str("must not reference ${secretRef:...}"),
            Self::InvalidSecret(error) => f.write_str(error),
            Self::ClientControlledVariable => f.write_str(
                "must not read client-controlled request input; use a trusted ctx, remoteIp, or matched-route variable",
            ),
        }
    }
}

impl TemplateAdmission {
    /// Validate capabilities without broadening consumer-specific shape checks.
    pub fn validate(self, template: &CompiledTemplate) -> Result<(), TemplateAdmissionError> {
        if self.secrets == SecretPolicy::Deny && template_has_secret_ref(template) {
            return Err(TemplateAdmissionError::SecretDenied);
        }
        if self.secrets == SecretPolicy::AllowResolved {
            validate_secret_ref_segments(template).map_err(TemplateAdmissionError::InvalidSecret)?;
        }
        if self.variables == VariablePolicy::Trusted {
            for segment in template.segments() {
                if let TemplateSegment::Variable { var_type, .. } = segment {
                    if !matches!(
                        var_type,
                        TemplateVarType::Ctx
                            | TemplateVarType::RemoteIp
                            | TemplateVarType::MatchedHttpRoute
                            | TemplateVarType::MatchedGrpcRoute
                            | TemplateVarType::SecretRef
                    ) {
                        return Err(TemplateAdmissionError::ClientControlledVariable);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
}
