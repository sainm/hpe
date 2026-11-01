import { describe, expect, it } from "vitest";
import {
  deriveMcpEndpoint,
  isLoopbackHost,
  normalizeArgs,
  normalizeEndpoint,
  normalizeLocale,
  normalizeMode,
  resolveEndpoints,
  validateEndpoint,
  type HpaConfig,
} from "../../src/config";

function config(overrides: Partial<HpaConfig> = {}): HpaConfig {
  return {
    mode: "auto",
    localEndpoint: "http://127.0.0.1:8787",
    mcpEndpoint: "",
    remoteEndpoint: "",
    remoteMcpEndpoint: "",
    locale: "auto",
    skillTarget: "project",
    personalSkillsPath: "",
    binary: "",
    startArgs: ["serve"],
    ...overrides,
  };
}

describe("normalizeEndpoint", () => {
  it("trims whitespace and trailing slashes", () => {
    expect(normalizeEndpoint("  http://127.0.0.1:8787///  ")).toBe("http://127.0.0.1:8787");
    expect(normalizeEndpoint("https://example.com/")).toBe("https://example.com");
  });

  it("returns an empty string for missing values", () => {
    expect(normalizeEndpoint(undefined)).toBe("");
    expect(normalizeEndpoint("")).toBe("");
    expect(normalizeEndpoint("   ")).toBe("");
  });
});

describe("normalizeArgs", () => {
  it("defaults to serve", () => {
    expect(normalizeArgs(undefined)).toEqual(["serve"]);
    expect(normalizeArgs(null as unknown as string[])).toEqual(["serve"]);
  });

  it("filters blank and non-string entries", () => {
    expect(normalizeArgs(["serve", "", "  ", 42 as unknown as string])).toEqual(["serve"]);
    expect(normalizeArgs(["serve", " --port ", "8787"])).toEqual([
      "serve",
      " --port ",
      "8787",
    ]);
  });

  it("falls back to serve when no usable arguments remain", () => {
    expect(normalizeArgs([])).toEqual(["serve"]);
    expect(normalizeArgs(["", " "])).toEqual(["serve"]);
  });
});

describe("normalizeMode", () => {
  it("accepts local and remote", () => {
    expect(normalizeMode("local")).toBe("local");
    expect(normalizeMode("remote")).toBe("remote");
  });

  it("falls back to auto for unknown values", () => {
    expect(normalizeMode("auto")).toBe("auto");
    expect(normalizeMode("AUTO")).toBe("auto");
    expect(normalizeMode("cloud")).toBe("auto");
    expect(normalizeMode(undefined)).toBe("auto");
  });
});

describe("normalizeLocale", () => {
  it("accepts supported locales", () => {
    expect(normalizeLocale("ja")).toBe("ja");
    expect(normalizeLocale("en")).toBe("en");
    expect(normalizeLocale("zh")).toBe("zh");
  });

  it("falls back to auto for unsupported values", () => {
    expect(normalizeLocale("auto")).toBe("auto");
    expect(normalizeLocale("zh-cn")).toBe("auto");
    expect(normalizeLocale("fr")).toBe("auto");
    expect(normalizeLocale(undefined)).toBe("auto");
  });
});

describe("deriveMcpEndpoint", () => {
  it("derives the MCP endpoint from the URL origin", () => {
    expect(deriveMcpEndpoint("http://127.0.0.1:8787")).toBe("http://127.0.0.1:8787/mcp");
    expect(deriveMcpEndpoint("https://api.example.com/v1/executions?x=1")).toBe(
      "https://api.example.com/mcp",
    );
  });

  it("returns an empty string for invalid URLs", () => {
    expect(deriveMcpEndpoint("")).toBe("");
    expect(deriveMcpEndpoint("not a url")).toBe("");
  });
});

describe("validateEndpoint", () => {
  it("accepts loopback HTTP endpoints for local mode", () => {
    expect(validateEndpoint("http://127.0.0.1:8787", false)).toBeUndefined();
    expect(validateEndpoint("http://localhost:8787", false)).toBeUndefined();
    expect(validateEndpoint("http://[::1]:8787", false)).toBeUndefined();
    expect(validateEndpoint("https://127.0.0.1:8787", false)).toBeUndefined();
  });

  it("accepts HTTPS endpoints for remote mode", () => {
    expect(validateEndpoint("https://runtime.example.com", true)).toBeUndefined();
    expect(validateEndpoint("https://runtime.example.com/api/v1", true)).toBeUndefined();
  });

  it("rejects remote HTTP endpoints", () => {
    expect(validateEndpoint("http://runtime.example.com", true)).toBe("remote-requires-https");
    expect(validateEndpoint("http://127.0.0.1:8787", true)).toBe("remote-requires-https");
  });

  it("rejects non-loopback hosts for local mode", () => {
    expect(validateEndpoint("http://192.168.1.10:8787", false)).toBe("local-requires-loopback");
    expect(validateEndpoint("http://runtime.example.com", false)).toBe("local-requires-loopback");
    expect(validateEndpoint("http://0.0.0.0:8787", false)).toBe("local-requires-loopback");
  });

  it("rejects userinfo and invalid URLs", () => {
    expect(validateEndpoint("http://user:secret@127.0.0.1:8787", false)).toBe(
      "endpoint-userinfo",
    );
    expect(validateEndpoint("https://token@runtime.example.com", true)).toBe(
      "endpoint-userinfo",
    );
    expect(validateEndpoint("not a url", false)).toBe("endpoint-invalid");
    expect(validateEndpoint("ftp://127.0.0.1:8787", false)).toBe("endpoint-invalid");
    expect(validateEndpoint("", false)).toBe("endpoint-invalid");
  });
});

describe("isLoopbackHost", () => {
  it("recognizes loopback names and addresses", () => {
    expect(isLoopbackHost("localhost")).toBe(true);
    expect(isLoopbackHost("LOCALHOST")).toBe(true);
    expect(isLoopbackHost("127.0.0.1")).toBe(true);
    expect(isLoopbackHost("127.42.0.9")).toBe(true);
    expect(isLoopbackHost("::1")).toBe(true);
    expect(isLoopbackHost("[::1]")).toBe(true);
  });

  it("rejects wildcard, LAN, and lookalike hosts", () => {
    expect(isLoopbackHost("0.0.0.0")).toBe(false);
    expect(isLoopbackHost("192.168.1.1")).toBe(false);
    expect(isLoopbackHost("localhost.localdomain")).toBe(false);
    expect(isLoopbackHost("127.0.0.1.example.com")).toBe(false);
    expect(isLoopbackHost("128.0.0.1")).toBe(false);
    expect(isLoopbackHost("")).toBe(false);
  });
});

describe("resolveEndpoints", () => {
  it("prefers the local endpoint in auto mode", () => {
    expect(resolveEndpoints(config())).toEqual({
      apiEndpoint: "http://127.0.0.1:8787",
      mcpEndpoint: "http://127.0.0.1:8787/mcp",
      isRemote: false,
      problem: undefined,
    });
  });

  it("uses the remote endpoint in remote mode", () => {
    expect(
      resolveEndpoints(
        config({
          mode: "remote",
          remoteEndpoint: "https://runtime.example.com",
        }),
      ),
    ).toEqual({
      apiEndpoint: "https://runtime.example.com",
      mcpEndpoint: "https://runtime.example.com/mcp",
      isRemote: true,
      problem: undefined,
    });
  });

  it("uses the remote endpoint in auto mode when no local endpoint is configured", () => {
    expect(
      resolveEndpoints(
        config({
          mode: "auto",
          localEndpoint: "",
          remoteEndpoint: "https://runtime.example.com",
        }),
      ),
    ).toEqual({
      apiEndpoint: "https://runtime.example.com",
      mcpEndpoint: "https://runtime.example.com/mcp",
      isRemote: true,
      problem: undefined,
    });
  });

  it("prefers the local endpoint in auto mode when both are configured", () => {
    const resolved = resolveEndpoints(
      config({
        mode: "auto",
        remoteEndpoint: "https://runtime.example.com",
      }),
    );
    expect(resolved.apiEndpoint).toBe("http://127.0.0.1:8787");
    expect(resolved.isRemote).toBe(false);
    expect(resolved.problem).toBeUndefined();
  });

  it("uses the local endpoint in local mode even when remote is configured", () => {
    const resolved = resolveEndpoints(
      config({
        mode: "local",
        remoteEndpoint: "https://runtime.example.com",
      }),
    );
    expect(resolved.apiEndpoint).toBe("http://127.0.0.1:8787");
    expect(resolved.isRemote).toBe(false);
  });

  it("reports endpoint-not-configured for missing endpoints", () => {
    expect(resolveEndpoints(config({ localEndpoint: "" }))).toEqual({
      apiEndpoint: "",
      mcpEndpoint: "",
      isRemote: false,
      problem: "endpoint-not-configured",
    });
    expect(
      resolveEndpoints(config({ mode: "remote", localEndpoint: "", remoteEndpoint: "" })),
    ).toEqual({
      apiEndpoint: "",
      mcpEndpoint: "",
      isRemote: true,
      problem: "endpoint-not-configured",
    });
  });

  it("derives the local MCP endpoint and honors overrides", () => {
    expect(resolveEndpoints(config()).mcpEndpoint).toBe("http://127.0.0.1:8787/mcp");
    expect(resolveEndpoints(config({ mcpEndpoint: "http://127.0.0.1:9999/mcp" })).mcpEndpoint).toBe(
      "http://127.0.0.1:9999/mcp",
    );
  });

  it("derives the remote MCP endpoint and honors overrides", () => {
    const base = config({
      mode: "remote",
      remoteEndpoint: "https://runtime.example.com/api/v1",
    });
    expect(resolveEndpoints(base).mcpEndpoint).toBe("https://runtime.example.com/mcp");
    expect(
      resolveEndpoints({ ...base, remoteMcpEndpoint: "https://mcp.example.com/mcp" }).mcpEndpoint,
    ).toBe("https://mcp.example.com/mcp");
  });

  it("reports invalid API endpoints and clears the MCP endpoint", () => {
    const remoteHttp = resolveEndpoints(
      config({ mode: "remote", remoteEndpoint: "http://runtime.example.com" }),
    );
    expect(remoteHttp.problem).toBe("remote-requires-https");
    expect(remoteHttp.mcpEndpoint).toBe("");

    const localPublic = resolveEndpoints(
      config({ localEndpoint: "http://runtime.example.com" }),
    );
    expect(localPublic.problem).toBe("local-requires-loopback");
    expect(localPublic.mcpEndpoint).toBe("");
  });

  it("reports invalid MCP endpoints and clears the MCP endpoint", () => {
    const resolved = resolveEndpoints(
      config({ mcpEndpoint: "http://192.168.1.10:9999/mcp" }),
    );
    expect(resolved.apiEndpoint).toBe("http://127.0.0.1:8787");
    expect(resolved.problem).toBe("local-requires-loopback");
    expect(resolved.mcpEndpoint).toBe("");
  });
});
