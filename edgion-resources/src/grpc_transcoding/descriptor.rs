//! Admission before reflection expands descriptor namespace prefixes.
use prost::Message;
use prost_types::field_descriptor_proto::Type;
use prost_types::{DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, FileDescriptorSet};

const NAME_LIMIT: usize = 512;
const HTTP_OPTION: i32 = 72_295_728;

pub(super) fn admission_pool() -> &'static prost_reflect::DescriptorPool {
    static POOL: std::sync::LazyLock<prost_reflect::DescriptorPool> = std::sync::LazyLock::new(|| {
        prost_reflect::DescriptorPool::decode(include_bytes!("descriptor_admission.pb").as_slice())
            .expect("checked-in descriptor admission schema is valid")
    });
    &POOL
}
fn reserved_shape(message: &DescriptorProto, full_name: &str) -> Result<(), String> {
    if !full_name.starts_with("google.protobuf.")
        && !matches!(
            full_name,
            "google.api.HttpRule" | "google.api.CustomHttpPattern" | "google.api.Http"
        )
    {
        return Ok(());
    }
    let Some(canonical) = admission_pool()
        .get_message_by_name(full_name)
        .or_else(|| prost_reflect::DescriptorPool::global().get_message_by_name(full_name))
    else {
        return Ok(());
    };
    if !message.extension.is_empty()
        || message
            .options
            .as_ref()
            .is_some_and(|options| options.map_entry.unwrap_or(false))
            != canonical.is_map_entry()
        || message.oneof_decl.iter().enumerate().any(|(index, oneof)| {
            canonical
                .oneofs()
                .nth(index)
                .is_none_or(|expected| oneof.name.as_deref() != Some(expected.name()))
        })
        || message.field.iter().any(|field| {
            field
                .number
                .and_then(|number| u32::try_from(number).ok())
                .and_then(|number| canonical.get_field(number))
                .is_none_or(|expected| {
                    let expected = expected.field_descriptor_proto();
                    field.name != expected.name
                        || field.r#type != expected.r#type
                        || field.label != expected.label
                        || field.type_name != expected.type_name
                        || field.oneof_index != expected.oneof_index
                })
        })
    {
        return Err("noncanonical reserved descriptor shape (including well-known types)".into());
    }
    if !full_name.starts_with("google.protobuf.") && !message.extension_range.is_empty() {
        return Err("HTTP annotation message extensions are unsupported".into());
    }
    Ok(())
}
fn extension_profile(field: &FieldDescriptorProto, prefix: &str) -> Result<(), String> {
    if field.extendee.as_deref().is_none_or(|name| !name.starts_with('.')) {
        return Err("descriptor extension extendee must be absolute".into());
    }
    let full_name = if prefix.is_empty() {
        field.name.clone().unwrap_or_default()
    } else {
        format!("{prefix}.{}", field.name.as_deref().unwrap_or(""))
    };
    let extendee = field.extendee.as_deref().unwrap_or("").trim_start_matches('.');
    if admission_pool()
        .get_message_by_name(extendee)
        .or_else(|| prost_reflect::DescriptorPool::global().get_message_by_name(extendee))
        .is_some_and(|canonical| {
            field
                .number
                .and_then(|number| u32::try_from(number).ok())
                .is_some_and(|number| canonical.get_field(number).is_some())
        })
    {
        return Err("descriptor extension shadows a reserved field".into());
    }
    if (full_name == "google.api.http"
        || (extendee == "google.protobuf.MethodOptions" && field.number == Some(HTTP_OPTION)))
        && (full_name != "google.api.http"
            || field.number != Some(HTTP_OPTION)
            || field.extendee.as_deref() != Some(".google.protobuf.MethodOptions")
            || field.r#type != Some(Type::Message as i32)
            || field.type_name.as_deref() != Some(".google.api.HttpRule")
            || field.label != Some(prost_types::field_descriptor_proto::Label::Optional as i32))
    {
        return Err("noncanonical HTTP option extension".into());
    }
    if matches!(
        extendee,
        "google.api.HttpRule" | "google.api.CustomHttpPattern" | "google.api.Http"
    ) {
        return Err("HTTP annotation message extensions are unsupported".into());
    }
    Ok(())
}
fn identifier(name: &str) -> Result<(), String> {
    let mut bytes = name.bytes();
    if name.is_empty()
        || name.len() > 128
        || !bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        || bytes.any(|b| !b.is_ascii_alphanumeric() && b != b'_')
    {
        return Err("descriptor identifier exceeds limits or is invalid".into());
    }
    Ok(())
}
fn qualified(name: &str, segments: usize) -> Result<(), String> {
    let name = name.strip_prefix('.').unwrap_or(name);
    if name.len() > NAME_LIMIT || name.split('.').count() > segments {
        return Err("descriptor namespace exceeds limits".into());
    }
    for part in name.split('.') {
        identifier(part)?;
    }
    Ok(())
}
fn child(prefix: usize, name: &str) -> Result<usize, String> {
    identifier(name)?;
    let len = prefix
        .saturating_add(usize::from(prefix != 0))
        .saturating_add(name.len());
    if len > NAME_LIMIT {
        return Err("descriptor qualified name exceeds limits".into());
    }
    Ok(len)
}
fn field(field: &FieldDescriptorProto, prefix: usize, count: &mut usize) -> Result<(), String> {
    // Reflection decodes custom options while building the pool, before the
    // post-pool message profile validation. Defaults must be admitted first.
    if matches!(field.r#type, Some(kind) if kind == Type::String as i32 || kind == Type::Bytes as i32)
        && field.default_value.as_ref().is_some_and(|value| !value.is_empty())
    {
        return Err("explicit string and bytes defaults are unsupported".into());
    }
    *count += 1;
    if *count > 8192 {
        return Err("descriptor field inventory exceeds limits".into());
    }
    child(prefix, field.name.as_deref().unwrap_or(""))?;
    for name in [&field.type_name, &field.extendee].into_iter().flatten() {
        qualified(name, 64)?;
    }
    if field.json_name.as_ref().is_some_and(|name| name.len() > NAME_LIMIT) {
        return Err("descriptor JSON field name exceeds limits".into());
    }
    Ok(())
}
fn enumeration(value: &EnumDescriptorProto, prefix: usize) -> Result<(), String> {
    let len = child(prefix, value.name.as_deref().unwrap_or(""))?;
    for value in &value.value {
        child(len, value.name.as_deref().unwrap_or(""))?;
    }
    Ok(())
}
fn message(
    message: &DescriptorProto,
    prefix: &str,
    depth: usize,
    messages: &mut usize,
    fields: &mut usize,
) -> Result<(), String> {
    *messages += 1;
    if depth > 32 || *messages > 1024 {
        return Err("descriptor message inventory exceeds limits".into());
    }
    let name = message.name.as_deref().unwrap_or("");
    let len = child(prefix.len(), name)?;
    let full_name = if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}.{name}")
    };
    reserved_shape(message, &full_name)?;
    for entry in &message.field {
        field(entry, len, fields)?;
    }
    for entry in &message.extension {
        field(entry, len, fields)?;
        extension_profile(entry, &full_name)?;
    }
    for entry in &message.oneof_decl {
        child(len, entry.name.as_deref().unwrap_or(""))?;
    }
    for entry in &message.enum_type {
        enumeration(entry, len)?;
    }
    for entry in &message.nested_type {
        self::message(entry, &full_name, depth + 1, messages, fields)?;
    }
    Ok(())
}
pub(super) fn admit(bytes: &[u8]) -> Result<(), String> {
    // The canonical descriptor wire preflight has already bounded recursion,
    // structural nodes and bytes. This typed decode cannot expand namespaces.
    let set = FileDescriptorSet::decode(bytes).map_err(|_| "invalid descriptor")?;
    if set.file.len() > 128 {
        return Err("descriptor file inventory exceeds limits".into());
    }
    let mut messages = 0;
    let mut fields = 0;
    for file in set.file {
        if file
            .name
            .as_ref()
            .is_none_or(|name| name.is_empty() || name.len() > 4096)
            || file.dependency.iter().any(|name| name.len() > 4096)
        {
            return Err("descriptor file name exceeds limits".into());
        }
        let package = file.package.as_deref().unwrap_or("");
        if !package.is_empty() {
            if package.len() > 256 {
                return Err("descriptor namespace exceeds limits".into());
            }
            qualified(package, 16)?;
        }
        for entry in &file.message_type {
            message(entry, package, 1, &mut messages, &mut fields)?;
        }
        for entry in &file.extension {
            field(entry, package.len(), &mut fields)?;
            extension_profile(entry, package)?;
        }
        for entry in &file.enum_type {
            enumeration(entry, package.len())?;
        }
        for service in file.service {
            let len = child(package.len(), service.name.as_deref().unwrap_or(""))?;
            for method in service.method {
                child(len, method.name.as_deref().unwrap_or(""))?;
                for name in [method.input_type, method.output_type].into_iter().flatten() {
                    qualified(&name, 64)?;
                }
            }
        }
    }
    Ok(())
}
