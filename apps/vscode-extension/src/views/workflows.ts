import * as vscode from "vscode";
import type { HpaServices } from "../contracts";
import type { WorkflowSummary } from "../runtime/types";
import { emptyItem, treeItem, ViewProvider, type HpaTreeItem } from "./common";

export class WorkflowsProvider extends ViewProvider {
  constructor(services: HpaServices) {
    super(services);
  }

  protected async load(): Promise<HpaTreeItem[]> {
    const workflows = await this.services.client.listWorkflows();
    if (workflows.length === 0) return [emptyItem(this.services)];
    return workflows.map((workflow) => this.workflowItem(workflow));
  }

  private workflowItem(workflow: WorkflowSummary): HpaTreeItem {
    const i18n = this.services.i18n;
    const state = workflow.published ? i18n.t("workflows.published") : i18n.t("workflows.draft");
    const item = treeItem(workflow.name);
    item.description = `${workflow.version} · ${state}`;
    item.iconPath = new vscode.ThemeIcon(workflow.published ? "rocket" : "edit");
    item.contextValue = "hpa.workflow";
    item.tooltip = [
      `${i18n.t("common.name")}: ${workflow.name}`,
      `${i18n.t("common.id")}: ${workflow.id}`,
      `${i18n.t("common.version")}: ${workflow.version}`,
      `${i18n.t("common.status")}: ${state}`,
      `${i18n.t("common.time")}: ${workflow.updatedAt}`,
      workflow.description ?? "",
    ]
      .filter((line) => line !== "")
      .join("\n");
    item.workflow = workflow;
    item.command = {
      command: "hpa.workflow.run",
      title: i18n.t("workflows.run"),
      arguments: [item],
    };
    return item;
  }
}
