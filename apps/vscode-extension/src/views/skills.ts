import * as vscode from "vscode";
import type { HpaServices, InstalledSkillRecord } from "../contracts";
import type { SkillSummary } from "../runtime/types";
import { emptyItem, treeItem, ViewProvider, type HpaTreeItem } from "./common";

export class SkillsProvider extends ViewProvider {
  constructor(services: HpaServices) {
    super(services);
  }

  protected async load(): Promise<HpaTreeItem[]> {
    const skills = await this.services.client.listSkills();
    if (skills.length === 0) return [emptyItem(this.services)];
    const installed = this.services.getInstalledSkills();
    return skills.map((skill) => this.skillItem(skill, installed[skill.id]));
  }

  private skillItem(
    skill: SkillSummary,
    record: InstalledSkillRecord | undefined,
  ): HpaTreeItem {
    const i18n = this.services.i18n;
    const state = record ? i18n.t("skills.installed") : i18n.t("skills.available");
    const item = treeItem(skill.name);
    item.description = `${skill.latestVersion} · ${skill.status} · ${state}`;
    item.iconPath = new vscode.ThemeIcon(record ? "check" : "lightbulb");
    item.contextValue = record ? "hpa.skill.installed" : "hpa.skill";
    item.tooltip = [
      `${i18n.t("common.name")}: ${skill.name}`,
      `${i18n.t("common.id")}: ${skill.id}`,
      `${i18n.t("common.version")}: ${skill.latestVersion}`,
      `${i18n.t("common.status")}: ${skill.status}`,
      `${i18n.t("skills.installed")}: ${record ? record.version : i18n.t("common.no")}`,
      record ? `${i18n.t("common.details")}: ${record.targetPath}` : "",
    ]
      .filter((line) => line !== "")
      .join("\n");
    item.skill = skill;
    if (record) item.record = record;
    item.command = {
      command: "hpa.web.open",
      title: i18n.t("workflows.open"),
      arguments: ["/skills"],
    };
    return item;
  }
}
