import type * as vscode from "vscode";
import type { HpaConfig } from "./config";
import type { I18n } from "./i18n";
import type { RuntimeClient } from "./runtime/client";
import type { RuntimeState } from "./runtime/types";

export interface InstalledSkillRecord {
  skillId: string;
  name: string;
  version: string;
  contentHash: string;
  localHash: string;
  targetPath: string;
  installedAt: string;
}

export interface RuntimeDataEvents {
  onDidChange(listener: (scope?: string) => void): vscode.Disposable;
  fire(scope?: string): void;
}

export interface HpaServices {
  readonly client: RuntimeClient;
  readonly i18n: I18n;
  readonly output: vscode.OutputChannel;
  readonly secrets: vscode.SecretStorage;
  readonly extensionUri: vscode.Uri;
  readonly dataEvents: RuntimeDataEvents;
  config(): HpaConfig;
  runtime(): RuntimeState;
  onDidChangeRuntime(listener: (state: RuntimeState) => void): vscode.Disposable;
  refreshRuntime(): Promise<RuntimeState>;
  requireOnline(): RuntimeState;
  brandName(): string;
  webUrl(path: string): string;
  openWeb(path: string): Promise<void>;
  isTrusted(): boolean;
  requireTrusted(): boolean;
  workspaceRoot(): vscode.Uri | undefined;
  mcpEndpoint(): string;
  mcpToken(): Promise<string>;
  resetMcpToken(): Promise<void>;
  getInstalledSkills(): Record<string, InstalledSkillRecord>;
  setInstalledSkill(record: InstalledSkillRecord): Promise<void>;
  deleteInstalledSkill(skillId: string): Promise<void>;
}
