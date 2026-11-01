import * as vscode from "vscode";
import type { HpaServices } from "../contracts";
import type { AgentSummary } from "../runtime/types";
import { emptyItem, treeItem, ViewProvider, type HpaTreeItem } from "./common";

export class AgentsProvider extends ViewProvider {
  constructor(services: HpaServices) {
    super(services);
  }

  protected async load(): Promise<HpaTreeItem[]> {
    const agents = await this.services.client.listAgents();
    if (agents.length === 0) return [emptyItem(this.services)];
    return agents.map((agent) => this.agentItem(agent));
  }

  private agentItem(agent: AgentSummary): HpaTreeItem {
    const i18n = this.services.i18n;
    const item = treeItem(agent.name);
    item.description = `${agent.latestVersion} · ${agent.status}`;
    item.iconPath = new vscode.ThemeIcon("robot");
    item.contextValue = "hpa.agent";
    item.tooltip = [
      `${i18n.t("common.name")}: ${agent.name}`,
      `${i18n.t("common.id")}: ${agent.id}`,
      `${i18n.t("common.version")}: ${agent.latestVersion}`,
      `${i18n.t("common.status")}: ${agent.status}`,
      `${i18n.t("common.time")}: ${agent.updatedAt}`,
    ].join("\n");
    item.agent = agent;
    item.command = {
      command: "hpa.web.open",
      title: i18n.t("agents.open"),
      arguments: ["/agents"],
    };
    return item;
  }
}
