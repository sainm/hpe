import * as vscode from "vscode";
import type { HpaServices, InstalledSkillRecord } from "../contracts";
import type {
  AgentSummary,
  Artifact,
  ExecutionSummary,
  PluginSummary,
  SkillSummary,
  WorkflowSummary,
} from "../runtime/types";

export interface HpaTreeItem extends vscode.TreeItem {
  workflow?: WorkflowSummary;
  execution?: ExecutionSummary;
  artifact?: Artifact;
  skill?: SkillSummary;
  record?: InstalledSkillRecord;
  agent?: AgentSummary;
  plugin?: PluginSummary;
  children?: HpaTreeItem[];
}

export function treeItem(
  label: string,
  collapsibleState: vscode.TreeItemCollapsibleState = vscode.TreeItemCollapsibleState.None,
): HpaTreeItem {
  return new vscode.TreeItem(label, collapsibleState) as HpaTreeItem;
}

export function groupItem(
  label: string,
  icon: string,
  children: HpaTreeItem[],
  description?: string,
): HpaTreeItem {
  const item = treeItem(label, vscode.TreeItemCollapsibleState.Expanded);
  item.iconPath = new vscode.ThemeIcon(icon);
  item.contextValue = "hpa.group";
  item.children = children;
  if (description !== undefined) item.description = description;
  return item;
}

export function fieldItem(
  services: HpaServices,
  label: string,
  value: string,
  icon: string,
  command?: vscode.Command,
): HpaTreeItem {
  const item = treeItem(label);
  item.description = value;
  item.tooltip = `${label}: ${value}`;
  item.iconPath = new vscode.ThemeIcon(icon);
  if (command) item.command = command;
  return item;
}

export function actionItem(
  label: string,
  icon: string,
  command: string,
  title: string,
  args: unknown[] = [],
): HpaTreeItem {
  const item = treeItem(label);
  item.iconPath = new vscode.ThemeIcon(icon);
  item.command = { command, title, arguments: args };
  return item;
}

export function offlineItem(services: HpaServices): HpaTreeItem {
  const label = services.i18n.t("view.offline");
  const item = treeItem(label);
  item.iconPath = new vscode.ThemeIcon("debug-disconnect");
  item.contextValue = "hpa.offline";
  item.tooltip = services.i18n.t("runtime.startHint");
  item.command = { command: "hpa.runtime.connect", title: label };
  return item;
}

export function errorItem(services: HpaServices, error: unknown): HpaTreeItem {
  const message = errorMessage(error);
  const label = services.i18n.t("view.error");
  const item = treeItem(label);
  item.description = message;
  item.tooltip = `${label}: ${message}`;
  item.iconPath = new vscode.ThemeIcon("error");
  item.contextValue = "hpa.error";
  return item;
}

export function emptyItem(services: HpaServices): HpaTreeItem {
  const item = treeItem(services.i18n.t("view.empty"));
  item.iconPath = new vscode.ThemeIcon("info");
  item.contextValue = "hpa.empty";
  return item;
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return String(bytes);
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(1)} ${units[index] ?? "KB"}`;
}

export function runtimeStatusLabel(services: HpaServices): string {
  return services.i18n.t(`status.${services.runtime().status}`);
}

export abstract class ViewProvider implements vscode.TreeDataProvider<HpaTreeItem>, vscode.Disposable {
  private readonly changed = new vscode.EventEmitter<HpaTreeItem | undefined | void>();
  private readonly subscriptions: vscode.Disposable[];

  readonly onDidChangeTreeData = this.changed.event;

  protected constructor(
    protected readonly services: HpaServices,
    private readonly onlineOnly = true,
  ) {
    this.subscriptions = [
      services.onDidChangeRuntime(() => this.refresh()),
      services.dataEvents.onDidChange(() => this.refresh()),
    ];
  }

  getTreeItem(element: HpaTreeItem): vscode.TreeItem {
    return element;
  }

  async getChildren(element?: HpaTreeItem): Promise<HpaTreeItem[]> {
    if (element) return element.children ?? [];
    if (this.onlineOnly && this.services.runtime().status !== "online") {
      return [offlineItem(this.services)];
    }
    try {
      return await this.load();
    } catch (error) {
      return [errorItem(this.services, error)];
    }
  }

  refresh(): void {
    this.changed.fire();
  }

  dispose(): void {
    for (const subscription of this.subscriptions) subscription.dispose();
    this.changed.dispose();
  }

  protected abstract load(): Promise<HpaTreeItem[]>;
}
