//! Shared GraphQL input/output type-wrapper parsing.

use super::{name, GraphqlError};
use apollo_parser::cst;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeRef {
    pub name: String,
    pub nulls: Vec<bool>,
}
impl TypeRef {
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
