//! XML runtime policy, owned request scanner and transactional publication.

use std::sync::Arc;

use bytes::Bytes;
use edgion_dsl::xml::{self, XmlError, XmlValue, MAX_XML_BYTES, MAX_XML_NODES, MAX_XML_TAG_ATTRIBUTES};

use crate::plugins::runtime::PluginSession;
use crate::request_body::{BodyOperationError, RawBodyScanner, ScanControl};

use super::super::runtime::DslRuntime;
use super::body_ops::{body_access_error, map_body_read_error, DslBodyOps};
use super::builtins::checked_value_str;
use super::bytecode::BuiltinId;
use super::error::RuntimeError;
use super::value::{Value, ValueWorkBudget};
use super::vm::{Vm, VmState};

fn xml_error(error: XmlError) -> RuntimeError {
    match error {
        XmlError::Work(error) => error,
        XmlError::Limit { kind, limit } => RuntimeError::XmlLimitExceeded { kind, limit },
        XmlError::Invalid => RuntimeError::ApiError {
            function: "xml".into(),
            message: "invalid XML operation".into(),
        },
        XmlError::Allocation => RuntimeError::ApiError {
            function: "xml".into(),
            message: "bounded XML allocation failed".into(),
        },
    }
}

fn parsed_value(source: &str, budget: &mut ValueWorkBudget) -> Result<Value, RuntimeError> {
    match xml::parse(source, budget) {
        Ok(value) => Ok(Value::Xml(value)),
        Err(XmlError::Invalid) => Ok(Value::Nil),
        Err(error) => Err(xml_error(error)),
    }
}

pub(crate) struct XmlScanOutcome {
    pub(crate) work: ValueWorkBudget,
    pub(crate) result: Result<Value, RuntimeError>,
}

pub(crate) struct XmlBodyScanner {
    source: Vec<u8>,
    work: ValueWorkBudget,
    error: Option<RuntimeError>,
}

impl XmlBodyScanner {
    pub(crate) fn new(work: ValueWorkBudget) -> Self {
        Self {
            source: Vec::new(),
            work,
            error: None,
        }
    }
}

impl RawBodyScanner for XmlBodyScanner {
    type Output = XmlScanOutcome;
    fn requires_blocking_context(&self) -> bool {
        true
    }

    fn push(&mut self, chunk: &[u8]) -> Result<ScanControl, BodyOperationError> {
        let next = self.source.len().saturating_add(chunk.len());
        if next > MAX_XML_BYTES {
            self.error = Some(body_access_error("XML source exceeds the request XML byte limit"));
            return Ok(ScanControl::Stop);
        }
        if let Err(error) = self.work.charge((chunk.len() as u64).saturating_mul(2)) {
            self.error = Some(body_access_error(error.to_string()));
            return Ok(ScanControl::Stop);
        }
        if next > self.source.capacity() {
            let target = next.max(self.source.capacity().saturating_mul(2)).min(MAX_XML_BYTES);
            if self.source.try_reserve_exact(target - self.source.len()).is_err() {
                self.error = Some(body_access_error("bounded XML source allocation failed"));
                return Ok(ScanControl::Stop);
            }
        }
        self.source.extend_from_slice(chunk);
        Ok(ScanControl::Continue)
    }

    fn finish(mut self) -> Result<Self::Output, BodyOperationError> {
        let result = if let Some(error) = self.error {
            Err(error)
        } else if let Ok(source) = std::str::from_utf8(&self.source) {
            parsed_value(source, &mut self.work).map_err(|error| body_access_error(error.to_string()))
        } else {
            Ok(Value::Nil)
        };
        Ok(XmlScanOutcome {
            work: self.work,
            result,
        })
    }
}

fn expect_str(value: Value, operation: &'static str) -> Result<Arc<str>, RuntimeError> {
    match value {
        Value::Str(text) => Ok(text),
        other => Err(RuntimeError::TypeError {
            expected: "Str",
            got: other.type_name(),
            operation,
        }),
    }
}

fn expect_xml(value: Value, operation: &'static str) -> Result<Option<XmlValue>, RuntimeError> {
    match value {
        Value::Xml(node) => Ok(Some(node)),
        Value::Nil => Ok(None),
        other => Err(RuntimeError::TypeError {
            expected: "Xml or Nil",
            got: other.type_name(),
            operation,
        }),
    }
}

fn index(value: Value) -> Result<Option<usize>, RuntimeError> {
    match value {
        Value::Int(number) => Ok(usize::try_from(number).ok()),
        other => Err(RuntimeError::TypeError {
            expected: "Int",
            got: other.type_name(),
            operation: "xml index",
        }),
    }
}

fn nodes_from_map(
    value: Value,
    kind: &'static str,
    cap: usize,
    budget: &mut ValueWorkBudget,
) -> Result<Vec<XmlValue>, RuntimeError> {
    let Value::Map(map) = value else {
        return Err(RuntimeError::TypeError {
            expected: "Map",
            got: value.type_name(),
            operation: "xml.element",
        });
    };
    if map.len() > cap {
        return Err(RuntimeError::XmlLimitExceeded {
            kind: "construction entries",
            limit: cap,
        });
    }
    budget.charge((map.len() as u64).saturating_mul(32))?;
    let mut nodes = Vec::with_capacity(map.len());
    for (_, value) in *map {
        let Value::Xml(node) = value else {
            return Err(RuntimeError::TypeError {
                expected: "Xml",
                got: value.type_name(),
                operation: "xml.element",
            });
        };
        if (kind == "attribute") != (node.kind() == "attribute") {
            return Err(xml_error(XmlError::Invalid));
        }
        nodes.push(node);
    }
    Ok(nodes)
}

fn xml_str(value: &str, limit: usize, budget: &mut ValueWorkBudget) -> Result<Value, RuntimeError> {
    if value.len() > limit {
        return Err(RuntimeError::StringTooLong {
            len: value.len(),
            limit,
        });
    }
    budget.charge((value.len() as u64).saturating_mul(4))?;
    Ok(Value::Str(Arc::from(value)))
}

fn string_error(error: XmlError) -> RuntimeError {
    match error {
        XmlError::Limit {
            kind: "output bytes",
            limit,
        } => RuntimeError::StringTooLong {
            len: limit.saturating_add(1),
            limit,
        },
        other => xml_error(other),
    }
}

impl Vm {
    pub(crate) async fn call_builtin_xml(
        &self,
        id: BuiltinId,
        state: &mut VmState,
        session: &mut dyn PluginSession,
        runtime: &DslRuntime,
    ) -> Result<Value, RuntimeError> {
        let source = if id == BuiltinId::XmlParse {
            Some(state.pop()?)
        } else {
            None
        };
        let body_operation = id == BuiltinId::XmlSetBody || matches!(&source, Some(Value::Body(_)));
        state.value_work_budget.charge(16).map_err(|error| {
            if body_operation {
                body_access_error(error.to_string())
            } else {
                error
            }
        })?;
        match id {
            BuiltinId::XmlParse => match source.ok_or_else(|| RuntimeError::Internal {
                message: "missing XML parse source".into(),
            })? {
                Value::Str(text) => parsed_value(&text, &mut state.value_work_budget),
                Value::Body(_) => {
                    let work = std::mem::replace(&mut state.value_work_budget, ValueWorkBudget::exhausted());
                    let mut ops = DslBodyOps::new(session, &mut state.body_scan_budget, &self.limits, runtime);
                    let outcome = ops.parse_xml(work).await?;
                    state.value_work_budget = outcome.work;
                    outcome.result
                }
                other => Err(RuntimeError::TypeError {
                    expected: "Str or Body",
                    got: other.type_name(),
                    operation: "xml.parse",
                }),
            },
            BuiltinId::XmlText => {
                let text = expect_str(state.pop()?, "xml.text")?;
                XmlValue::text(&text, &mut state.value_work_budget)
                    .map(Value::Xml)
                    .map_err(xml_error)
            }
            BuiltinId::XmlAttr => {
                let value = expect_str(state.pop()?, "xml.attr")?;
                let name = expect_str(state.pop()?, "xml.attr")?;
                let namespace = expect_str(state.pop()?, "xml.attr")?;
                XmlValue::attribute(&namespace, &name, &value, &mut state.value_work_budget)
                    .map(Value::Xml)
                    .map_err(xml_error)
            }
            BuiltinId::XmlElement => {
                let children = nodes_from_map(state.pop()?, "children", MAX_XML_NODES, &mut state.value_work_budget)?;
                let attributes = nodes_from_map(
                    state.pop()?,
                    "attribute",
                    MAX_XML_TAG_ATTRIBUTES,
                    &mut state.value_work_budget,
                )?;
                let name = expect_str(state.pop()?, "xml.element")?;
                let namespace = expect_str(state.pop()?, "xml.element")?;
                XmlValue::element(&namespace, &name, attributes, children, &mut state.value_work_budget)
                    .map(Value::Xml)
                    .map_err(xml_error)
            }
            BuiltinId::XmlSetBody => {
                let node = expect_xml(state.pop()?, "xml.set_body")?.ok_or_else(|| xml_error(XmlError::Invalid))?;
                let limit = session
                    .ctx()
                    .request_body_config
                    .max_body_size_bytes
                    .min(MAX_XML_BYTES as u64) as usize;
                let output = node
                    .stringify(limit, &mut state.value_work_budget)
                    .map_err(|error| body_access_error(xml_error(error).to_string()))?;
                drop(node);
                // Bytes consumes String allocation; no second payload copy or partial publish.
                session
                    .overwrite_request_body(Bytes::from(output))
                    .await
                    .map_err(map_body_read_error)?;
                Ok(Value::Nil)
            }
            BuiltinId::XmlStringify | BuiltinId::XmlValue => {
                let Some(node) = expect_xml(state.pop()?, "xml string output")? else {
                    return Ok(Value::Nil);
                };
                let output = if id == BuiltinId::XmlValue {
                    node.value(self.limits.max_string_len, &mut state.value_work_budget)
                } else {
                    node.stringify(self.limits.max_string_len, &mut state.value_work_budget)
                }
                .map_err(string_error)?;
                if id == BuiltinId::XmlStringify {
                    state.value_work_budget.charge(output.len() as u64)?;
                }
                checked_value_str(output, self.limits.max_string_len)
            }
            BuiltinId::XmlFind | BuiltinId::XmlAttrValue => {
                let occurrence = if id == BuiltinId::XmlFind {
                    index(state.pop()?)?
                } else {
                    Some(0)
                };
                let name = expect_str(state.pop()?, "xml lookup")?;
                let namespace = expect_str(state.pop()?, "xml lookup")?;
                let Some(node) = expect_xml(state.pop()?, "xml lookup")? else {
                    return Ok(Value::Nil);
                };
                if id == BuiltinId::XmlFind {
                    let Some(occurrence) = occurrence else {
                        return Ok(Value::Nil);
                    };
                    Ok(node
                        .find(&namespace, &name, occurrence, &mut state.value_work_budget)
                        .map_err(xml_error)?
                        .map(Value::Xml)
                        .unwrap_or(Value::Nil))
                } else {
                    match node
                        .attr_value(&namespace, &name, &mut state.value_work_budget)
                        .map_err(xml_error)?
                    {
                        Some(value) => xml_str(value, self.limits.max_string_len, &mut state.value_work_budget),
                        None => Ok(Value::Nil),
                    }
                }
            }
            BuiltinId::XmlChild | BuiltinId::XmlAttribute => {
                let index = index(state.pop()?)?;
                let Some(node) = expect_xml(state.pop()?, "xml index")? else {
                    return Ok(Value::Nil);
                };
                let items = if id == BuiltinId::XmlChild {
                    node.children()
                } else {
                    node.attributes()
                }
                .ok_or_else(|| xml_error(XmlError::Invalid))?;
                Ok(index
                    .and_then(|i| items.get(i))
                    .cloned()
                    .map(Value::Xml)
                    .unwrap_or(Value::Nil))
            }
            BuiltinId::XmlKind
            | BuiltinId::XmlName
            | BuiltinId::XmlNamespace
            | BuiltinId::XmlChildCount
            | BuiltinId::XmlAttributeCount => {
                let Some(node) = expect_xml(state.pop()?, "xml getter")? else {
                    return Ok(Value::Nil);
                };
                match id {
                    BuiltinId::XmlKind => {
                        xml_str(node.kind(), self.limits.max_string_len, &mut state.value_work_budget)
                    }
                    BuiltinId::XmlName | BuiltinId::XmlNamespace => match node.name() {
                        Some((namespace, name)) => xml_str(
                            if id == BuiltinId::XmlName { name } else { namespace },
                            self.limits.max_string_len,
                            &mut state.value_work_budget,
                        ),
                        None => Ok(Value::Nil),
                    },
                    _ => {
                        let items = if id == BuiltinId::XmlChildCount {
                            node.children()
                        } else {
                            node.attributes()
                        }
                        .ok_or_else(|| xml_error(XmlError::Invalid))?;
                        Ok(Value::Int(items.len() as i64))
                    }
                }
            }
            _ => Err(RuntimeError::Internal {
                message: "non-XML builtin reached XML dispatch".into(),
            }),
        }
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/dsl/lang/xml_builtins_test.rs"]
mod tests;
