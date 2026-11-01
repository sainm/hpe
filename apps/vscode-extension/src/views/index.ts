import * as vscode from "vscode";
import type { HpaServices } from "../contracts";
import { AgentsProvider } from "./agents";
import { ArtifactsProvider } from "./artifacts";
import { ExecutionsProvider } from "./executions";
import { McpProvider } from "./mcp";
import { OverviewProvider } from "./overview";
import { PluginsProvider } from "./plugins";
import { RuntimeProvider } from "./runtime";
import { SettingsProvider } from "./settings";
import { SkillsProvider } from "./skills";
import { WorkflowsProvider } from "./workflows";
import type { ViewProvider } from "./common";

export { openExecutionPanel } from "../webviews/executionPanel";
export { openArtifactPanel } from "../webviews/artifactPanel";

export function registerViews(
  context: vscode.ExtensionContext,
  services: HpaServices,
): vscode.Disposable[] {
  void context;
  const registrations: [string, ViewProvider][] = [
    ["hpa.overview", new OverviewProvider(services)],
    ["hpa.workflows", new WorkflowsProvider(services)],
    ["hpa.agents", new AgentsProvider(services)],
    ["hpa.skills", new SkillsProvider(services)],
    ["hpa.mcp", new McpProvider(services)],
    ["hpa.plugins", new PluginsProvider(services)],
    ["hpa.runtime", new RuntimeProvider(services)],
    ["hpa.executions", new ExecutionsProvider(services)],
    ["hpa.artifacts", new ArtifactsProvider(services)],
    ["hpa.settings", new SettingsProvider(services)],
  ];
  const disposables: vscode.Disposable[] = [];
  for (const [id, provider] of registrations) {
    disposables.push(vscode.window.registerTreeDataProvider(id, provider));
    disposables.push(provider);
  }
  return disposables;
}
