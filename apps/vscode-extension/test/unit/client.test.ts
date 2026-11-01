import { describe, expect, it, vi } from "vitest";
import { RuntimeApiError, RuntimeClient } from "../../src/runtime/client";

type FetchHandler = (url: string, init: RequestInit) => Response | Promise<Response>;

function installFetch(handler: FetchHandler) {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const fetchImpl = vi.fn(
    async (input: Parameters<typeof fetch>[0], init?: Parameters<typeof fetch>[1]) => {
      const url =
        typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      const requestInit = init ?? {};
      calls.push({ url, init: requestInit });
      return handler(url, requestInit);
    },
  );
  return { calls, fetchImpl: fetchImpl as unknown as typeof fetch };
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function createClient(fetchImpl: typeof fetch, token?: string): RuntimeClient {
  return new RuntimeClient({
    baseUrl: "http://127.0.0.1:8787/",
    tokenProvider: token ? () => token : undefined,
    fetchImpl,
  });
}

describe("RuntimeClient authentication", () => {
  it("sends a bearer token when the token provider returns one", async () => {
    const { calls, fetchImpl } = installFetch(() =>
      jsonResponse({ status: "ok", internalId: "hpa", version: "0.1.0", runtime: "rust" }),
    );
    const client = createClient(fetchImpl, "secret-token");

    await client.health();

    const call = calls[0];
    expect(call).toBeDefined();
    expect(call?.url).toBe("http://127.0.0.1:8787/api/health");
    const headers = new Headers(call?.init.headers);
    expect(headers.get("authorization")).toBe("Bearer secret-token");
    expect(headers.get("accept")).toBe("application/json");
  });

  it("omits the authorization header when no token is available", async () => {
    const { calls, fetchImpl } = installFetch(() => jsonResponse({ workflows: 0 }));
    const client = createClient(fetchImpl);

    await client.stats();

    const headers = new Headers(calls[0]?.init.headers);
    expect(headers.has("authorization")).toBe(false);
  });
});

describe("RuntimeClient responses", () => {
  it("returns parsed JSON payloads", async () => {
    const payload = { workflows: 2, executions: 3, running: 1, waitingApproval: 0 };
    const { fetchImpl } = installFetch(() => jsonResponse(payload));
    const client = createClient(fetchImpl);

    await expect(client.stats()).resolves.toEqual(payload);
  });

  it("unwraps listSkills and listAgents envelopes", async () => {
    const agent = {
      id: "agent-1",
      name: "Agent",
      latestVersion: "1.0.0",
      status: "published",
      createdAt: "2026-01-01T00:00:00Z",
      updatedAt: "2026-01-01T00:00:00Z",
    };
    const skill = {
      id: "skill-1",
      name: "Skill",
      latestVersion: "2.0.0",
      license: "MIT",
      status: "published",
      updatedAt: "2026-01-01T00:00:00Z",
    };
    const handler: FetchHandler = (url) => {
      if (url.endsWith("/api/agents")) return jsonResponse({ agents: [agent] });
      if (url.endsWith("/api/skills")) return jsonResponse({ skills: [skill] });
      return jsonResponse({}, 404);
    };
    const { fetchImpl } = installFetch(handler);
    const client = createClient(fetchImpl);

    await expect(client.listAgents()).resolves.toEqual([agent]);
    await expect(client.listSkills()).resolves.toEqual([skill]);
  });

  it("rejects with a structured RuntimeApiError", async () => {
    const { fetchImpl } = installFetch(() =>
      jsonResponse(
        {
          error: {
            code: "WORKFLOW_INVALID",
            message: "workflow input is invalid",
            details: { field: "topic" },
          },
        },
        422,
      ),
    );
    const client = createClient(fetchImpl);

    const error = await client
      .runWorkflow("wf-1", {})
      .catch((cause: unknown) => cause);

    expect(error).toBeInstanceOf(RuntimeApiError);
    expect(error).toMatchObject({
      status: 422,
      code: "WORKFLOW_INVALID",
      message: "workflow input is invalid",
      details: { field: "topic" },
    });
  });

  it("maps plain 401 responses to UNAUTHORIZED", async () => {
    const { fetchImpl } = installFetch(() => new Response("token required", { status: 401 }));
    const client = createClient(fetchImpl);

    const error = await client.stats().catch((cause: unknown) => cause);

    expect(error).toBeInstanceOf(RuntimeApiError);
    expect(error).toMatchObject({
      status: 401,
      code: "UNAUTHORIZED",
      message: "token required",
    });
  });

  it("maps fetch failures to status 0 and NETWORK", async () => {
    const { fetchImpl } = installFetch(() => {
      throw new TypeError("fetch failed");
    });
    const client = createClient(fetchImpl);

    const error = await client.health().catch((cause: unknown) => cause);

    expect(error).toBeInstanceOf(RuntimeApiError);
    expect(error).toMatchObject({
      status: 0,
      code: "NETWORK",
      message: "fetch failed",
    });
  });

  it("maps invalid JSON on a successful response to INVALID_JSON", async () => {
    const { fetchImpl } = installFetch(
      () => new Response("not json", { status: 200, headers: { "content-type": "text/plain" } }),
    );
    const client = createClient(fetchImpl);

    const error = await client.stats().catch((cause: unknown) => cause);

    expect(error).toBeInstanceOf(RuntimeApiError);
    expect(error).toMatchObject({ status: 200, code: "INVALID_JSON" });
  });
});

describe("RuntimeClient request construction", () => {
  it("builds runWorkflow requests", async () => {
    const { calls, fetchImpl } = installFetch(() => jsonResponse({ summary: {}, state: {} }));
    const client = createClient(fetchImpl);

    await client.runWorkflow("wf/one", { topic: "release" }, "v3");

    const call = calls[0];
    expect(call?.url).toBe("http://127.0.0.1:8787/api/workflows/wf%2Fone/executions");
    expect(call?.init.method).toBe("POST");
    expect(new Headers(call?.init.headers).get("content-type")).toBe("application/json");
    expect(JSON.parse(String(call?.init.body))).toEqual({
      input: { topic: "release" },
      version: "v3",
    });
  });

  it("builds decideApproval requests", async () => {
    const { calls, fetchImpl } = installFetch(() => jsonResponse({ summary: {}, state: {} }));
    const client = createClient(fetchImpl);

    await client.decideApproval("exec 1", "approval-1", "approved", { comment: "ship it" });

    const call = calls[0];
    expect(call?.url).toBe(
      "http://127.0.0.1:8787/api/executions/exec%201/approvals",
    );
    expect(call?.init.method).toBe("POST");
    expect(JSON.parse(String(call?.init.body))).toEqual({
      nodeId: "approval-1",
      decision: "approved",
      form: { comment: "ship it" },
    });
  });

  it("builds exportSkill requests", async () => {
    const { calls, fetchImpl } = installFetch(() =>
      jsonResponse({ exported: true, path: "/tmp/skills/demo" }),
    );
    const client = createClient(fetchImpl);

    await client.exportSkill("skill 1", "1.2.3", "/tmp/skills/demo");

    const call = calls[0];
    expect(call?.url).toBe(
      "http://127.0.0.1:8787/api/skills/skill%201/versions/1.2.3/export",
    );
    expect(call?.init.method).toBe("POST");
    expect(JSON.parse(String(call?.init.body))).toEqual({ path: "/tmp/skills/demo" });
  });
});

describe("RuntimeClient streaming", () => {
  it("parses multiple SSE events and ignores event and id lines", async () => {
    const payload =
      "event: execution\nid: 1\ndata: {\"id\":\"evt-1\",\"sequence\":1}\n\n" +
      ": keep-alive\n\n" +
      "event: execution\nid: 2\ndata: {\"id\":\"evt-2\",\"sequence\":2}\n\n";
    const { calls, fetchImpl } = installFetch(
      () =>
        new Response(payload, {
          status: 200,
          headers: { "content-type": "text/event-stream" },
        }),
    );
    const client = createClient(fetchImpl, "stream-token");
    const events: Array<{ id: string; sequence: number }> = [];

    await client.streamEvents("exec 1", (event) => {
      events.push(event as unknown as { id: string; sequence: number });
    });

    expect(events.map((event) => event.id)).toEqual(["evt-1", "evt-2"]);
    expect(events.map((event) => event.sequence)).toEqual([1, 2]);
    const call = calls[0];
    expect(call?.url).toBe("http://127.0.0.1:8787/api/executions/exec%201/events/stream");
    const headers = new Headers(call?.init.headers);
    expect(headers.get("accept")).toBe("text/event-stream");
    expect(headers.get("authorization")).toBe("Bearer stream-token");
  });

  it("rejects non-OK streaming responses", async () => {
    const { fetchImpl } = installFetch(() => new Response("stream unavailable", { status: 503 }));
    const client = createClient(fetchImpl);

    const error = await client
      .streamEvents("exec-1", () => undefined)
      .catch((cause: unknown) => cause);

    expect(error).toBeInstanceOf(RuntimeApiError);
    expect(error).toMatchObject({ status: 503, code: "HTTP_ERROR" });
  });
});
