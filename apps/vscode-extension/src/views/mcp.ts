import * as vscode from "vscode";
import type { HpaServices } from "../contracts";
import type { McpServer } from "../runtime/types";
import {
  actionItem,
  emptyItem,
  fieldItem,
  groupItem,
  treeItem,
  ViewProvider,
  type HpaTreeItem,
} from "./common";

export class McpProvider extends ViewProvider {
  constructor(services: HpaServices) {
    super(services);
  }

  protected async load(): Promise<HpaTreeItem[]> {
    const [overview, servers] = await Promise.all([
      this.services.client.mcpOverview(),
      this.services.client.listMcpServers(),
    ]);
    const i18n = this.services.i18n;
    const openCommand: vscode.Command = {
      command: "hpa.web.open",
      title: i18n.t("mcp.open"),
      arguments: ["/mcp"],
    };
    const serverNodes =
      servers.length === 0
        ? [emptyItem(this.services)]
        : servers.map((server) => this.serverItem(server, openCommand));
    return [
      fieldItem(
        this.services,
        i18n.t("mcp.endpoint"),
        overview.endpoint,
        "globe",
        openCommand,
      ),
      fieldItem(
        this.services,
        i18n.t("mcp.tools"),
        String(overview.stats.tools),
        "tools",
        openCommand,
      ),
      fieldItem(
        this.services,
        i18n.t("mcp.prompts"),
        String(overview.stats.prompts),
        "comment-discussion",
        openCommand,
      ),
      fieldItem(
        this.services,
        i18n.t("mcp.resources"),
        String(overview.stats.resources),
        "library",
        openCommand,
      ),
      groupItem(
        i18n.t("mcp.servers"),
        "server",
        serverNodes,
        String(overview.stats.servers),
      ),
      actionItem(
        i18n.t("mcp.register"),
        "add",
        "hpa.mcp.register",
        i18n.t("mcp.register"),
      ),
    ];
  }

  private serverItem(server: McpServer, openCommand: vscode.Command): HpaTreeItem {
    const i18n = this.services.i18n;
    const endpoint = server.url ?? (server.command.length > 0 ? server.command.join(" ") : i18n.t("common.never"));
    const item = treeItem(server.name);
    item.description = `${server.transport} · ${endpoint}`;
    item.iconPath = new vscode.ThemeIcon("server-environment");
    item.contextValue = "hpa.mcp.server";
    item.tooltip = [
      `${i18n.t("common.name")}: ${server.name}`,
      `${i18n.t("common.id")}: ${server.id}`,
      `${i18n.t("common.type")}: ${server.transport}`,
      `${i18n.t("common.details")}: ${endpoint}`,
      `${i18n.t("mcp.tools")}: ${server.allowedTools.join(", ")}`,
    ].join("\n");
    item.command = openCommand;
    return item;
  }
}
