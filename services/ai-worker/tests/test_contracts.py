import pytest
from pydantic import SecretStr, ValidationError

from hpa_ai_worker.contracts import StartSessionRequest


def test_unknown_request_fields_are_rejected() -> None:
    with pytest.raises(ValidationError):
        StartSessionRequest.model_validate(
            {
                "type": "start",
                "sessionId": "session-1",
                "requestToken": "secret",
                "tenantId": "tenant-1",
                "agentVersionId": "agent-1",
                "input": {},
                "unexpected": True,
            }
        )


def test_secret_is_not_in_plain_representation() -> None:
    request = StartSessionRequest(
        type="start",
        sessionId="session-1",
        requestToken=SecretStr("worker-token"),
        tenantId="tenant-1",
        agentVersionId="agent-1",
        input={},
    )
    assert "worker-token" not in repr(request)
    assert "worker-token" not in request.model_dump_json()
