#!/usr/bin/env python3
"""Local Messages SDK conformance; never contacts an external provider.

SDK helpers: https://github.com/anthropics/anthropic-sdk-python/blob/main/helpers.md
Set EDGION_MESSAGES_SDK_PYTHON to an interpreter with the official SDK installed.
"""
import argparse
import importlib.metadata
import json
import time
from pathlib import Path

import anthropic
import httpx

if not __debug__:
    raise RuntimeError("SDK conformance requires Python assertions enabled")

parser = argparse.ArgumentParser()
parser.add_argument("--ports", type=Path, required=True)
args = parser.parse_args()
ports = json.loads(args.ports.read_text())
port = ports["suites"]["EdgionBackend/Messages"]["http"]
SDK_VERSION = "0.125.0"
assert importlib.metadata.version("anthropic") == SDK_VERSION, "install anthropic==0.125.0"
kwargs = {"model": "fixture-model", "max_tokens": 16,
          "messages": [{"role": "user", "content": "hello"}],
          "tools": [{"name":"weather","description":"Fixture tool","input_schema":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}]}
for observation in ("off", "provider"):
    with anthropic.Anthropic(
        api_key="caller-key", base_url=f"http://127.0.0.1:{port}",
        default_headers={"host": f"messages-{observation}.example.com"},
        max_retries=0, http_client=httpx.Client(trust_env=False, follow_redirects=False, timeout=10),
    ) as client:
        cached = client.messages.create(**kwargs, system=[{"type":"text","text":"cached system","cache_control":{"type":"ephemeral"}}], extra_headers={"anthropic-beta":"fixture-beta"})
        observed = json.loads(cached.content[0].text)
        assert observed["request"]["system"][0]["cache_control"]["type"] == "ephemeral"
        assert observed["version"] == "2023-06-01" and observed["beta"] == ["fixture-beta"]
        for mode in ("normal", "tool", "thinking"):
            extra = {"extra_query": {"mock_mode": mode, "fixture_query": "sdk"}}
            message = client.messages.create(**kwargs, **extra)
            assert message.type == "message"
            assert message.usage.input_tokens == 8 and message.usage.output_tokens == 7
            with client.messages.stream(**kwargs, **extra) as stream:
                events = []
                for event in stream:
                    events.append(event)
                    time.sleep(0.005)
                final = stream.get_final_message()
            assert any(event.type == "message_stop" for event in events)
            assert final.type == "message"
            assert final.usage.input_tokens == 8
            assert final.usage.cache_creation_input_tokens == 3
            assert final.usage.cache_read_input_tokens == 2
            assert final.usage.output_tokens == 7
            if mode in ("tool", "thinking"):
                # Compare provider wire fields; SDK stream objects retain private parsing state.
                fields = {"type", "id", "name", "input", "thinking", "signature", "text"}
                assert [block.model_dump(include=fields) for block in message.content] == [
                    block.model_dump(include=fields) for block in final.content
                ]
            if mode == "tool":
                assert final.stop_reason == "tool_use"
                assert final.content[0].input == {"city": "Paris"}
                assert sum(event.type == "content_block_delta" and event.delta.type == "input_json_delta" for event in events) == 2
            elif mode == "thinking":
                assert final.content[0].type == "thinking"
                assert final.content[0].signature == "opaque-fixture-signature"
            else:
                assert json.loads(final.content[0].text)["credential_label"] == "A"
            if mode in ("tool", "thinking"):
                history = [{"role": "user", "content": "hello"},
                           {"role": "assistant", "content": [b.model_dump(exclude_none=True) for b in final.content]}]
                if mode == "tool":
                    history.append({"role": "user", "content": [{"type": "tool_result",
                                    "tool_use_id": final.content[0].id, "content": "sunny"}]})
                else:
                    history.append({"role": "user", "content": "continue"})
                replay = client.messages.create(**{**kwargs, "messages": history},
                                                extra_query={"mock_mode": "normal"},
                                                extra_body={"native_extension": {"keep": True}})
                observed = json.loads(replay.content[0].text)
                assert observed["request"]["messages"] == history
                assert observed["request"]["native_extension"] == {"keep": True}
                assert observed["credential_label"] == "A"
        zero = client.messages.create(**{**kwargs, "max_tokens": 0})
        assert zero.content == [] and zero.usage.output_tokens == 0
        assert zero.stop_reason == "max_tokens"
        try:
            client.messages.create(**{**kwargs, "max_tokens": -1})
            raise AssertionError("invalid native request was forwarded")
        except anthropic.BadRequestError as error:
            assert error.body["type"] == "error"
            assert error.body["error"]["type"] == "invalid_request_error"
        try:
            with client.messages.create(
                **{**kwargs, "max_tokens": 64}, stream=True,
                extra_query={"mock_mode": "quota"},
                extra_headers={"host":"messages-quota.example.com","x-quota-user":f"sdk-native-quota-{time.time_ns()}"},
            ) as stream:
                list(stream)
            raise AssertionError("native quota did not produce a stream error")
        except anthropic.APIError as error:
            assert error.body["type"] == "error"
            assert error.body["error"]["type"] == "rate_limit_error"
print(f"Messages SDK PASS: anthropic={importlib.metadata.version('anthropic')}")
