use super::*;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct UsageEventObservation {
    pub(super) reported_usage: Option<ReportedAiUsage>,
    pub(super) output_units: u64,
}

#[derive(Debug, Default, Clone, Copy)]
struct UsageFields {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    details_cached_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
}

#[cfg(test)]
pub(super) fn observe_openai_streaming_event(json: &[u8]) -> Option<UsageEventObservation> {
    let mut deserializer = serde_json::Deserializer::from_slice(json);
    let observation = RootSeed.deserialize(&mut deserializer).ok()?;
    deserializer.end().ok()?;
    Some(observation)
}

#[cfg(test)]
struct RootSeed;

#[derive(Clone, Copy)]
enum ObservationField {
    Usage,
    Choices,
    PromptTokens,
    CompletionTokens,
    PromptTokensDetails,
    CachedTokens,
    CacheReadInputTokens,
    CacheCreationInputTokens,
    Delta,
    Content,
    ToolCalls,
    Function,
    Arguments,
    Other,
}

struct ObservationFieldSeed;

impl<'de> DeserializeSeed<'de> for ObservationFieldSeed {
    type Value = ObservationField;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_identifier(ObservationFieldVisitor)
    }
}

struct ObservationFieldVisitor;

fn observation_field(value: &str) -> ObservationField {
    match value {
        "usage" => ObservationField::Usage,
        "choices" => ObservationField::Choices,
        "prompt_tokens" => ObservationField::PromptTokens,
        "completion_tokens" => ObservationField::CompletionTokens,
        "prompt_tokens_details" => ObservationField::PromptTokensDetails,
        "cached_tokens" => ObservationField::CachedTokens,
        "cache_read_input_tokens" => ObservationField::CacheReadInputTokens,
        "cache_creation_input_tokens" => ObservationField::CacheCreationInputTokens,
        "delta" => ObservationField::Delta,
        "content" => ObservationField::Content,
        "tool_calls" => ObservationField::ToolCalls,
        "function" => ObservationField::Function,
        "arguments" => ObservationField::Arguments,
        _ => ObservationField::Other,
    }
}

impl Visitor<'_> for ObservationFieldVisitor {
    type Value = ObservationField;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON object field")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(observation_field(value))
    }

    fn visit_borrowed_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(observation_field(value))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(observation_field(&value))
    }
}

#[cfg(test)]
impl<'de> DeserializeSeed<'de> for RootSeed {
    type Value = UsageEventObservation;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(RootVisitor)
    }
}

struct RootVisitor;

impl<'de> serde::Deserialize<'de> for UsageEventObservation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(RootVisitor)
    }
}

impl<'de> Visitor<'de> for RootVisitor {
    type Value = UsageEventObservation;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an OpenAI response object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut usage = None;
        let mut output_units = None;
        while let Some(key) = map.next_key_seed(ObservationFieldSeed)? {
            match key {
                ObservationField::Usage => usage = map.next_value_seed(UsageSeed)?,
                ObservationField::Choices => output_units = map.next_value_seed(ChoicesSeed)?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let reported_usage = usage.and_then(|usage: UsageFields| {
            Some(ReportedAiUsage {
                input_tokens: usage.prompt_tokens?,
                output_tokens: usage.completion_tokens?,
                cache_read_input_tokens: usage
                    .details_cached_tokens
                    .or(usage.cache_read_input_tokens)
                    .unwrap_or(0),
                cache_write_input_tokens: usage.cache_creation_input_tokens.unwrap_or(0),
            })
        });
        Ok(UsageEventObservation {
            reported_usage,
            output_units: output_units.unwrap_or(0),
        })
    }

    ignore_json_value_except_map!(UsageEventObservation::default());
}

struct UsageSeed;

impl<'de> DeserializeSeed<'de> for UsageSeed {
    type Value = Option<UsageFields>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(UsageVisitor)
    }
}

struct UsageVisitor;

impl<'de> Visitor<'de> for UsageVisitor {
    type Value = Option<UsageFields>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an optional usage object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut usage = UsageFields::default();
        while let Some(key) = map.next_key_seed(ObservationFieldSeed)? {
            match key {
                ObservationField::PromptTokens => usage.prompt_tokens = map.next_value_seed(U64Seed)?,
                ObservationField::CompletionTokens => usage.completion_tokens = map.next_value_seed(U64Seed)?,
                ObservationField::PromptTokensDetails => {
                    usage.details_cached_tokens = map.next_value_seed(CachedDetailsSeed)?;
                }
                ObservationField::CacheReadInputTokens => {
                    usage.cache_read_input_tokens = map.next_value_seed(U64Seed)?;
                }
                ObservationField::CacheCreationInputTokens => {
                    usage.cache_creation_input_tokens = map.next_value_seed(U64Seed)?;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(Some(usage))
    }

    ignore_json_value_except_map!(None);
}

struct CachedDetailsSeed;

impl<'de> DeserializeSeed<'de> for CachedDetailsSeed {
    type Value = Option<u64>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(CachedDetailsVisitor)
    }
}

struct CachedDetailsVisitor;

impl<'de> Visitor<'de> for CachedDetailsVisitor {
    type Value = Option<u64>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an optional prompt token details object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut cached = None;
        while let Some(key) = map.next_key_seed(ObservationFieldSeed)? {
            if matches!(key, ObservationField::CachedTokens) {
                cached = map.next_value_seed(U64Seed)?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(cached)
    }

    ignore_json_value_except_map!(None);
}

struct U64Seed;

impl<'de> DeserializeSeed<'de> for U64Seed {
    type Value = Option<u64>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(U64Visitor)
    }
}

struct U64Visitor;

impl<'de> Visitor<'de> for U64Visitor {
    type Value = Option<u64>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an optional unsigned integer")
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Some(value))
    }

    ignore_json_value_except_u64!(None);
}

struct ChoicesSeed;

impl<'de> DeserializeSeed<'de> for ChoicesSeed {
    type Value = Option<u64>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ChoicesVisitor)
    }
}

struct ChoicesVisitor;

impl<'de> Visitor<'de> for ChoicesVisitor {
    type Value = Option<u64>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an optional choices array")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut units = 0_u64;
        while let Some(qualifies) = sequence.next_element_seed(ChoiceSeed)? {
            units = units.saturating_add(u64::from(qualifies));
        }
        Ok(Some(units))
    }

    ignore_json_value_except_seq!(None);
}

struct ChoiceSeed;

impl<'de> DeserializeSeed<'de> for ChoiceSeed {
    type Value = bool;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ChoiceVisitor)
    }
}

struct ChoiceVisitor;

impl<'de> Visitor<'de> for ChoiceVisitor {
    type Value = bool;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a choice object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut delta = false;
        while let Some(key) = map.next_key_seed(ObservationFieldSeed)? {
            if matches!(key, ObservationField::Delta) {
                delta = map.next_value_seed(DeltaSeed)?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(delta)
    }

    ignore_json_value_except_map!(false);
}

struct DeltaSeed;

impl<'de> DeserializeSeed<'de> for DeltaSeed {
    type Value = bool;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(DeltaVisitor)
    }
}

struct DeltaVisitor;

impl<'de> Visitor<'de> for DeltaVisitor {
    type Value = bool;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a delta object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut content = false;
        let mut tool_calls = false;
        while let Some(key) = map.next_key_seed(ObservationFieldSeed)? {
            match key {
                ObservationField::Content => content = map.next_value_seed(NonEmptyStringSeed)?,
                ObservationField::ToolCalls => tool_calls = map.next_value_seed(ToolCallsSeed)?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(content || tool_calls)
    }

    ignore_json_value_except_map!(false);
}

struct NonEmptyStringSeed;

impl<'de> DeserializeSeed<'de> for NonEmptyStringSeed {
    type Value = bool;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(NonEmptyStringVisitor)
    }
}

struct NonEmptyStringVisitor;

impl<'de> Visitor<'de> for NonEmptyStringVisitor {
    type Value = bool;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an optional non-empty string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(!value.is_empty())
    }

    fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E> {
        Ok(!value.is_empty())
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(!value.is_empty())
    }

    ignore_json_value_except_string!(false);
}

struct ToolCallsSeed;

impl<'de> DeserializeSeed<'de> for ToolCallsSeed {
    type Value = bool;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ToolCallsVisitor)
    }
}

struct ToolCallsVisitor;

impl<'de> Visitor<'de> for ToolCallsVisitor {
    type Value = bool;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an optional tool calls array")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut any = false;
        while let Some(qualifies) = sequence.next_element_seed(ToolCallSeed)? {
            any |= qualifies;
        }
        Ok(any)
    }

    ignore_json_value_except_seq!(false);
}

struct ToolCallSeed;

impl<'de> DeserializeSeed<'de> for ToolCallSeed {
    type Value = bool;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ToolCallVisitor)
    }
}

struct ToolCallVisitor;

impl<'de> Visitor<'de> for ToolCallVisitor {
    type Value = bool;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a tool call object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut function = false;
        while let Some(key) = map.next_key_seed(ObservationFieldSeed)? {
            if matches!(key, ObservationField::Function) {
                function = map.next_value_seed(FunctionSeed)?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(function)
    }

    ignore_json_value_except_map!(false);
}

struct FunctionSeed;

impl<'de> DeserializeSeed<'de> for FunctionSeed {
    type Value = bool;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(FunctionVisitor)
    }
}

struct FunctionVisitor;

impl<'de> Visitor<'de> for FunctionVisitor {
    type Value = bool;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a tool function object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut arguments = false;
        while let Some(key) = map.next_key_seed(ObservationFieldSeed)? {
            if matches!(key, ObservationField::Arguments) {
                arguments = map.next_value_seed(NonEmptyStringSeed)?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(arguments)
    }

    ignore_json_value_except_map!(false);
}

pub(super) fn approximate_openai_output_units(value: &serde_json::Value) -> u64 {
    value
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .filter(|choice| {
                    let Some(delta) = choice.get("delta").and_then(serde_json::Value::as_object) else {
                        return false;
                    };
                    delta
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|content| !content.is_empty())
                        || delta
                            .get("tool_calls")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|calls| {
                                calls.iter().any(|call| {
                                    call.get("function")
                                        .and_then(serde_json::Value::as_object)
                                        .and_then(|function| function.get("arguments"))
                                        .and_then(serde_json::Value::as_str)
                                        .is_some_and(|arguments| !arguments.is_empty())
                                })
                            })
                })
                .count() as u64
        })
        .unwrap_or(0)
}
