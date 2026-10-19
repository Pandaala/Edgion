//! Shared GraphQL input/output type-wrapper parsing.

use super::{name, GraphqlError};
use apollo_parser::cst;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeRef {
    pub name: String,
    pub nulls: Vec<bool>,
}
impl TypeRef {
    /// Inspect the scalar Int requirement without constructing owned type wrappers.
    pub fn scalar_int_requirement(ty: cst::Type) -> Result<bool, GraphqlError> {
        let (named, required) = match ty {
            cst::Type::NamedType(named) => (named, false),
            cst::Type::NonNullType(non_null) => (non_null.named_type().ok_or(GraphqlError::Invalid)?, true),
            cst::Type::ListType(_) => return Err(GraphqlError::Invalid),
        };
        if named.name().ok_or(GraphqlError::Invalid)?.text() == "Int" {
            Ok(required)
        } else {
            Err(GraphqlError::Invalid)
        }
    }

    pub fn parse(ty: cst::Type) -> Result<Self, GraphqlError> {
        fn walk(ty: cst::Type, nulls: &mut Vec<bool>) -> Result<String, GraphqlError> {
            if nulls.len() >= 64 {
                return Err(GraphqlError::Invalid);
            }
            match ty {
                cst::Type::NamedType(n) => {
                    nulls.push(false);
                    name(n.name())
                }
                cst::Type::ListType(l) => {
                    nulls.push(false);
                    walk(l.ty().ok_or(GraphqlError::Invalid)?, nulls)
                }
                cst::Type::NonNullType(n) => {
                    nulls.push(true);
                    if let Some(named) = n.named_type() {
                        name(named.name())
                    } else {
                        walk(n.list_type().and_then(|l| l.ty()).ok_or(GraphqlError::Invalid)?, nulls)
                    }
                }
            }
        }
        let mut nulls = Vec::new();
        let name = walk(ty, &mut nulls)?;
        Ok(Self { name, nulls })
    }
    pub fn scalar_int(&self) -> bool {
        self.name == "Int" && self.nulls.len() == 1
    }
    pub fn required(&self) -> bool {
        self.nulls[0]
    }
}

#[cfg(test)]
#[path = "../../tests/unit/graphql_request/input_type_test.rs"]
mod tests;
