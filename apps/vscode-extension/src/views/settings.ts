import * as vscode from "vscode";
import type { HpaServices } from "../contracts";
import {
  fieldItem,
  groupItem,
  runtimeStatusLabel,
  ViewProvider,
  type HpaTreeItem,
} from "./common";

const API_TOKEN_KEY = "hpa.api.token";

export class SettingsProvider extends ViewProvider {
  constructor(services: HpaServices) {
    super(services, false);
  }

  protected async load(): Promise<HpaTreeItem[]> {
    const i18n = this.services.i18n;
    const config = this.services.config();
    const token = await this.services.secrets.get(API_TOKEN_KEY);
    const target =
      config.skillTarget === "personal"
        ? i18n.t("skills.targetPersonal")
        : i18n.t("skills.targetProject");
    const settingsCommand: vscode.Command = {
      command: "workbench.action.openSettings",
      title: i18n.t("settings.open"),
      arguments: ["@ext:hpa.workbench"],
    };
    const endpointItem = fieldItem(
      this.services,
      i18n.t("settings.endpoint"),
      this.services.client.endpoint,
      "globe",
      settingsCommand,
    );
    const modeItem = fieldItem(
      this.services,
      i18n.t("settings.mode"),
      config.mode,
      "settings",
      settingsCommand,
    );
    const localeItem = fieldItem(
      this.services,
      i18n.t("settings.locale"),
      config.locale,
      "globe",
      settingsCommand,
    );
    const targetItem = fieldItem(
      this.services,
      i18n.t("settings.skillTarget"),
      target,
      "folder",
      settingsCommand,
    );
    const authItem = fieldItem(
      this.services,
      i18n.t("settings.auth"),
      token ? i18n.t("auth.saved") : i18n.t("auth.notSet"),
      "key",
    );
    authItem.command = {
      command: "hpa.auth.setToken",
      title: i18n.t("settings.auth"),
    };
    const diagnostics = [
      fieldItem(
        this.services,
        i18n.t("overview.runtime"),
        runtimeStatusLabel(this.services),
        "pulse",
        settingsCommand,
      ),
      fieldItem(
        this.services,
        i18n.t("settings.endpoint"),
        this.services.client.endpoint,
        "globe",
        settingsCommand,
      ),
      fieldItem(
        this.services,
        i18n.t("mcp.endpoint"),
        this.services.mcpEndpoint() || i18n.t("common.never"),
        "server",
        settingsCommand,
      ),
      fieldItem(
        this.services,
        i18n.t("settings.mode"),
        config.mode,
        "settings",
        settingsCommand,
      ),
      fieldItem(
        this.services,
        i18n.t("settings.locale"),
        config.locale,
        "globe",
        settingsCommand,
      ),
      fieldItem(
        this.services,
        i18n.t("settings.skillTarget"),
        target,
        "folder",
        settingsCommand,
      ),
      fieldItem(
        this.services,
        i18n.t("settings.auth"),
        token ? i18n.t("auth.saved") : i18n.t("auth.notSet"),
        "key",
        settingsCommand,
      ),
      fieldItem(
        this.services,
        i18n.t("runtime.version"),
        this.services.runtime().health?.version ?? i18n.t("common.never"),
        "versions",
        settingsCommand,
      ),
      fieldItem(
        this.services,
        i18n.t("common.time"),
        this.services.runtime().checkedAt,
        "clock",
        settingsCommand,
      ),
    ];
    const diagnosticsItem = groupItem(
      i18n.t("settings.diagnostics"),
      "pulse",
      diagnostics,
    );
    diagnosticsItem.command = settingsCommand;
    return [endpointItem, modeItem, localeItem, targetItem, authItem, diagnosticsItem];
  }
}
