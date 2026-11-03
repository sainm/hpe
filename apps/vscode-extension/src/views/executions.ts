import * as vscode from "vscode";
import type { HpaServices } from "../contracts";
import type { ExecutionSummary } from "../runtime/types";
import { emptyItem, treeItem, ViewProvider, type HpaTreeItem } from "./common";

export class ExecutionsProvider extends ViewProvider {
  constructor(services: HpaServices) {
    super(services);
  }

  protected async load(): Promise<HpaTreeItem[]> {
    const executions = await this.services.client.listExecutions();
    if (executions.length === 0) return [emptyItem(this.services)];
    return [...executions]
      .sort((left, right) => timestamp(right.startedAt) - timestamp(left.startedAt))
      .slice(0, 50)
      .map((execution) => this.executionItem(execution));
  }

  private executionItem(execution: ExecutionSummary): HpaTreeItem {
    const i18n = this.services.i18n;
    const item = treeItem(execution.workflowId);
    item.description = `${execution.status} · ${execution.startedAt}`;
    item.iconPath = new vscode.ThemeIcon(iconForStatus(execution.status));
    item.contextValue = contextForStatus(execution.status);
    item.tooltip = [
      `${i18n.t("common.id")}: ${execution.id}`,
      `${i18n.t("common.status")}: ${execution.status}`,
      `${i18n.t("common.version")}: ${execution.workflowVersion}`,
      `${i18n.t("common.time")}: ${execution.startedAt}`,
      execution.waiting ? `${i18n.t("executions.approvalTitle")}: ${execution.waiting.title}` : "",
      execution.waiting
        ? i18n.t("executions.approvalNode", { node: execution.waiting.nodeId })
        : "",
      execution.error ? `${i18n.t("common.error")}: ${execution.error.message}` : "",
    ]
      .filter((line) => line !== "")
      .join("\n");
    item.execution = execution;
    item.command = {
      command: "hpa.execution.open",
      title: i18n.t("executions.open"),
      arguments: [item],
    };
    return item;
  }
}

function contextForStatus(status: string): string {
  if (status === "running") return "hpa.execution.running";
  if (status === "waiting_approval") return "hpa.execution.waiting";
  if (["succeeded", "failed", "cancelled"].includes(status)) {
    return "hpa.execution.finished";
  }
  return "hpa.execution";
}

function iconForStatus(status: string): string {
  if (status === "running") return "sync~spin";
  if (status === "waiting_approval") return "bell-dot";
  if (status === "succeeded") return "pass";
  if (status === "failed") return "error";
  if (status === "cancelled") return "circle-slash";
  if (status === "pending") return "clock";
  return "run";
}

function timestamp(value: string): number {
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? 0 : parsed;
}
