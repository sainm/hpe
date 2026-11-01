import * as vscode from "vscode";
import { registerCommands } from "./commands";
import { RuntimeController } from "./controller";
import { createCopilotIntegration } from "./copilot";
import { SkillInstaller } from "./skills/installer";
import { registerViews } from "./views";

export interface WorkbenchApi {
  controller: RuntimeController;
  installer: SkillInstaller;
  copilot: ReturnType<typeof createCopilotIntegration>;
}

export function activate(context: vscode.ExtensionContext): WorkbenchApi {
  const controller = new RuntimeController(context);
  const installer = new SkillInstaller(context, controller);
  const copilot = createCopilotIntegration(context, controller);
  const viewDisposables = registerViews(context, controller);
  const copilotDisposables = copilot.register();
  const commandDisposables = registerCommands(context, controller, installer, copilot);
  context.subscriptions.push(
    controller,
    ...viewDisposables,
    ...copilotDisposables,
    ...commandDisposables,
  );
  controller.startHealthLoop();
  controller.output.appendLine("hpa extension activated");
  return { controller, installer, copilot };
}

export function deactivate(): void {}
