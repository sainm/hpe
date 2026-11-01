import * as vscode from "vscode";
import type { HpaServices } from "../contracts";

export const MCP_PROVIDER_ID = "hpa-local";

export interface CopilotIntegration {
  register(): vscode.Disposable[];
  refresh(): void;
  isSupported(): boolean;
}

export function createCopilotIntegration(
  context: vscode.ExtensionContext,
  services: HpaServices,
): CopilotIntegration {
  const emitter = new vscode.EventEmitter<void>();
  const supported = typeof vscode.lm?.registerMcpServerDefinitionProvider === "function";
  let registration: vscode.Disposable[] | undefined;

  const provider: vscode.McpServerDefinitionProvider<vscode.McpHttpServerDefinition> = {
    onDidChangeMcpServerDefinitions: emitter.event,
    provideMcpServerDefinitions() {
      const state = services.runtime();
      const endpoint = services.mcpEndpoint();
      if (state.status !== "online" || endpoint === "") return [];
      return [
        new vscode.McpHttpServerDefinition(
          `${services.brandName()} Local MCP`,
          vscode.Uri.parse(endpoint),
          { "X-HPA-Tool-Name-Style": "slug" },
          state.health?.version,
        ),
      ];
    },
    async resolveMcpServerDefinition(server) {
      try {
        const token = await services.mcpToken();
        if (token === "") throw new Error("empty token");
        server.headers = {
          Authorization: `Bearer ${token}`,
          "X-HPA-Tool-Name-Style": "slug",
        };
        return server;
      } catch {
        await services.resetMcpToken().catch(() => undefined);
        throw new Error(services.i18n.t("mcp.tokenFailed"));
      }
    },
  };

  const register = (): vscode.Disposable[] => {
    if (!supported) return [];
    if (registration) return registration;
    registration = [
      vscode.lm.registerMcpServerDefinitionProvider(MCP_PROVIDER_ID, provider),
      services.onDidChangeRuntime(() => emitter.fire()),
      vscode.workspace.onDidChangeConfiguration((event) => {
        if (event.affectsConfiguration("hpa")) emitter.fire();
      }),
      emitter,
    ];
    return registration;
  };

  return {
    register,
    refresh() {
      if (supported) emitter.fire();
    },
    isSupported() {
      return supported;
    },
  };
}
