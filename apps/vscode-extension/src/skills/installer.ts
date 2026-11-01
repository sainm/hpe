import { createHash } from "node:crypto";
import { homedir } from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import type { HpaServices, InstalledSkillRecord } from "../contracts";
import type { SkillSummary } from "../runtime/types";

export interface SkillTreeNode {
  skill?: SkillSummary;
  record?: InstalledSkillRecord;
}

type SkillTarget = "project" | "personal";

interface HashEntry {
  rel: string;
  uri: vscode.Uri;
}

export class SkillInstaller {
  private readonly context: vscode.ExtensionContext;
  private readonly services: HpaServices;

  constructor(context: vscode.ExtensionContext, services: HpaServices) {
    this.context = context;
    this.services = services;
  }

  async resolveTarget(skillName: string, target?: SkillTarget): Promise<vscode.Uri> {
    this.assertSkillName(skillName);
    const selected = target ?? this.services.config().skillTarget;
    const root = this.rootFor(selected);
    const destination = path.resolve(root, skillName);
    this.assertWithin(root, destination);
    return vscode.Uri.file(destination);
  }

  async hashTarget(uri: vscode.Uri): Promise<string> {
    const files: HashEntry[] = [];
    await this.collectFiles(uri, "", files);
    files.sort((a, b) => (a.rel < b.rel ? -1 : a.rel > b.rel ? 1 : 0));
    const aggregate = createHash("sha256");
    for (const file of files) {
      const data = await vscode.workspace.fs.readFile(file.uri);
      const digest = createHash("sha256").update(data).digest("hex");
      aggregate.update(`${file.rel}\u0000${digest}\n`);
    }
    return aggregate.digest("hex");
  }

  async install(skill?: SkillSummary, target?: SkillTarget): Promise<void> {
    try {
      this.ensureOnline();
      this.ensureTrusted();
      const chosen = skill ?? (await this.pickSkill());
      if (!chosen) return;
      const destination = await this.resolveTarget(chosen.name, target);
      await this.installTo(chosen, destination, undefined, "skills.installedToast");
    } catch (error) {
      this.showFailure("skills.installFailed", error);
    }
  }

  async update(node?: SkillTreeNode): Promise<void> {
    try {
      this.ensureOnline();
      this.ensureTrusted();
      const record = await this.selectRecord(node, "skills.update");
      if (!record) return;
      const destination = vscode.Uri.file(path.resolve(record.targetPath));
      if (!this.classifyTarget(destination.fsPath)) {
        throw new Error(this.services.i18n.t("skills.targetOutside"));
      }
      const skills = await this.services.client.listSkills();
      const summary =
        skills.find((entry) => entry.id === record.skillId) ??
        skills.find((entry) => entry.name === record.name);
      const skill: SkillSummary = summary ?? {
        id: record.skillId,
        name: record.name,
        latestVersion: record.version,
        license: null,
        status: "unknown",
        updatedAt: record.installedAt,
      };
      await this.installTo(skill, destination, record, "skills.updated");
    } catch (error) {
      this.showFailure("skills.updateFailed", error);
    }
  }

  async remove(node?: SkillTreeNode): Promise<void> {
    try {
      this.ensureTrusted();
      const record = await this.selectRecord(node, "skills.remove");
      if (!record) return;
      const destination = vscode.Uri.file(path.resolve(record.targetPath));
      if (!this.classifyTarget(destination.fsPath)) {
        throw new Error(this.services.i18n.t("skills.targetOutside"));
      }
      const exists = await this.uriExists(destination);
      if (exists) {
        const localHash = await this.hashTarget(destination);
        if (localHash !== record.localHash) {
          const confirmed = await this.confirmLocalChanges();
          if (!confirmed) return;
        }
      }
      const confirmed = await this.confirmRemove(record, destination);
      if (!confirmed) return;
      if (exists) {
        await vscode.workspace.fs.delete(destination, { recursive: true, useTrash: false });
      }
      await this.services.deleteInstalledSkill(record.skillId);
      void vscode.window.showInformationMessage(
        this.services.i18n.t("skills.removed", { name: record.name }),
      );
    } catch (error) {
      this.showFailure("skills.removeFailed", error);
    }
  }

  records(): InstalledSkillRecord[] {
    return Object.values(this.services.getInstalledSkills());
  }

  private async installTo(
    skill: SkillSummary,
    destination: vscode.Uri,
    knownRecord: InstalledSkillRecord | undefined,
    toastKey: string,
  ): Promise<void> {
    const version = skill.latestVersion;
    const detail = await this.services.client.getSkill(skill.id, version);
    const existing = await this.uriExists(destination);
    let targetRecord = knownRecord;
    if (
      !targetRecord ||
      path.resolve(targetRecord.targetPath) !== path.resolve(destination.fsPath)
    ) {
      targetRecord = this.findRecord(destination.fsPath);
    }
    if (existing) {
      let localChanged = false;
      if (targetRecord) {
        const currentHash = await this.hashTarget(destination);
        localChanged = currentHash !== targetRecord.localHash;
      }
      const confirmed = await this.confirmReplace(destination, localChanged);
      if (!confirmed) return;
    }
    const parentPath = path.dirname(destination.fsPath);
    const parentUri = vscode.Uri.file(parentPath);
    let backup: vscode.Uri | undefined;
    let replaced = false;
    try {
      await vscode.workspace.fs.createDirectory(parentUri);
      if (existing) {
        backup = vscode.Uri.file(
          path.join(parentPath, `${path.basename(destination.fsPath)}.backup-${Date.now()}`),
        );
        await vscode.workspace.fs.rename(destination, backup, { overwrite: false });
        replaced = true;
      }
      const result = await this.services.client.exportSkill(skill.id, version, destination.fsPath);
      if (!result.exported) {
        throw new Error(this.services.i18n.t("skills.installFailed"));
      }
      if (!(await this.uriExists(destination))) {
        throw new Error(this.services.i18n.t("skills.installFailed"));
      }
      const localHash = await this.hashTarget(destination);
      await this.services.setInstalledSkill({
        skillId: skill.id,
        name: skill.name,
        version: detail.skill.version || version,
        contentHash: detail.skill.contentHash,
        localHash,
        targetPath: destination.fsPath,
        installedAt: new Date().toISOString(),
      });
      if (backup) await this.removeQuietly(backup);
      void vscode.window.showInformationMessage(
        this.services.i18n.t(toastKey, { name: skill.name }),
      );
    } catch (error) {
      if (replaced) {
        await this.removeQuietly(destination);
        if (backup) await this.renameQuietly(backup, destination);
      } else if (!existing) {
        await this.removeQuietly(destination);
      }
      throw error;
    }
  }

  private async pickSkill(): Promise<SkillSummary | undefined> {
    const skills = await this.services.client.listSkills();
    const items = skills.map((skill) => ({
      label: skill.name,
      description: skill.latestVersion,
      skill,
    }));
    const picked = await vscode.window.showQuickPick(items, {
      placeHolder: this.services.i18n.t("skills.select"),
    });
    return picked?.skill;
  }

  private async selectRecord(
    node: SkillTreeNode | undefined,
    actionKey: string,
  ): Promise<InstalledSkillRecord | undefined> {
    if (node?.record) return node.record;
    const all = this.records();
    if (all.length === 1) return all[0];
    if (all.length === 0) return undefined;
    const items = all.map((record) => ({
      label: record.name,
      description: `${record.version} · ${record.targetPath}`,
      record,
    }));
    const picked = await vscode.window.showQuickPick(items, {
      placeHolder: this.services.i18n.t(actionKey),
    });
    return picked?.record;
  }

  private async confirmReplace(destination: vscode.Uri, localChanged: boolean): Promise<boolean> {
    const confirm = this.services.i18n.t("common.confirm");
    if (localChanged) {
      const choice = await vscode.window.showWarningMessage(
        this.services.i18n.t("skills.localChanges"),
        { modal: true },
        confirm,
      );
      return choice === confirm;
    }
    const message = `${this.services.i18n.t("skills.exists", {
      path: destination.fsPath,
    })}\n${this.services.i18n.t("skills.overwrite")}`;
    const choice = await vscode.window.showWarningMessage(message, { modal: true }, confirm);
    return choice === confirm;
  }

  private async confirmLocalChanges(): Promise<boolean> {
    const confirm = this.services.i18n.t("common.confirm");
    const choice = await vscode.window.showWarningMessage(
      this.services.i18n.t("skills.localChanges"),
      { modal: true },
      confirm,
    );
    return choice === confirm;
  }

  private async confirmRemove(
    record: InstalledSkillRecord,
    destination: vscode.Uri,
  ): Promise<boolean> {
    const confirm = this.services.i18n.t("common.confirm");
    const choice = await vscode.window.showWarningMessage(
      this.services.i18n.t("skills.removeConfirm", {
        name: record.name,
        path: destination.fsPath,
      }),
      { modal: true },
      confirm,
    );
    return choice === confirm;
  }

  private rootFor(target: SkillTarget): string {
    if (target === "personal") return this.personalRoot();
    const workspace = this.services.workspaceRoot();
    if (!workspace) throw new Error(this.services.i18n.t("skills.noWorkspace"));
    return path.join(workspace.fsPath, ".github", "skills");
  }

  private personalRoot(): string {
    const configured = this.services.config().personalSkillsPath;
    return path.resolve(configured !== "" ? configured : path.join(homedir(), ".copilot", "skills"));
  }

  private classifyTarget(targetPath: string): SkillTarget | undefined {
    const workspace = this.services.workspaceRoot();
    if (workspace) {
      const projectRoot = path.join(workspace.fsPath, ".github", "skills");
      if (this.within(projectRoot, targetPath)) return "project";
    }
    if (this.within(this.personalRoot(), targetPath)) return "personal";
    return undefined;
  }

  private assertSkillName(skillName: string): void {
    if (typeof skillName !== "string" || skillName === "" || skillName === "." || skillName === "..") {
      throw new Error(this.services.i18n.t("skills.targetOutside"));
    }
    if (skillName.includes("/") || skillName.includes("\\") || path.isAbsolute(skillName)) {
      throw new Error(this.services.i18n.t("skills.targetOutside"));
    }
    if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(skillName)) {
      throw new Error(this.services.i18n.t("skills.targetOutside"));
    }
  }

  private assertWithin(root: string, destination: string): void {
    if (!this.within(root, destination)) {
      throw new Error(this.services.i18n.t("skills.targetOutside"));
    }
  }

  private within(root: string, candidate: string): boolean {
    const relative = path.relative(path.resolve(root), path.resolve(candidate));
    return (
      relative !== "" &&
      relative !== ".." &&
      !relative.startsWith(`..${path.sep}`) &&
      !path.isAbsolute(relative)
    );
  }

  private assertNotSymlink(type: vscode.FileType): void {
    if ((type & vscode.FileType.SymbolicLink) !== 0) {
      throw new Error(this.services.i18n.t("skills.targetOutside"));
    }
  }

  private async collectFiles(
    uri: vscode.Uri,
    prefix: string,
    files: HashEntry[],
  ): Promise<void> {
    const entries = await vscode.workspace.fs.readDirectory(uri);
    for (const [name, type] of entries) {
      this.assertNotSymlink(type);
      const child = vscode.Uri.joinPath(uri, name);
      const rel = prefix === "" ? name : `${prefix}/${name}`;
      if ((type & vscode.FileType.Directory) !== 0) {
        await this.collectFiles(child, rel, files);
      } else if ((type & vscode.FileType.File) !== 0) {
        files.push({ rel, uri: child });
      } else {
        throw new Error(this.services.i18n.t("skills.targetOutside"));
      }
    }
  }

  private findRecord(targetPath: string): InstalledSkillRecord | undefined {
    return this.records().find(
      (record) => path.resolve(record.targetPath) === path.resolve(targetPath),
    );
  }

  private async uriExists(uri: vscode.Uri): Promise<boolean> {
    try {
      await vscode.workspace.fs.stat(uri);
      return true;
    } catch {
      return false;
    }
  }

  private async removeQuietly(uri: vscode.Uri): Promise<void> {
    try {
      await vscode.workspace.fs.delete(uri, { recursive: true, useTrash: false });
    } catch {
      return;
    }
  }

  private async renameQuietly(source: vscode.Uri, target: vscode.Uri): Promise<void> {
    try {
      await vscode.workspace.fs.rename(source, target, { overwrite: true });
    } catch {
      return;
    }
  }

  private ensureOnline(): void {
    const state = this.services.requireOnline();
    if (state.status !== "online") {
      throw new Error(this.services.i18n.t("runtime.notRunning"));
    }
  }

  private ensureTrusted(): void {
    if (!this.services.requireTrusted()) {
      throw new Error(this.services.i18n.t("skills.untrusted"));
    }
  }

  private showFailure(key: string, error: unknown): void {
    const detail = error instanceof Error ? error.message : String(error);
    void vscode.window.showErrorMessage(`${this.services.i18n.t(key)}: ${detail}`);
  }
}
