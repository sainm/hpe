import * as vscode from "vscode";
import type { HpaServices } from "../contracts";
import type { PluginSummary } from "../runtime/types";
import { emptyItem, treeItem, ViewProvider, type HpaTreeItem } from "./common";

export class PluginsProvider extends ViewProvider {
  constructor(services: HpaServices) {
    super(services);
  }

  protected async load(): Promise<HpaTreeItem[]> {
    const plugins = await this.services.client.listPlugins();
    if (plugins.length === 0) return [emptyItem(this.services)];
    return plugins.map((plugin) => this.pluginItem(plugin));
  }

  private pluginItem(plugin: PluginSummary): HpaTreeItem {
    const i18n = this.services.i18n;
    const installed = plugin.installedVersion ?? i18n.t("common.never");
    const item = treeItem(plugin.name);
    item.description = `${i18n.t("skills.installed")} ${installed} · ${i18n.t("plugins.version")} ${plugin.latestVersion} · ${i18n.t("plugins.trust")} ${plugin.trust}`;
    item.iconPath = new vscode.ThemeIcon("package");
    item.contextValue = "hpa.plugin";
    item.tooltip = [
      `${i18n.t("common.name")}: ${plugin.name}`,
      `${i18n.t("common.id")}: ${plugin.id}`,
      `${i18n.t("skills.installed")}: ${installed}`,
      `${i18n.t("plugins.version")}: ${plugin.latestVersion}`,
      `${i18n.t("plugins.trust")}: ${plugin.trust}`,
      `${i18n.t("common.status")}: ${plugin.status}`,
      `${i18n.t("common.time")}: ${plugin.updatedAt}`,
    ].join("\n");
    item.plugin = plugin;
    item.command = {
      command: "hpa.web.open",
      title: i18n.t("plugins.open"),
      arguments: ["/plugins"],
    };
    return item;
  }
}
