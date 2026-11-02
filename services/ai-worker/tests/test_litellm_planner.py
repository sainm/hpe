from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from hpa_ai_worker.planner import LiteLlmPlanner


class _OpenAICompatibleHandler(BaseHTTPRequestHandler):
    requests: list[dict[str, object]] = []

    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("content-length", "0"))
        request = json.loads(self.rfile.read(length))
        self.__class__.requests.append(request)
        response = {
            "id": "chatcmpl_test",
            "object": "chat.completion",
            "created": 1_793_550_000,
            "model": "hpa-test-model",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": '{"action":"finish","output":{"ok":true}}',
                    },
                    "finish_reason": "stop",
                }
            ],
            "usage": {"prompt_tokens": 5, "completion_tokens": 7, "total_tokens": 12},
        }
        body = json.dumps(response).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        del format, args


class _FallbackHandler(BaseHTTPRequestHandler):
    models: list[str] = []

    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("content-length", "0"))
        request = json.loads(self.rfile.read(length))
        model = request["model"]
        self.__class__.models.append(model)
        if model == "unavailable-model":
            body = json.dumps(
                {"error": {"message": "controlled provider failure", "type": "server_error"}}
            ).encode()
            self.send_response(503)
        else:
            body = json.dumps(
                {
                    "id": "chatcmpl_fallback",
                    "object": "chat.completion",
                    "created": 1_793_550_000,
                    "model": model,
                    "choices": [
                        {
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": '{"action":"finish","output":{"fallback":true}}',
                            },
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {
                        "prompt_tokens": 4,
                        "completion_tokens": 6,
                        "total_tokens": 10,
                    },
                }
            ).encode()
            self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        del format, args


async def test_litellm_openai_compatible_adapter(monkeypatch) -> None:
    server = ThreadingHTTPServer(("127.0.0.1", 0), _OpenAICompatibleHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    monkeypatch.setenv("HPA_MODEL_API_KEY", "controlled-test-key")
    try:
        planner = LiteLlmPlanner(
            ["openai/hpa-test-model"],
            base_url=f"http://127.0.0.1:{server.server_port}/v1",
        )
        decision = await planner.decide(
            input_data={"message": "finish"},
            history=[],
            tools=[],
            model="openai/hpa-test-model",
        )
    finally:
        server.shutdown()
        thread.join(timeout=5)
    assert decision.action == "finish"
    assert decision.output == {"ok": True}
    assert decision.usage.prompt_tokens == 5
    assert decision.usage.completion_tokens == 7
    assert _OpenAICompatibleHandler.requests
    assert _OpenAICompatibleHandler.requests[0]["model"] == "hpa-test-model"


async def test_litellm_model_fallback(monkeypatch) -> None:
    _FallbackHandler.models = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), _FallbackHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    monkeypatch.setenv("HPA_MODEL_API_KEY", "controlled-test-key")
    try:
        planner = LiteLlmPlanner(
            ["openai/unavailable-model", "openai/good-model"],
            base_url=f"http://127.0.0.1:{server.server_port}/v1",
        )
        decision = await planner.decide(
            input_data={"message": "finish"},
            history=[],
            tools=[],
            model=None,
        )
    finally:
        server.shutdown()
        thread.join(timeout=5)
    assert decision.output == {"fallback": True}
    assert _FallbackHandler.models[0] == "unavailable-model"
    assert "good-model" in _FallbackHandler.models
