use super::*;

const LIMIT: usize = 1024 * 1024;

fn base_request() -> Value {
    json!({
        "model": "client-alias",
        "messages": [{"role": "user", "content": "Hello"}],
        "max_completion_tokens": 64,
    })
}

fn prepare(request: Value) -> Result<PreparedAnthropicRequest, AnthropicRequestError> {
    prepare_anthropic_request(request, "claude-canonical", None, false, LIMIT)
}

fn body(prepared: &PreparedAnthropicRequest) -> Value {
    serde_json::from_slice(&prepared.body).expect("translated body must be JSON")
}

#[test]
fn translates_basic_request_with_canonical_model() {
    let prepared = prepare(base_request()).expect("request must translate");
    assert!(!prepared.requested_streaming);
    assert!(!prepared.effective_usage_chunk);
    assert_eq!(
        body(&prepared),
        json!({
            "model": "claude-canonical",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [{"type": "text", "text": "Hello"}],
            }],
            "stream": false,
        })
    );
}

#[test]
fn moves_contiguous_system_prefix_and_text_parts() {
    let request = json!({
        "model": "alias",
        "messages": [
            {"role": "system", "content": "First"},
            {"role": "system", "content": [{"type": "text", "text": "Second"}]},
            {"role": "user", "content": [{"type": "text", "text": "Question"}]},
        ],
        "max_tokens": 10,
    });
    let translated = body(&prepare(request).expect("request must translate"));
    assert_eq!(
        translated["system"],
        json!([
            {"type": "text", "text": "First"},
            {"type": "text", "text": "Second"},
        ])
    );
    assert_eq!(translated["messages"].as_array().map(Vec::len), Some(1));
}

#[test]
fn resolves_legacy_completion_and_default_token_limits() {
    let legacy_only = json!({
        "model": "alias",
        "messages": [{"role": "user", "content": "Hello"}],
        "max_tokens": 42,
    });
    assert_eq!(body(&prepare(legacy_only).unwrap())["max_tokens"], 42);

    let completion_only = json!({
        "model": "alias",
        "messages": [{"role": "user", "content": "Hello"}],
        "max_completion_tokens": 43,
    });
    let prepared = prepare_anthropic_request(completion_only, "canonical", Some(17), false, LIMIT).unwrap();
    assert_eq!(body(&prepared)["max_tokens"], 43);

    let without_limit = json!({
        "model": "alias",
        "messages": [{"role": "user", "content": "Hello"}],
    });
    let prepared = prepare_anthropic_request(without_limit, "canonical", Some(17), false, LIMIT).unwrap();
    assert_eq!(body(&prepared)["max_tokens"], 17);

    let mut conflict = base_request();
    conflict["max_tokens"] = json!(63);
    assert_eq!(prepare(conflict), Err(AnthropicRequestError::InvalidRequestBody));

    let missing = json!({
        "model": "alias",
        "messages": [{"role": "user", "content": "Hello"}],
    });
    assert_eq!(prepare(missing), Err(AnthropicRequestError::MissingRequiredParameter));
}

#[test]
fn validates_token_limit_type_null_and_range() {
    for value in [json!(0), json!(-1), json!(1.5), json!(u32::MAX as u64 + 1), json!("1")] {
        let mut request = base_request();
        request["max_completion_tokens"] = value;
        assert_eq!(prepare(request), Err(AnthropicRequestError::InvalidRequestBody));
    }
    let mut request = base_request();
    request["max_completion_tokens"] = Value::Null;
    assert_eq!(prepare(request), Err(AnthropicRequestError::MissingRequiredParameter));
}

#[test]
fn computes_stream_and_effective_usage_flags() {
    let mut request = base_request();
    request["stream"] = json!(true);
    request["stream_options"] = json!({"include_usage": false});
    let prepared = prepare_anthropic_request(request.clone(), "canonical", None, true, LIMIT).unwrap();
    assert!(prepared.requested_streaming);
    assert!(prepared.effective_usage_chunk);

    request["stream_options"] = json!({"include_usage": true});
    let prepared = prepare_anthropic_request(request, "canonical", None, false, LIMIT).unwrap();
    assert!(prepared.effective_usage_chunk);
    assert!(body(&prepared).get("stream_options").is_none());
}

#[test]
fn validates_stream_options_strictly() {
    for options in [
        json!({}),
        json!({"include_usage": null}),
        json!({"include_usage": true, "extra": false}),
    ] {
        let mut request = base_request();
        request["stream"] = json!(true);
        request["stream_options"] = options;
        assert_eq!(prepare(request), Err(AnthropicRequestError::InvalidRequestBody));
    }
    let mut not_streaming = base_request();
    not_streaming["stream_options"] = json!({"include_usage": true});
    assert_eq!(prepare(not_streaming), Err(AnthropicRequestError::UnsupportedParameter));
}

#[test]
fn maps_stops_and_rejects_invalid_shapes() {
    let mut request = base_request();
    request["stop"] = json!("END");
    assert_eq!(body(&prepare(request).unwrap())["stop_sequences"], json!(["END"]));

    let mut request = base_request();
    request["stop"] = json!(["one", "two", "three", "four"]);
    assert_eq!(
        body(&prepare(request).unwrap())["stop_sequences"],
        json!(["one", "two", "three", "four"])
    );

    for stop in [
        json!(""),
        json!([]),
        json!(["a", "b", "c", "d", "e"]),
        json!(["ok", ""]),
    ] {
        let mut request = base_request();
        request["stop"] = stop;
        assert_eq!(prepare(request), Err(AnthropicRequestError::InvalidRequestBody));
    }
}

#[test]
fn maps_tools_calls_results_and_named_choice() {
    let request = json!({
        "model": "alias",
        "messages": [
            {"role": "user", "content": "Weather?"},
            {"role": "assistant", "content": "Checking now", "tool_calls": [
                {"id": "call-a", "type": "function", "function": {"name": "weather", "arguments": "{\"city\":\"Paris\"}"}},
                {"id": "call-b", "type": "function", "function": {"name": "clock", "arguments": "{}"}},
            ]},
            {"role": "tool", "tool_call_id": "call-b", "content": "12:00"},
            {"role": "tool", "tool_call_id": "call-a", "content": [{"type": "text", "text": "Sunny"}]},
        ],
        "max_tokens": 32,
        "tools": [
            {"type": "function", "function": {"name": "weather", "description": "Get weather", "parameters": {"type": "object"}}},
            {"type": "function", "function": {"name": "clock", "parameters": {}}},
        ],
        "tool_choice": {"type": "function", "function": {"name": "weather"}},
        "parallel_tool_calls": false,
    });
    let translated = body(&prepare(request).expect("tool conversation must translate"));
    assert_eq!(translated["tools"][0]["input_schema"], json!({"type": "object"}));
    assert_eq!(
        translated["tool_choice"],
        json!({"type": "tool", "name": "weather", "disable_parallel_tool_use": true})
    );
    assert_eq!(
        translated["messages"][1]["content"],
        json!([
            {"type":"text","text":"Checking now"},
            {"type":"tool_use","id":"call-a","name":"weather","input":{"city":"Paris"}},
            {"type":"tool_use","id":"call-b","name":"clock","input":{}},
        ])
    );
    assert_eq!(translated["messages"][2]["role"], "user");
    assert_eq!(translated["messages"][2]["content"][0]["tool_use_id"], "call-b");
    assert_eq!(translated["messages"][2]["content"][1]["tool_use_id"], "call-a");
}

#[test]
fn maps_all_string_tool_choices_and_parallel_default() {
    for (choice, expected) in [("auto", "auto"), ("required", "any"), ("none", "none")] {
        let mut request = base_request();
        request["tools"] = json!([{
            "type": "function",
            "function": {"name": "lookup", "parameters": {}},
        }]);
        request["tool_choice"] = json!(choice);
        assert_eq!(body(&prepare(request).unwrap())["tool_choice"]["type"], expected);
    }

    let mut request = base_request();
    request["tools"] = json!([{
        "type": "function",
        "function": {"name": "lookup", "parameters": {}},
    }]);
    request["parallel_tool_calls"] = json!(false);
    assert_eq!(
        body(&prepare(request).unwrap())["tool_choice"],
        json!({"type": "auto", "disable_parallel_tool_use": true})
    );

    let mut request = base_request();
    request["tools"] = json!([{
        "type": "function",
        "function": {"name": "lookup", "parameters": {}},
    }]);
    request["tool_choice"] = json!("required");
    request["parallel_tool_calls"] = json!(false);
    assert_eq!(
        body(&prepare(request).unwrap())["tool_choice"],
        json!({"type": "any", "disable_parallel_tool_use": true})
    );

    let mut request = base_request();
    request["tools"] = json!([{
        "type": "function",
        "function": {"name": "lookup", "parameters": {}},
    }]);
    request["tool_choice"] = json!("none");
    request["parallel_tool_calls"] = json!(false);
    assert_eq!(body(&prepare(request).unwrap())["tool_choice"], json!({"type": "none"}));

    let mut request = base_request();
    request["tools"] = json!([{
        "type": "function",
        "function": {"name": "lookup", "parameters": {}},
    }]);
    request["parallel_tool_calls"] = json!(true);
    assert!(body(&prepare(request).unwrap()).get("tool_choice").is_none());

    let mut request = base_request();
    request["tools"] = json!([{
        "type": "function",
        "function": {"name": "lookup", "parameters": {}},
    }]);
    request["tool_choice"] = json!("auto");
    request["parallel_tool_calls"] = json!(true);
    assert_eq!(body(&prepare(request).unwrap())["tool_choice"], json!({"type":"auto"}));
}

#[test]
fn rejects_unknown_top_level_fields_even_when_null() {
    for (field, value) in [
        ("metadata", json!({})),
        ("response_format", Value::Null),
        ("unknown", json!(1)),
    ] {
        let mut request = base_request();
        request[field] = value;
        assert_eq!(prepare(request), Err(AnthropicRequestError::UnsupportedParameter));
    }
}

#[test]
fn validates_default_compatibility_fields() {
    let mut accepted = base_request();
    accepted["n"] = json!(1);
    accepted["temperature"] = json!(1.0);
    accepted["top_p"] = json!(1);
    assert!(prepare(accepted).is_ok());

    for (field, value, expected) in [
        ("n", json!(2), AnthropicRequestError::UnsupportedParameter),
        ("temperature", json!(0.5), AnthropicRequestError::UnsupportedParameter),
        ("top_p", json!(0), AnthropicRequestError::UnsupportedParameter),
        ("n", json!(1.0), AnthropicRequestError::InvalidRequestBody),
        ("temperature", json!("1"), AnthropicRequestError::InvalidRequestBody),
    ] {
        let mut request = base_request();
        request[field] = value;
        assert_eq!(prepare(request), Err(expected));
    }
}

#[test]
fn rejects_non_contiguous_system_unsupported_roles_and_parts() {
    let cases = [
        json!([{"role": "user", "content": "Hi"}, {"role": "system", "content": "Late"}]),
        json!([{"role": "developer", "content": "Instruction"}, {"role": "user", "content": "Hi"}]),
        json!([{"role": "user", "content": [{"type": "image_url", "image_url": {"url": "x"}}]}]),
    ];
    for messages in cases {
        let mut request = base_request();
        request["messages"] = messages;
        assert_eq!(prepare(request), Err(AnthropicRequestError::UnsupportedParameter));
    }
}

#[test]
fn rejects_message_shape_and_final_assistant() {
    let cases = [
        json!([{"role": "user", "content": "Hi", "name": "x"}]),
        json!([{"role": "user", "content": ""}]),
        json!([{"role": "assistant", "content": "Prefill"}]),
        json!([{"role": "system", "content": "Only"}]),
    ];
    for messages in cases {
        let mut request = base_request();
        request["messages"] = messages;
        assert!(prepare(request).is_err());
    }
}

#[test]
fn rejects_invalid_tool_definitions_and_choices() {
    let invalid_tools = [
        json!([]),
        json!([{"type": "custom", "function": {"name": "x", "parameters": {}}}]),
        json!([{"type": "function", "function": {"name": "bad name", "parameters": {}}}]),
        json!([{"type": "function", "function": {"name": "x", "parameters": [], "strict": true}}]),
        json!([
            {"type": "function", "function": {"name": "same", "parameters": {}}},
            {"type": "function", "function": {"name": "same", "parameters": {}}}
        ]),
    ];
    for tools in invalid_tools {
        let mut request = base_request();
        request["tools"] = tools;
        assert!(prepare(request).is_err());
    }

    let mut invalid_parameters = base_request();
    invalid_parameters["tools"] = json!([{"type":"function","function":{"name":"x","parameters":[]}}]);
    assert_eq!(
        prepare(invalid_parameters),
        Err(AnthropicRequestError::InvalidRequestBody)
    );

    let mut strict_tool = base_request();
    strict_tool["tools"] = json!([{"type":"function","function":{"name":"x","parameters":{},"strict":true}}]);
    assert_eq!(prepare(strict_tool), Err(AnthropicRequestError::UnsupportedParameter));

    for (field, value) in [("tool_choice", json!("auto")), ("parallel_tool_calls", json!(true))] {
        let mut request = base_request();
        request[field] = value;
        assert_eq!(prepare(request), Err(AnthropicRequestError::UnsupportedParameter));
    }
}

#[test]
fn validates_tool_conversation_graph() {
    let tools = json!([{"type": "function", "function": {"name": "lookup", "parameters": {}}}]);
    let cases = [
        json!([{"role": "tool", "tool_call_id": "call", "content": "orphan"}]),
        json!([
            {"role": "user", "content": "Hi"},
            {"role": "assistant", "tool_calls": [{"id": "call", "type": "function", "function": {"name": "lookup", "arguments": "{}"}}]},
            {"role": "user", "content": "not immediate"}
        ]),
        json!([
            {"role": "user", "content": "Hi"},
            {"role": "assistant", "tool_calls": [{"id": "call", "type": "function", "function": {"name": "lookup", "arguments": "[]"}}]},
            {"role": "tool", "tool_call_id": "call", "content": "result"}
        ]),
        json!([
            {"role": "user", "content": "Hi"},
            {"role": "assistant", "tool_calls": [{"id": "call", "type": "function", "function": {"name": "missing", "arguments": "{}"}}]},
            {"role": "tool", "tool_call_id": "call", "content": "result"}
        ]),
    ];
    for messages in cases {
        let mut request = base_request();
        request["tools"] = tools.clone();
        request["messages"] = messages;
        assert_eq!(prepare(request), Err(AnthropicRequestError::InvalidRequestBody));
    }
}

#[test]
fn enforces_tool_identifier_bounds_and_uniqueness() {
    let overlong_id = "x".repeat(MAX_TOOL_CALL_ID_BYTES + 1);
    let tools = json!([{"type": "function", "function": {"name": "lookup", "parameters": {}}}]);
    let cases = [
        json!([
            {"role": "user", "content": "Hi"},
            {"role": "assistant", "tool_calls": [{"id": overlong_id, "type": "function", "function": {"name": "lookup", "arguments": "{}"}}]},
            {"role": "tool", "tool_call_id": "unused", "content": "result"}
        ]),
        json!([
            {"role": "user", "content": "Hi"},
            {"role": "assistant", "tool_calls": [
                {"id": "same", "type": "function", "function": {"name": "lookup", "arguments": "{}"}},
                {"id": "same", "type": "function", "function": {"name": "lookup", "arguments": "{}"}}
            ]},
            {"role": "tool", "tool_call_id": "same", "content": "result"}
        ]),
    ];
    for messages in cases {
        let mut request = base_request();
        request["tools"] = tools.clone();
        request["messages"] = messages;
        assert_eq!(prepare(request), Err(AnthropicRequestError::InvalidRequestBody));
    }
}

#[test]
fn bounded_serialization_accepts_exact_limit_and_rejects_one_less() {
    let prepared = prepare(base_request()).unwrap();
    let exact = prepare_anthropic_request(base_request(), "claude-canonical", None, false, prepared.body.len());
    assert!(exact.is_ok());
    let too_small = prepare_anthropic_request(base_request(), "claude-canonical", None, false, prepared.body.len() - 1);
    assert_eq!(too_small, Err(AnthropicRequestError::OutputTooLarge));
}

#[test]
fn tool_ids_remain_unique_across_completed_rounds() {
    let mut request = base_request();
    request["tools"] = json!([{"type":"function","function":{"name":"lookup","parameters":{}}}]);
    request["messages"] = json!([
        {"role":"user","content":"Hi"},
        {"role":"assistant","tool_calls":[
            {"id":"first","type":"function","function":{"name":"lookup","arguments":"{}"}}
        ]},
        {"role":"tool","tool_call_id":"first","content":"one"},
        {"role":"assistant","tool_calls":[
            {"id":"second","type":"function","function":{"name":"lookup","arguments":"{}"}},
            {"id":"third","type":"function","function":{"name":"lookup","arguments":"{}"}}
        ]},
        {"role":"tool","tool_call_id":"third","content":"three"},
        {"role":"tool","tool_call_id":"second","content":"two"}
    ]);
    let expected = json!({
        "model":"claude-canonical","max_tokens":64,"stream":false,
        "tools":[{"name":"lookup","input_schema":{}}],
        "messages":[
            {"role":"user","content":[{"type":"text","text":"Hi"}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"first","name":"lookup","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"first","content":[{"type":"text","text":"one"}]}]},
            {"role":"assistant","content":[
                {"type":"tool_use","id":"second","name":"lookup","input":{}},
                {"type":"tool_use","id":"third","name":"lookup","input":{}}
            ]},
            {"role":"user","content":[
                {"type":"tool_result","tool_use_id":"third","content":[{"type":"text","text":"three"}]},
                {"type":"tool_result","tool_use_id":"second","content":[{"type":"text","text":"two"}]}
            ]}
        ]
    });
    assert_eq!(
        prepare(request.clone()).unwrap().body,
        serde_json::to_vec(&expected).unwrap()
    );

    let mut repeated = request.clone();
    repeated["messages"][3]["tool_calls"][0]["id"] = json!("first");
    // Duplicate validation precedes the unsupported call-type check.
    repeated["messages"][3]["tool_calls"][0]["type"] = json!("custom");
    assert_eq!(prepare(repeated), Err(AnthropicRequestError::InvalidRequestBody));

    for id in ["missing", "third", "first"] {
        let mut invalid_result = request.clone();
        invalid_result["messages"][5]["tool_call_id"] = json!(id);
        assert_eq!(prepare(invalid_result), Err(AnthropicRequestError::InvalidRequestBody));
    }
}
