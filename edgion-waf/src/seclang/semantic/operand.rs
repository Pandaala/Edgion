//! Operator operands: strict compile-time integers, asset words, IP networks and dynamic projections.

use super::*;

pub(super) fn strict_i64<I>(bytes: I) -> Result<i64, (IntegerOperandErrorKind, usize)>
where
    I: IntoIterator<Item = u8>,
{
    let mut bytes = bytes.into_iter().enumerate().peekable();
    let mut negative = false;
    if let Some((_, sign @ (b'+' | b'-'))) = bytes.peek().copied() {
        negative = sign == b'-';
        bytes.next();
    }
    if bytes.peek().is_none() {
        return Err((IntegerOperandErrorKind::Invalid, 0));
    }
    let limit = if negative {
        (i64::MAX as u64) + 1
    } else {
        i64::MAX as u64
    };
    let mut magnitude = 0u64;
    for (offset, byte) in bytes {
        if !byte.is_ascii_digit() {
            return Err((IntegerOperandErrorKind::Invalid, offset));
        }
        magnitude = magnitude
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(byte - b'0')))
            .filter(|value| *value <= limit)
            .ok_or((IntegerOperandErrorKind::Overflow, offset))?;
    }
    if negative && magnitude == limit {
        Ok(i64::MIN)
    } else if negative {
        Ok(-(magnitude as i64))
    } else {
        Ok(magnitude as i64)
    }
}

fn operand_position(operator: OperatorView<'_, '_>, line: &LogicalLine, offset: usize) -> SourcePosition {
    if offset < operator.argument().decoded_len() {
        operator_mapped_text(operator, line, offset..offset + 1)
            .map_or(operator.argument_span().start, |text| text.span().start)
    } else {
        operator.argument_span().end
    }
}

fn split_ascii_words<I>(bytes: I, mut item: impl FnMut(Range<usize>)) -> usize
where
    I: IntoIterator<Item = u8>,
{
    let mut start = None;
    let mut offset = 0usize;
    let mut count = 0usize;
    for byte in bytes {
        if matches!(byte, b' ' | b'\t') {
            if let Some(item_start) = start.take() {
                item(item_start..offset);
                count += 1;
            }
        } else if start.is_none() {
            start = Some(offset);
        }
        offset += 1;
    }
    if let Some(item_start) = start {
        item(item_start..offset);
        count += 1;
    }
    count
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AssetWalkError {
    ByteLimit { offset: usize },
    Unsafe { kind: AssetErrorKind, offset: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AssetWalkShape {
    pub(super) count: usize,
    pub(super) payload_bytes: usize,
    pub(super) inspected_bytes: usize,
}

pub(super) fn walk_asset_words<I>(
    bytes: I,
    max_asset_bytes: usize,
    mut item: impl FnMut(Range<usize>),
) -> Result<AssetWalkShape, AssetWalkError>
where
    I: IntoIterator<Item = u8>,
{
    let mut start = None;
    let mut count = 0usize;
    let mut payload_bytes = 0usize;
    let mut inspected_bytes = 0usize;
    let mut byte_limit_offset = None;
    let mut component_start = 0usize;
    let mut component_len = 0usize;
    let mut component_dots = true;
    let mut scheme = true;

    let finish = |end: usize,
                  start: &mut Option<usize>,
                  count: &mut usize,
                  payload_bytes: &mut usize,
                  component_start: usize,
                  component_len: usize,
                  component_dots: bool,
                  item: &mut dyn FnMut(Range<usize>)|
     -> Result<(), AssetWalkError> {
        let Some(word_start) = start.take() else {
            return Ok(());
        };
        if component_len == 0 {
            return Err(AssetWalkError::Unsafe {
                kind: AssetErrorKind::EmptyComponent,
                offset: word_start + component_start,
            });
        }
        if component_dots && component_len <= 2 {
            return Err(AssetWalkError::Unsafe {
                kind: AssetErrorKind::DotComponent,
                offset: word_start + component_start,
            });
        }
        item(word_start..end);
        *count += 1;
        *payload_bytes += end - word_start;
        Ok(())
    };

    let mut offset = 0usize;
    for byte in bytes {
        inspected_bytes += 1;
        if matches!(byte, b' ' | b'\t') {
            finish(
                offset,
                &mut start,
                &mut count,
                &mut payload_bytes,
                component_start,
                component_len,
                component_dots,
                &mut item,
            )?;
            offset += 1;
            continue;
        }
        let word_start = *start.get_or_insert_with(|| {
            component_start = 0;
            component_len = 0;
            component_dots = true;
            scheme = true;
            offset
        });
        let local = offset - word_start;
        if local >= max_asset_bytes && byte_limit_offset.is_none() {
            byte_limit_offset = Some(offset);
        }
        let error = if local == 0 && byte == b'/' {
            Some(AssetErrorKind::Absolute)
        } else if byte == b'\\' {
            Some(AssetErrorKind::Backslash)
        } else if byte.is_ascii_control() || byte == 0x7f {
            Some(AssetErrorKind::Control)
        } else if byte == b':' && local > 0 && scheme {
            Some(AssetErrorKind::Scheme)
        } else {
            None
        };
        if let Some(kind) = error {
            return Err(AssetWalkError::Unsafe { kind, offset });
        }
        scheme &= if local == 0 {
            byte.is_ascii_alphabetic()
        } else {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-')
        };
        if byte == b'/' {
            if component_len == 0 {
                return Err(AssetWalkError::Unsafe {
                    kind: AssetErrorKind::EmptyComponent,
                    offset: word_start + component_start,
                });
            }
            if component_dots && component_len <= 2 {
                return Err(AssetWalkError::Unsafe {
                    kind: AssetErrorKind::DotComponent,
                    offset: word_start + component_start,
                });
            }
            component_start = local + 1;
            component_len = 0;
            component_dots = true;
        } else {
            component_len += 1;
            component_dots &= byte == b'.';
        }
        offset += 1;
    }
    finish(
        offset,
        &mut start,
        &mut count,
        &mut payload_bytes,
        component_start,
        component_len,
        component_dots,
        &mut item,
    )?;
    if let Some(offset) = byte_limit_offset {
        return Err(AssetWalkError::ByteLimit { offset });
    }
    Ok(AssetWalkShape {
        count,
        payload_bytes,
        inspected_bytes,
    })
}

pub(super) fn project_dynamic(
    semantic_charge: usize,
    inherited: usize,
    dynamic: usize,
    limits: SemanticLimits,
    position: SourcePosition,
) -> Result<usize, SemanticError> {
    let charge = semantic_charge
        .checked_add(dynamic)
        .ok_or(SemanticError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
            position,
        })?;
    if charge > limits.max_owned_bytes {
        return Err(SemanticError::OwnedByteLimit {
            limit: limits.max_owned_bytes,
            position,
        });
    }
    if inherited
        .checked_add(charge)
        .is_none_or(|retained| retained > limits.max_retained_bytes)
    {
        return Err(SemanticError::RetainedByteLimit {
            limit: limits.max_retained_bytes,
        });
    }
    Ok(charge)
}

pub(super) struct OperatorOperandBuild<'a, 'source> {
    pub(super) templates: TemplateBuild<'a, 'source>,
    pub(super) operands: &'a mut Vec<OperatorOperandRecord>,
    pub(super) static_operands: &'a mut Vec<StaticOperandRecord<'source>>,
    pub(super) list_items: &'a mut Vec<ListItemRecord>,
    pub(super) networks: &'a mut Vec<NetworkRecord>,
    pub(super) phrase_count: &'a mut usize,
    pub(super) asset_count: &'a mut usize,
    pub(super) phrase_bytes: &'a mut usize,
    pub(super) unavailable: &'a mut Vec<CapabilityRecord>,
}

impl<'a, 'source> OperatorOperandBuild<'a, 'source> {
    fn prepare_metadata_only(
        &mut self,
        requirement_records: usize,
        capability_records: usize,
        position: SourcePosition,
    ) -> Result<(), SemanticError> {
        let dynamic = size_of::<RequirementRecord>()
            .checked_mul(requirement_records)
            .and_then(|value| value.checked_add(size_of::<CapabilityRecord>().checked_mul(capability_records)?))
            .ok_or(SemanticError::OwnedByteLimit {
                limit: self.templates.limits.max_owned_bytes,
                position,
            })?;
        let charge = project_dynamic(
            *self.templates.semantic_charge,
            self.templates.inherited,
            dynamic,
            self.templates.limits,
            position,
        )?;
        self.templates
            .requirements
            .try_reserve(requirement_records)
            .map_err(|_| SemanticError::Allocation {
                operation: "operator requirements",
            })?;
        self.unavailable
            .try_reserve(capability_records)
            .map_err(|_| SemanticError::Allocation {
                operation: "operator capabilities",
            })?;
        *self.templates.semantic_charge = charge;
        Ok(())
    }

    fn store_static(
        &mut self,
        operator: OperatorView<'_, 'source>,
        extra_records: usize,
        network_records: usize,
        requirement_records: usize,
        capability_records: usize,
        position: SourcePosition,
    ) -> Result<usize, SemanticError> {
        let decoded_len = operator.argument().decoded_len();
        let copied = usize::from(operator.argument_has_removable_escapes()) * decoded_len;
        if self
            .templates
            .decoded
            .len()
            .checked_add(copied)
            .is_none_or(|count| count > self.templates.limits.operands.max_decoded_bytes)
        {
            return Err(SemanticError::DecodedOperandByteLimit {
                limit: self.templates.limits.operands.max_decoded_bytes,
                position,
            });
        }
        let dynamic = size_of::<StaticOperandRecord<'_>>()
            .checked_add(size_of::<ListItemRecord>().checked_mul(extra_records).ok_or(
                SemanticError::OwnedByteLimit {
                    limit: self.templates.limits.max_owned_bytes,
                    position,
                },
            )?)
            .and_then(|value| value.checked_add(size_of::<NetworkRecord>().checked_mul(network_records)?))
            .and_then(|value| value.checked_add(copied))
            .and_then(|value| value.checked_add(size_of::<RequirementRecord>().checked_mul(requirement_records)?))
            .and_then(|value| value.checked_add(size_of::<CapabilityRecord>().checked_mul(capability_records)?))
            .ok_or(SemanticError::OwnedByteLimit {
                limit: self.templates.limits.max_owned_bytes,
                position,
            })?;
        let charge = project_dynamic(
            *self.templates.semantic_charge,
            self.templates.inherited,
            dynamic,
            self.templates.limits,
            position,
        )?;
        self.static_operands
            .try_reserve(1)
            .map_err(|_| SemanticError::Allocation {
                operation: "static operands",
            })?;
        self.list_items
            .try_reserve(extra_records)
            .map_err(|_| SemanticError::Allocation {
                operation: "operator list items",
            })?;
        self.networks
            .try_reserve(network_records)
            .map_err(|_| SemanticError::Allocation {
                operation: "IP networks",
            })?;
        self.templates
            .decoded
            .try_reserve(copied)
            .map_err(|_| SemanticError::Allocation {
                operation: "decoded operands",
            })?;
        self.templates
            .requirements
            .try_reserve(requirement_records)
            .map_err(|_| SemanticError::Allocation {
                operation: "operator requirements",
            })?;
        self.unavailable
            .try_reserve(capability_records)
            .map_err(|_| SemanticError::Allocation {
                operation: "operator capabilities",
            })?;
        let storage = if copied == 0 {
            let logical = operator.argument_logical_range();
            OperandStorage::Borrowed(SemanticText {
                text: operator.argument().raw(),
                logical_start: logical.start,
                logical_end: logical.end,
                span: operator.argument_span(),
            })
        } else {
            let start = self.templates.decoded.len();
            self.templates.decoded.extend(operator.argument().bytes());
            OperandStorage::Decoded {
                start,
                len: decoded_len,
                span: operator.argument_span(),
            }
        };
        let id = self.static_operands.len();
        self.static_operands.push(StaticOperandRecord { storage });
        *self.templates.semantic_charge = charge;
        Ok(id)
    }
}

fn mask_ip(address: IpAddr, prefix: u8) -> IpAddr {
    match address {
        IpAddr::V4(value) => {
            let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
            IpAddr::V4(Ipv4Addr::from(u32::from(value) & mask))
        }
        IpAddr::V6(value) => {
            let mask = if prefix == 0 { 0 } else { u128::MAX << (128 - prefix) };
            IpAddr::V6(Ipv6Addr::from(u128::from(value) & mask))
        }
    }
}

fn trim_ascii(bytes: &[u8], mut range: Range<usize>) -> Range<usize> {
    while range.start < range.end && matches!(bytes[range.start], b' ' | b'\t') {
        range.start += 1;
    }
    while range.start < range.end && matches!(bytes[range.end - 1], b' ' | b'\t') {
        range.end -= 1;
    }
    range
}

fn walk_comma_ranges(bytes: &[u8], mut item: impl FnMut(Range<usize>)) -> Result<usize, usize> {
    let mut count = 0usize;
    let mut start = 0usize;
    for end in bytes
        .iter()
        .enumerate()
        .filter_map(|(index, byte)| (*byte == b',').then_some(index))
        .chain(std::iter::once(bytes.len()))
    {
        let range = trim_ascii(bytes, start..end);
        if range.is_empty() {
            return Err(start);
        }
        item(range);
        count += 1;
        start = end.saturating_add(1);
    }
    Ok(count)
}

fn decimal_u8(bytes: &[u8], range: Range<usize>) -> Result<u8, usize> {
    if range.is_empty() {
        return Err(range.start);
    }
    let mut value = 0u16;
    for (index, byte) in bytes[range.clone()].iter().copied().enumerate() {
        if !byte.is_ascii_digit() {
            return Err(range.start + index);
        }
        value = value
            .checked_mul(10)
            .and_then(|current| current.checked_add(u16::from(byte - b'0')))
            .filter(|current| *current <= 255)
            .ok_or(range.start + index)?;
    }
    Ok(value as u8)
}

fn parse_byte_member(bytes: &[u8], range: Range<usize>) -> Result<(u8, u8), usize> {
    let dash = bytes[range.clone()].iter().position(|byte| *byte == b'-');
    let (start, end) = if let Some(relative) = dash {
        let split = range.start + relative;
        if bytes[split + 1..range.end].contains(&b'-') {
            return Err(split + 1);
        }
        let start_range = trim_ascii(bytes, range.start..split);
        let end_range = trim_ascii(bytes, split + 1..range.end);
        (decimal_u8(bytes, start_range)?, decimal_u8(bytes, end_range)?)
    } else {
        let value = decimal_u8(bytes, range.clone())?;
        (value, value)
    };
    if start > end {
        return Err(range.start);
    }
    Ok((start, end))
}

pub(super) fn classify_operator_operand<'source>(
    build: &mut OperatorOperandBuild<'_, 'source>,
    operator: OperatorView<'_, 'source>,
    line: &'source LogicalLine,
    kind: OperatorKind,
    location: SemanticLocation,
    document_index: usize,
) -> Result<usize, SemanticError> {
    let decoded_len = operator.argument().decoded_len();
    build
        .templates
        .check_operand_scan(decoded_len, operator.argument_span().start)?;
    let required = !matches!(
        kind,
        OperatorKind::DetectSqli
            | OperatorKind::DetectXss
            | OperatorKind::UnconditionalMatch
            | OperatorKind::ValidateUrlEncoding
            | OperatorKind::ValidateUtf8Encoding
            | OperatorKind::Rx
    );
    if required && decoded_len == 0 {
        return Err(SemanticError::OperatorOperandPresence {
            expected: OperatorOperandContract::NonEmpty,
            position: operator.argument_span().start,
        });
    }
    let no_argument = matches!(
        kind,
        OperatorKind::DetectSqli
            | OperatorKind::DetectXss
            | OperatorKind::UnconditionalMatch
            | OperatorKind::ValidateUrlEncoding
            | OperatorKind::ValidateUtf8Encoding
    );
    if no_argument && decoded_len != 0 {
        return Err(SemanticError::OperatorOperandPresence {
            expected: OperatorOperandContract::Empty,
            position: operand_position(operator, line, 0),
        });
    }

    let template_kind = operator_macro_capable(kind);
    if template_kind {
        let shape = walk_template(operator.argument().bytes(), |_| {}).map_err(|error| SemanticError::MacroSyntax {
            kind: error.kind,
            position: macro_error_position_operator(error, operator, line),
        })?;
        let constant = if matches!(
            kind,
            OperatorKind::Eq | OperatorKind::Ge | OperatorKind::Gt | OperatorKind::Lt
        ) && shape.expansions == 0
        {
            Some(strict_i64(operator.argument().bytes()).map_err(|(error_kind, offset)| {
                SemanticError::IntegerOperand {
                    kind: error_kind,
                    position: operand_position(operator, line, offset),
                }
            })?)
        } else {
            None
        };
        let template = attach_operator_template(&mut build.templates, operator, line, location)?;
        let operand = match kind {
            OperatorKind::Eq | OperatorKind::Ge | OperatorKind::Gt | OperatorKind::Lt => {
                OperatorOperandRecord::IntegerTemplate { template, constant }
            }
            _ => OperatorOperandRecord::StringTemplate(template),
        };
        let id = build.operands.len();
        build.operands.push(operand);
        return Ok(id);
    }

    let operand = match kind {
        OperatorKind::DetectSqli
        | OperatorKind::DetectXss
        | OperatorKind::UnconditionalMatch
        | OperatorKind::ValidateUrlEncoding
        | OperatorKind::ValidateUtf8Encoding => {
            build.prepare_metadata_only(0, 0, operator.argument_span().start)?;
            OperatorOperandRecord::NoArgument
        }
        OperatorKind::Rx => {
            let storage = build.store_static(operator, 0, 0, 1, 0, operator.argument_span().start)?;
            OperatorOperandRecord::Regex(storage)
        }
        OperatorKind::Pm | OperatorKind::PmFromFile => {
            let bytes = operator.argument().bytes();
            let mut payload_bytes = 0usize;
            let mut mapping_error = None;
            let count = if kind == OperatorKind::PmFromFile {
                let shape = walk_asset_words(
                    bytes.clone(),
                    build.templates.limits.operands.max_asset_bytes,
                    |range| {
                        if mapping_error.is_none() {
                            mapping_error = operator_mapped_text(operator, line, range).err();
                        }
                    },
                )
                .map_err(|error| match error {
                    AssetWalkError::ByteLimit { offset } => SemanticError::AssetByteLimit {
                        limit: build.templates.limits.operands.max_asset_bytes,
                        position: operand_position(operator, line, offset),
                    },
                    AssetWalkError::Unsafe { kind, offset } => SemanticError::UnsafeAsset {
                        kind,
                        position: operand_position(operator, line, offset),
                    },
                })?;
                payload_bytes = shape.payload_bytes;
                shape.count
            } else {
                split_ascii_words(bytes.clone(), |range| {
                    payload_bytes += range.len();
                    if mapping_error.is_none() {
                        mapping_error = operator_mapped_text(operator, line, range).err();
                    }
                })
            };
            if count == 0 {
                return Err(SemanticError::OperatorOperandPresence {
                    expected: OperatorOperandContract::NonEmpty,
                    position: operator.argument_span().start,
                });
            }
            if let Some(error) = mapping_error {
                return Err(error);
            }
            if kind == OperatorKind::Pm {
                if let Some(offset) = bytes.clone().position(|byte| byte == b'|') {
                    return Err(SemanticError::PhraseSyntax {
                        position: operand_position(operator, line, offset),
                    });
                }
                if count > build.templates.limits.operands.max_phrases_per_operator {
                    return Err(SemanticError::OperatorPhraseLimit {
                        limit: build.templates.limits.operands.max_phrases_per_operator,
                        position: operator.argument_span().start,
                    });
                }
                let phrase_count = build
                    .phrase_count
                    .checked_add(count)
                    .ok_or(SemanticError::PhraseLimit {
                        limit: build.templates.limits.operands.max_phrases,
                        position: operator.argument_span().start,
                    })?;
                if phrase_count > build.templates.limits.operands.max_phrases {
                    return Err(SemanticError::PhraseLimit {
                        limit: build.templates.limits.operands.max_phrases,
                        position: operator.argument_span().start,
                    });
                }
                let phrase_bytes =
                    build
                        .phrase_bytes
                        .checked_add(payload_bytes)
                        .ok_or(SemanticError::PhraseByteLimit {
                            limit: build.templates.limits.operands.max_phrase_bytes,
                            position: operator.argument_span().start,
                        })?;
                if phrase_bytes > build.templates.limits.operands.max_phrase_bytes {
                    return Err(SemanticError::PhraseByteLimit {
                        limit: build.templates.limits.operands.max_phrase_bytes,
                        position: operator.argument_span().start,
                    });
                }
                let storage = build.store_static(operator, count, 0, 1, 0, operator.argument_span().start)?;
                let start = build.list_items.len();
                split_ascii_words(bytes, |range| {
                    let span = operator_mapped_text(operator, line, range.clone())
                        .expect("phrase ranges were source-mapped before candidate commit")
                        .span();
                    build.list_items.push(ListItemRecord {
                        start: range.start,
                        end: range.end,
                        span,
                    });
                });
                *build.phrase_count = phrase_count;
                *build.phrase_bytes = phrase_bytes;
                OperatorOperandRecord::PhraseList { storage, start, count }
            } else {
                if count > build.templates.limits.operands.max_assets_per_operator {
                    return Err(SemanticError::OperatorAssetLimit {
                        limit: build.templates.limits.operands.max_assets_per_operator,
                        position: operator.argument_span().start,
                    });
                }
                let asset_count = build.asset_count.checked_add(count).ok_or(SemanticError::AssetLimit {
                    limit: build.templates.limits.operands.max_assets,
                    position: operator.argument_span().start,
                })?;
                if asset_count > build.templates.limits.operands.max_assets {
                    return Err(SemanticError::AssetLimit {
                        limit: build.templates.limits.operands.max_assets,
                        position: operator.argument_span().start,
                    });
                }
                let storage = build.store_static(operator, count, 0, 2, 0, operator.argument_span().start)?;
                let start = build.list_items.len();
                split_ascii_words(bytes, |range| {
                    let span = operator_mapped_text(operator, line, range.clone())
                        .expect("asset ranges were source-mapped before candidate commit")
                        .span();
                    build.list_items.push(ListItemRecord {
                        start: range.start,
                        end: range.end,
                        span,
                    });
                });
                *build.asset_count = asset_count;
                OperatorOperandRecord::ExternalAssets {
                    storage,
                    start,
                    count,
                    document: document_index,
                }
            }
        }
        OperatorKind::IpMatch => {
            if operator.argument_has_removable_escapes() {
                let offset = operator.argument().bytes().position(|byte| byte == b'\"').unwrap_or(0);
                return Err(SemanticError::IpNetworkSyntax {
                    position: operand_position(operator, line, offset),
                });
            }
            let bytes = operator
                .argument()
                .as_borrowed()
                .expect("escape-free operator arguments remain borrowed")
                .as_bytes();
            let mut grammar_error = None;
            let mut mapping_error = None;
            let count = walk_comma_ranges(bytes, |range| {
                if mapping_error.is_none() {
                    mapping_error = operator_mapped_text(operator, line, range.clone()).err();
                }
                if grammar_error.is_none() {
                    grammar_error = parse_network(bytes, range).err();
                }
            })
            .map_err(|offset| SemanticError::IpNetworkSyntax {
                position: operand_position(operator, line, offset),
            })?;
            if let Some(error) = mapping_error {
                return Err(error);
            }
            if let Some(offset) = grammar_error {
                return Err(SemanticError::IpNetworkSyntax {
                    position: operand_position(operator, line, offset),
                });
            }
            if count > build.templates.limits.operands.max_networks_per_operator {
                return Err(SemanticError::OperatorNetworkLimit {
                    limit: build.templates.limits.operands.max_networks_per_operator,
                    position: operator.argument_span().start,
                });
            }
            if build
                .networks
                .len()
                .checked_add(count)
                .is_none_or(|count| count > build.templates.limits.operands.max_networks)
            {
                return Err(SemanticError::NetworkLimit {
                    limit: build.templates.limits.operands.max_networks,
                    position: operator.argument_span().start,
                });
            }
            let storage = build.store_static(operator, 0, count, 0, 0, operator.argument_span().start)?;
            let start = build.networks.len();
            walk_comma_ranges(bytes, |range| {
                let (address, prefix) = parse_network(bytes, range.clone())
                    .expect("IP network grammar was validated before candidate commit");
                build.networks.push(NetworkRecord {
                    address,
                    prefix,
                    span: operator_mapped_text(operator, line, range)
                        .expect("IP ranges were source-mapped before candidate commit")
                        .span(),
                });
            })
            .expect("IP member shape was validated before candidate commit");
            OperatorOperandRecord::IpNetworks {
                storage,
                start,
                count: build.networks.len() - start,
            }
        }
        OperatorKind::ValidateByteRange => {
            if operator.argument_has_removable_escapes() {
                let offset = operator.argument().bytes().position(|byte| byte == b'\"').unwrap_or(0);
                return Err(SemanticError::ByteSetSyntax {
                    position: operand_position(operator, line, offset),
                });
            }
            let bytes = operator
                .argument()
                .as_borrowed()
                .expect("escape-free operator arguments remain borrowed")
                .as_bytes();
            let mut words = [0u64; 4];
            let mut grammar_error = None;
            let member_count = walk_comma_ranges(bytes, |range| {
                if grammar_error.is_none() {
                    match parse_byte_member(bytes, range.clone()) {
                        Ok((start, end)) => {
                            for value in start..=end {
                                words[usize::from(value) / 64] |= 1u64 << (usize::from(value) % 64);
                            }
                        }
                        Err(offset) => grammar_error = Some(offset),
                    }
                }
            })
            .map_err(|offset| SemanticError::ByteSetSyntax {
                position: operand_position(operator, line, offset),
            })?;
            if let Some(offset) = grammar_error {
                return Err(SemanticError::ByteSetSyntax {
                    position: operand_position(operator, line, offset),
                });
            }
            if member_count > build.templates.limits.operands.max_byte_range_members {
                return Err(SemanticError::ByteRangeMemberLimit {
                    limit: build.templates.limits.operands.max_byte_range_members,
                    position: operator.argument_span().start,
                });
            }
            let storage = build.store_static(operator, 0, 0, 0, 0, operator.argument_span().start)?;
            OperatorOperandRecord::ByteSet {
                storage,
                words,
                member_count,
            }
        }
        _ => unreachable!("template-capable operators returned before static classification"),
    };
    *build.templates.scanned += decoded_len;
    let id = build.operands.len();
    build.operands.push(operand);
    match kind {
        OperatorKind::Rx => build.templates.requirements.push(RequirementRecord {
            location,
            requirement: CompileRequirement::RegexCompilation,
        }),
        OperatorKind::Pm => build.templates.requirements.push(RequirementRecord {
            location,
            requirement: CompileRequirement::PhraseMatcherCompilation,
        }),
        OperatorKind::PmFromFile => {
            build.templates.requirements.push(RequirementRecord {
                location,
                requirement: CompileRequirement::ExternalPhraseAsset,
            });
            build.templates.requirements.push(RequirementRecord {
                location,
                requirement: CompileRequirement::PhraseMatcherCompilation,
            });
        }
        _ => {}
    }
    Ok(id)
}

fn parse_network(bytes: &[u8], range: Range<usize>) -> Result<(IpAddr, u8), usize> {
    let member = &bytes[range.clone()];
    let slash = member.iter().position(|byte| *byte == b'/');
    if slash.is_some_and(|first| member[first + 1..].contains(&b'/')) {
        return Err(range.start + slash.unwrap_or(0) + 1);
    }
    let address_end = slash.map_or(range.end, |offset| range.start + offset);
    let address_text = std::str::from_utf8(&bytes[range.start..address_end]).map_err(|_| range.start)?;
    let address = address_text.parse::<IpAddr>().map_err(|_| range.start)?;
    let maximum = if address.is_ipv4() { 32 } else { 128 };
    let prefix = if let Some(offset) = slash {
        let prefix_start = range.start + offset + 1;
        let prefix_bytes = &bytes[prefix_start..range.end];
        if prefix_bytes.is_empty() {
            return Err(prefix_start);
        }
        let mut value = 0u16;
        for (index, byte) in prefix_bytes.iter().copied().enumerate() {
            if !byte.is_ascii_digit() {
                return Err(prefix_start + index);
            }
            value = value
                .checked_mul(10)
                .and_then(|current| current.checked_add(u16::from(byte - b'0')))
                .filter(|current| *current <= maximum)
                .ok_or(prefix_start + index)?;
        }
        value as u8
    } else {
        maximum as u8
    };
    Ok((mask_ip(address, prefix), prefix))
}
