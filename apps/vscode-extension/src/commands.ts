import * as vscode from "vscode";
import type { CopilotIntegration } from "./copilot";
import type { HpaServices } from "./contracts";
import { RuntimeApiError } from "./runtime/client";
import type {
  ApprovalSummary,
  Artifact,
  ExecutionSummary,
  JsonValue,
  WorkflowSummary,
} from "./runtime/types";
import type { SkillInstaller, SkillTreeNode } from "./skills/installer";
import { openArtifactPanel, openExecutionPanel } from "./views";

export function registerCommands(
  context: vscode.ExtensionContext,
  services: HpaServices,
  installer: SkillInstaller,
  copilot: CopilotIntegration,
): vscode.Disposable[] {
  return [
    register("hpa.runtime.refresh", async () => {
      services.dataEvents.fire();
      await services.refreshRuntime();
    }),
    register("hpa.runtime.connect", async () => {
      const state = await services.refreshRuntime();
      if (state.status === "online") {
        void vscode.window.showInformationMessage(services.i18n.t("runtime.connected"));
      } else {
        void vscode.window.showWarningMessage(
          `${services.i18n.t("runtime.disconnected")} ${state.error ?? ""}`,
        );
      }
    }),
    register("hpa.runtime.status", async () => {
      const state = services.runtime();
      const lines = [
        `${services.i18n.t("common.status")}: ${services.i18n.t(`status.${state.status}`)}`,
        `${services.i18n.t("runtime.endpoint")}: ${services.client.endpoint}`,
        `${services.i18n.t("runtime.version")}: ${state.health?.version ?? services.i18n.t("common.unknown")}`,
      ];
      if (state.error) lines.push(`${services.i18n.t("common.error")}: ${state.error}`);
      void vscode.window.showInformationMessage(lines.join(" · "));
    }),
    register("hpa.runtime.start", async () => {
      const config = services.config();
      if (!config.binary) {
        void vscode.window.showWarningMessage(services.i18n.t("runtime.noBinary"));
        return;
      }
      const confirmation = await vscode.window.showWarningMessage(
        services.i18n.t("runtime.startConfirm"),
        { modal: true },
        services.i18n.t("common.confirm"),
        services.i18n.t("common.cancel"),
      );
      if (confirmation !== services.i18n.t("common.confirm")) return;
      const terminal = vscode.window.createTerminal({
        name: "hpa",
        cwd: services.workspaceRoot(),
        shellPath: config.binary,
        shellArgs: config.startArgs,
      });
      terminal.show();
    }),
    register("hpa.settings.open", async () => {
      await vscode.commands.executeCommand("workbench.action.openSettings", "@ext:hpa.workbench");
    }),
    register("hpa.auth.setToken", async () => {
      const token = await vscode.window.showInputBox({
        prompt: services.i18n.t("auth.prompt"),
        placeHolder: services.i18n.t("auth.placeholder"),
        password: true,
        ignoreFocusOut: true,
      });
      if (!token || token.trim() === "") return;
      await services.secrets.store("hpa.api.token", token.trim());
      await services.refreshRuntime();
      void vscode.window.showInformationMessage(services.i18n.t("auth.saved"));
    }),
    register("hpa.auth.clearToken", async () => {
      await services.secrets.delete("hpa.api.token");
      await services.refreshRuntime();
      void vscode.window.showInformationMessage(services.i18n.t("auth.cleared"));
    }),
    register("hpa.web.open", async (path?: string) => {
      await services.openWeb(path || "/");
    }),
    register("hpa.workflow.run", async (argument?: unknown) => {
      if (!services.requireTrusted()) return;
      const workflow =
        workflowFrom(argument) ??
        (await pickWorkflow(services, services.i18n.t("workflows.run")));
      if (!workflow) return;
      const inputText = await vscode.window.showInputBox({
        title: services.i18n.t("run.inputTitle"),
        prompt: services.i18n.t("run.inputPrompt"),
        value: "{}",
        ignoreFocusOut: true,
      });
      if (inputText === undefined) return;
      let input: unknown;
      try {
        input = inputText.trim() === "" ? {} : JSON.parse(inputText);
      } catch {
        void vscode.window.showErrorMessage(services.i18n.t("run.inputInvalid"));
        return;
      }
      try {
        services.requireOnline();
        const execution = await services.client.runWorkflow(
          workflow.id,
          input as JsonValue,
          workflow.published ? workflow.version : undefined,
        );
        services.dataEvents.fire("executions");
        void vscode.window.showInformationMessage(
          services.i18n.t("run.started", { id: execution.summary.id }),
        );
        await openExecutionPanel(context, services, execution.summary.id);
      } catch (error) {
        showError(services, error, services.i18n.t("executions.runFailed"));
      }
    }),
    register("hpa.execution.open", async (argument?: unknown) => {
      const execution =
        executionFrom(argument) ??
        (await pickExecution(services, services.i18n.t("executions.open")));
      if (!execution) return;
      await openExecutionPanel(context, services, execution.id);
    }),
    register("hpa.execution.cancel", async (argument?: unknown) => {
      if (!services.requireTrusted()) return;
      const execution =
        executionFrom(argument) ??
        (await pickExecution(services, services.i18n.t("executions.cancel")));
      if (!execution) return;
      const confirmation = await vscode.window.showWarningMessage(
        services.i18n.t("executions.cancelConfirm", { id: execution.id }),
        { modal: true },
        services.i18n.t("common.confirm"),
        services.i18n.t("common.cancel"),
      );
      if (confirmation !== services.i18n.t("common.confirm")) return;
      try {
        services.requireOnline();
        await services.client.cancelExecution(execution.id);
        services.dataEvents.fire("executions");
        void vscode.window.showInformationMessage(services.i18n.t("executions.cancelled"));
      } catch (error) {
        showError(services, error, services.i18n.t("executions.cancelFailed"));
      }
    }),
    register("hpa.execution.approve", async (argument?: unknown) => {
      if (!services.requireTrusted()) return;
      const approval =
        approvalFrom(argument) ??
        (await pickApproval(services, services.i18n.t("executions.approvalTitle")));
      if (!approval) return;
      const decision = await vscode.window.showQuickPick(
        [
          { label: services.i18n.t("executions.approvalApprove"), value: "approved" as const },
          { label: services.i18n.t("executions.approvalReject"), value: "rejected" as const },
        ],
        {
          title: services.i18n.t("executions.approvalNode", { node: approval.request.title }),
        },
      );
      if (!decision) return;
      try {
        services.requireOnline();
        await services.client.decideApproval(
          approval.executionId,
          approval.request.nodeId,
          decision.value,
        );
        services.dataEvents.fire("executions");
        void vscode.window.showInformationMessage(
          services.i18n.t(decision.value === "approved" ? "executions.approved" : "executions.rejected"),
        );
      } catch (error) {
        showError(services, error, services.i18n.t("executions.approvalFailed"));
      }
    }),
    register("hpa.artifact.preview", async (argument?: unknown) => {
      const artifact =
        artifactFrom(argument) ??
        (await pickArtifact(services, services.i18n.t("artifacts.preview")));
      if (!artifact) return;
      await openArtifactPanel(context, services, artifact.id);
    }),
    register("hpa.skill.install", async () => {
      try {
        await installer.install();
      } catch (error) {
        showError(services, error, services.i18n.t("skills.installFailed"));
      }
    }),
    register("hpa.skill.update", async (argument?: unknown) => {
      try {
        await installer.update(skillNodeFrom(argument));
      } catch (error) {
        showError(services, error, services.i18n.t("skills.updateFailed"));
      }
    }),
    register("hpa.skill.remove", async (argument?: unknown) => {
      try {
        await installer.remove(skillNodeFrom(argument));
      } catch (error) {
        showError(services, error, services.i18n.t("skills.removeFailed"));
      }
    }),
    register("hpa.mcp.refresh", async () => {
      copilot.refresh();
      services.dataEvents.fire("mcp");
    }),
    register("hpa.mcp.register", async () => {
      if (!copilot.isSupported()) {
        void vscode.window.showWarningMessage(services.i18n.t("mcp.providerUnavailable"));
        return;
      }
      try {
        await services.mcpToken();
        copilot.refresh();
        void vscode.window.showInformationMessage(services.i18n.t("mcp.registered"));
      } catch (error) {
        showError(services, error, services.i18n.t("mcp.tokenFailed"));
      }
    }),
  ];

  function register(
    command: string,
    callback: (...args: any[]) => unknown,
  ): vscode.Disposable {
    return vscode.commands.registerCommand(command, callback);
  }
}

function workflowFrom(value: unknown): WorkflowSummary | undefined {
  return (value as { workflow?: WorkflowSummary } | undefined)?.workflow;
}

function executionFrom(value: unknown): ExecutionSummary | undefined {
  return (value as { execution?: ExecutionSummary } | undefined)?.execution;
}

function artifactFrom(value: unknown): Artifact | undefined {
  return (value as { artifact?: Artifact } | undefined)?.artifact;
}

function skillNodeFrom(value: unknown): SkillTreeNode | undefined {
  const node = value as SkillTreeNode | undefined;
  return node && (node.skill || node.record) ? node : undefined;
}

function approvalFrom(value: unknown): ApprovalSummary | undefined {
  return (value as { approval?: ApprovalSummary } | undefined)?.approval;
}

async function pickWorkflow(
  services: HpaServices,
  placeHolder: string,
): Promise<WorkflowSummary | undefined> {
  try {
    services.requireOnline();
    const workflows = await services.client.listWorkflows();
    const picked = await vscode.window.showQuickPick(
      workflows.map((workflow) => ({
        label: workflow.name,
        description: workflow.version,
        detail: workflow.id,
        workflow,
      })),
      { placeHolder, matchOnDescription: true, matchOnDetail: true },
    );
    return picked?.workflow;
  } catch (error) {
    showError(services, error, services.i18n.t("view.error"));
    return undefined;
  }
}

async function pickExecution(
  services: HpaServices,
  placeHolder: string,
): Promise<ExecutionSummary | undefined> {
  try {
    services.requireOnline();
    const executions = await services.client.listExecutions();
    const picked = await vscode.window.showQuickPick(
      executions.map((execution) => ({
        label: execution.id,
        description: execution.status,
        detail: execution.workflowId,
        execution,
      })),
      { placeHolder, matchOnDescription: true, matchOnDetail: true },
    );
    return picked?.execution;
  } catch (error) {
    showError(services, error, services.i18n.t("view.error"));
    return undefined;
  }
}

async function pickArtifact(
  services: HpaServices,
  placeHolder: string,
): Promise<Artifact | undefined> {
  try {
    services.requireOnline();
    const artifacts = await services.client.listArtifacts();
    const picked = await vscode.window.showQuickPick(
      artifacts.map((artifact) => ({
        label: artifact.name,
        description: artifact.kind,
        detail: artifact.executionId,
        artifact,
      })),
      { placeHolder, matchOnDescription: true, matchOnDetail: true },
    );
    return picked?.artifact;
  } catch (error) {
    showError(services, error, services.i18n.t("view.error"));
    return undefined;
  }
}

async function pickApproval(
  services: HpaServices,
  placeHolder: string,
): Promise<ApprovalSummary | undefined> {
  try {
    services.requireOnline();
    const approvals = await services.client.listApprovals();
    const picked = await vscode.window.showQuickPick(
      approvals.map((approval) => ({
        label: approval.request.title,
        description: approval.workflowId,
        detail: approval.executionId,
        approval,
      })),
      { placeHolder, matchOnDescription: true, matchOnDetail: true },
    );
    return picked?.approval;
  } catch (error) {
    showError(services, error, services.i18n.t("view.error"));
    return undefined;
  }
}

function showError(services: HpaServices, error: unknown, fallback: string): void {
  const message =
    error instanceof RuntimeApiError
      ? services.i18n.t("errors.http", {
          status: error.status,
          code: error.code,
          message: error.message,
        })
      : error instanceof Error
        ? `${fallback} ${error.message}`
        : fallback;
  void vscode.window.showErrorMessage(message);
  services.output.appendLine(message);
}
