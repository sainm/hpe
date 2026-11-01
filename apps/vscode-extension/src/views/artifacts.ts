import * as vscode from "vscode";
import type { HpaServices } from "../contracts";
import type { Artifact } from "../runtime/types";
import {
  emptyItem,
  formatSize,
  treeItem,
  ViewProvider,
  type HpaTreeItem,
} from "./common";

export class ArtifactsProvider extends ViewProvider {
  constructor(services: HpaServices) {
    super(services);
  }

  protected async load(): Promise<HpaTreeItem[]> {
    const artifacts = await this.services.client.listArtifacts();
    if (artifacts.length === 0) return [emptyItem(this.services)];
    return artifacts.map((artifact) => this.artifactItem(artifact));
  }

  private artifactItem(artifact: Artifact): HpaTreeItem {
    const i18n = this.services.i18n;
    const item = treeItem(artifact.name);
    item.description = `${artifact.kind} · ${formatSize(artifact.size)} · ${artifact.createdAt}`;
    item.iconPath = new vscode.ThemeIcon("file-media");
    item.contextValue = "hpa.artifact";
    item.tooltip = [
      `${i18n.t("common.name")}: ${artifact.name}`,
      `${i18n.t("common.id")}: ${artifact.id}`,
      `${i18n.t("common.type")}: ${artifact.kind}`,
      `${i18n.t("common.size")}: ${formatSize(artifact.size)}`,
      `${i18n.t("common.time")}: ${artifact.createdAt}`,
    ].join("\n");
    item.artifact = artifact;
    item.command = {
      command: "hpa.artifact.preview",
      title: i18n.t("artifacts.preview"),
      arguments: [item],
    };
    return item;
  }
}
