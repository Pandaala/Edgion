//! Private reference-only traversal of actual outbound request templates.
use super::owned::{FieldContract, FieldSlot, FieldWire, OwnerSnapshot, PreparedSlot, TopologyLoan};
use super::ReferenceToken;
use crate::resources::common::outbound_request::{OutboundRequestSpec, RequestTemplatePosition};
use crate::resources::common::request_template::{
    parse_secret_ref, CompiledTemplate, TemplateSegment, TemplateVarType,
};
use serde::Serialize;

#[derive(Clone, PartialEq, Eq, Serialize)]
pub(super) struct TemplateWire {
    variable: Option<String>,
    parts: Option<TemplateParts>,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
struct TemplateParts {
    namespace: String,
    name: String,
    key: String,
}
impl std::fmt::Debug for TemplateWire {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TemplateWire").finish_non_exhaustive()
    }
}
impl TemplateWire {
    fn new(variable: Option<&str>) -> Self {
        let parts = variable
            .and_then(|variable| parse_secret_ref(variable).ok())
            .map(|parts| TemplateParts {
                namespace: parts.namespace,
                name: parts.name,
                key: parts.key,
            });
        Self {
            variable: variable.map(str::to_string),
            parts,
        }
    }
    pub(super) fn target(&self) -> Option<(&str, &str)> {
        self.parts
            .as_ref()
            .map(|parts| (parts.namespace.as_str(), parts.name.as_str()))
    }
}

pub(super) enum TemplateAtom {
    Surface {
        position: RequestTemplatePosition,
        compiled: bool,
    },
    Reference {
        position: RequestTemplatePosition,
        segment: usize,
        wire: TemplateWire,
    },
}
pub(super) fn visit(request: &OutboundRequestSpec, mut visit: impl FnMut(TemplateAtom)) {
    request.visit_template_fields(|position, template| {
        let compiled = CompiledTemplate::compile(template);
        visit(TemplateAtom::Surface {
            position,
            compiled: compiled.is_ok(),
        });
        let Ok(compiled) = compiled else { return };
        let mut segment = 0;
        for item in compiled.segments() {
            if let TemplateSegment::Variable {
                var_type: TemplateVarType::SecretRef,
                var_name,
            } = item
            {
                visit(TemplateAtom::Reference {
                    position,
                    segment,
                    wire: TemplateWire::new(var_name.as_deref()),
                });
                segment += 1;
            }
        }
    });
}

#[derive(PartialEq, Eq, Serialize)]
pub(super) enum PreparedTemplateAtom {
    Surface {
        position: RequestTemplatePosition,
        compiled: bool,
    },
    Reference {
        position: RequestTemplatePosition,
        segment: usize,
        slot: Box<PreparedSlot>,
    },
}
impl PreparedTemplateAtom {
    pub(super) fn new(
        owner: &OwnerSnapshot,
        atom: TemplateAtom,
        position: impl FnOnce(RequestTemplatePosition, usize) -> FieldSlot,
    ) -> Self {
        match atom {
            TemplateAtom::Surface { position, compiled } => Self::Surface { position, compiled },
            TemplateAtom::Reference {
                position: field,
                segment,
                wire,
            } => Self::Reference {
                position: field,
                segment,
                slot: Box::new(PreparedSlot::new(
                    owner,
                    FieldWire::Template(wire),
                    FieldContract::TemplateSecret,
                    position(field, segment),
                )),
            },
        }
    }
    pub(super) fn matches(&self, atom: &TemplateAtom) -> bool {
        match (self, atom) {
            (
                Self::Surface { position, compiled },
                TemplateAtom::Surface {
                    position: current,
                    compiled: valid,
                },
            ) => position == current && compiled == valid,
            (
                Self::Reference {
                    position,
                    segment,
                    slot,
                },
                TemplateAtom::Reference {
                    position: current,
                    segment: ordinal,
                    wire,
                },
            ) => {
                position == current
                    && segment == ordinal
                    && matches!(&slot.wire,FieldWire::Template(prepared) if prepared == wire)
            }
            _ => false,
        }
    }
    pub(super) fn slot(&self) -> Option<&PreparedSlot> {
        match self {
            Self::Reference { slot, .. } => Some(slot.as_ref()),
            _ => None,
        }
    }
    pub(super) fn reference<'loan>(
        &'loan self,
        topology: TopologyLoan<'loan>,
    ) -> Option<TemplateSecretReference<'loan>> {
        let slot = self.slot()?;
        let FieldWire::Template(wire) = &slot.wire else {
            return None;
        };
        Some(TemplateSecretReference {
            token: slot.token(topology),
            wire,
        })
    }
}

/// An actual template's fixed Secret token and exact runtime/data keys.
/// Constructors and authored positions remain private to Resources.
/// A mutable plugin loan cannot overlap a live template reference.
/// ```compile_fail
/// use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
/// fn overlap(prepared: &PreparedPluginReferences, owner: &mut edgion_resources::EdgionPlugins) {
///     let mut binding = prepared.bind_mut(owner).unwrap();
///     let mut entry = binding.request_entries().next().unwrap();
///     let references = entry.references().unwrap();
///     let secret = references.request_secrets().next().unwrap();
///     entry.config_mut();
///     secret.reference().validated().unwrap();
/// }
/// ```
/// The same ownership boundary applies to Webhook templates.
/// ```compile_fail
/// use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;
/// fn overlap(prepared: &PreparedLinkSysReferences, owner: &mut edgion_resources::LinkSys) {
///     let mut binding = prepared.bind_mut(owner).unwrap();
///     let references = binding.references().unwrap();
///     let secret = references.request_secrets().next().unwrap();
///     binding.config_mut();
///     secret.reference().validated().unwrap();
/// }
/// ```
pub struct TemplateSecretReference<'a> {
    token: ReferenceToken<'a>,
    wire: &'a TemplateWire,
}
impl TemplateSecretReference<'_> {
    pub fn reference(&self) -> &ReferenceToken<'_> {
        &self.token
    }
    pub fn map_key(&self) -> &str {
        self.wire.variable.as_deref().unwrap_or_default()
    }
    pub fn data_key(&self) -> &str {
        self.wire.parts.as_ref().map_or("", |parts| parts.key.as_str())
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/template_fields_test.rs"]
mod tests;
