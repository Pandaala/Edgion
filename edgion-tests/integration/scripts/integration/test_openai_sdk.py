#!/usr/bin/env python3
"""Pinned official SDK contracts against local gateway/provider fixtures only."""
import argparse
import importlib.metadata
import json
import time
from pathlib import Path

import httpx
import openai

if not __debug__:
    raise RuntimeError("SDK conformance requires Python assertions enabled")


def client_for(port, host):
    return openai.OpenAI(
        api_key="caller-key", base_url=f"http://127.0.0.1:{port}/v1",
        default_headers={"host": host}, max_retries=0,
        http_client=httpx.Client(trust_env=False, follow_redirects=False, timeout=10),
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--ports", type=Path, required=True)
    args = parser.parse_args()
    ports = json.loads(args.ports.read_text())["suites"]
    assert importlib.metadata.version("openai") == "2.48.0", "install openai==2.48.0"
    tools = [{"type": "function", "function": {"name": "weather",
              "parameters": {"type": "object", "properties": {"city": {"type": "string"}},
                             "required": ["city"]}}}]
    schema = {"type": "json_schema", "json_schema": {"name": "result", "strict": True,
              "schema": {"type": "object", "properties": {"ok": {"type": "boolean"}},
                         "required": ["ok"], "additionalProperties": False}}}
    cells = 0
    for provider in ("compatible", "openai"):
        for observation in ("off", "provider"):
            with client_for(ports["EdgionBackend/SingleProvider"]["http"],
                            f"ai-chat-{provider}-{observation}.example.com") as client:
                for stream in (False, True):
                    for mode in ("normal", "tool", "structured"):
                        kwargs = dict(model="fixture-model", max_tokens=16,
                                      messages=[{"role": "user", "content": "hello"}],
                                      extra_query={"mock_mode": f"sdk_chat_{mode}", "fixture_query": "preserved"},
                                      extra_body={"native_extension": {"keep": True}})
                        if mode == "tool":
                            kwargs["tools"] = tools
                        if mode == "structured":
                            kwargs["response_format"] = schema
                        if stream:
                            # The client's explicit usage request is independent of gateway observation.
                            with client.chat.completions.stream(
                                **kwargs, stream_options={"include_usage": True}
                            ) as events:
                                types = []
                                assembled = None
                                for event in events:
                                    types.append(event.type)
                                    if event.type == "content.delta":
                                        assembled = event.snapshot
                                    elif event.type == "tool_calls.function.arguments.delta":
                                        assembled = event.arguments
                                    time.sleep(0.005)
                                completion = events.get_final_completion()
                            assert "chunk" in types
                        else:
                            completion = client.chat.completions.create(**kwargs)
                            assert completion.native_extension["request"]["native_extension"] == {"keep": True}
                            if mode == "structured":
                                assert completion.native_extension["request"]["response_format"] == schema
                            assert completion.mock["auth_is_provider_key"]
                            assert not completion.mock["auth_is_client_key"]
                        assert completion.usage.prompt_tokens == 11 and completion.usage.completion_tokens == 7
                        message = completion.choices[0].message
                        if stream:
                            if mode == "tool":
                                assert types.count("tool_calls.function.arguments.delta") == 2
                                assert assembled == message.tool_calls[0].function.arguments
                            else:
                                assert assembled == message.content
                        if mode == "tool":
                            call = message.tool_calls[0]
                            assert call.id == "call_fixture" and call.function.name == "weather"
                            assert json.loads(call.function.arguments) == {"city": "Paris"}
                            replay = [kwargs["messages"][0], message.model_dump(exclude_none=True),
                                      {"role": "tool", "tool_call_id": call.id, "content": "sunny"}]
                            echoed = client.chat.completions.create(
                                model="fixture-model", messages=replay,
                                extra_query={"mock_mode": "sdk_chat_normal"},
                            ).native_extension["request"]
                            assert echoed["messages"] == replay
                        elif mode == "structured":
                            assert json.loads(message.content) == {"ok": True}
                        else:
                            assert message.content == "fixed completion"
                    cells += 1
                try:
                    client.chat.completions.create(model="unknown-model", messages=[{"role": "user", "content": "hello"}])
                    raise AssertionError("unknown model was forwarded")
                except openai.NotFoundError as error:
                    assert error.code == "model_not_found"
    for observation in ("off", "provider"):
        with client_for(ports["EdgionBackend/Responses"]["http"],
                        f"responses-{observation}.example.com") as client:
            for stream in (False, True):
                for mode in ("normal", "tool", "structured"):
                    kwargs = dict(model="fixture-model", input="hello", max_output_tokens=16,
                                  extra_query={"mock_mode": f"sdk_{mode}", "fixture_query": "preserved"},
                                  extra_body={"native_extension": {"keep": True}})
                    if mode == "tool":
                        kwargs["tools"] = [{"type": "function", "name": "weather",
                                            "parameters": tools[0]["function"]["parameters"]}]
                    if mode == "structured":
                        kwargs["text"] = {"format": {"type": "json_schema", **schema["json_schema"]}}
                    if stream:
                        with client.responses.stream(**kwargs) as events:
                            types = []
                            sequences = []
                            assembled = None
                            for event in events:
                                types.append(event.type)
                                sequences.append(event.sequence_number)
                                if event.type in ("response.output_text.delta", "response.function_call_arguments.delta"):
                                    assembled = event.snapshot
                                time.sleep(0.005)
                            response = events.get_final_response()
                        assert sequences == list(range(len(sequences)))
                        assert assembled == (response.output[0].arguments if mode == "tool" else response.output_text)
                        assert types[0] == "response.created" and types[-1] == "response.completed"
                        assert "response.output_item.added" in types and "response.output_item.done" in types
                        if mode == "tool":
                            assert types.count("response.function_call_arguments.delta") == 2
                        else:
                            assert "response.content_part.added" in types and "response.content_part.done" in types
                    else:
                        response = client.responses.create(**kwargs)
                    assert response.status == "completed"
                    assert response.usage.input_tokens == 11 and response.usage.output_tokens == 7
                    assert response.native_extension["request"]["native_extension"] == {"keep": True}
                    if mode == "tool":
                        call = response.output[0]
                        assert call.call_id == "call_fixture" and call.name == "weather"
                        assert json.loads(call.arguments) == {"city": "Paris"}
                        replay = [call.model_dump(exclude_none=True),
                                  {"type": "function_call_output", "call_id": call.call_id, "output": "sunny"}]
                        echoed = client.responses.create(model="fixture-model", input=replay,
                            extra_query={"mock_mode": "sdk_normal"}).native_extension["request"]
                        assert echoed["input"] == replay
                    elif mode == "structured":
                        assert response.native_extension["request"]["text"] == kwargs["text"]
                        assert json.loads(response.output_text) == {"ok": True}
                    else:
                        assert json.loads(response.output_text)["auth_is_provider_key"]
                cells += 1
            try:
                client.responses.create(model="fixture-model", input="hello", background=True)
                raise AssertionError("unsupported background request was forwarded")
            except openai.BadRequestError as error:
                assert error.code == "unsupported_parameter"
    assert cells == 12
    print("OpenAI SDK PASS: openai=2.48.0; 12 native cells; finite delayed consumption")


if __name__ == "__main__":
    main()
