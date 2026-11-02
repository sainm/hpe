import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import * as vscode from "vscode";
import {
  createSecretStorage,
  setMockConfiguration,
  type StatusBarItem as MockStatusBarItem,
} from "../mocks/vscode";

interface ViewContribution {
  id: string;
  name: string;
}

interface CommandContribution {
  command: string;
}

interface McpProviderContribution {
  id: string;
  label: string;
}

interface Manifest {
  name: string;
  publisher: string;
  displayName: string;
  activationEvents: string[];
  contributes: {
    views: Record<string, ViewContribution[]>;
    commands: CommandContribution[];
    mcpServerDefinitionProviders: McpProviderContribution[];
  };
}

const manifest = JSON.parse(
  readFileSync(fileURLToPath(new URL("../../package.json", import.meta.url)), "utf8"),
) as Manifest;

const nls = JSON.parse(
  readFileSync(fileURLToPath(new URL("../../package.nls.json", import.meta.url)), "utf8"),
) as Record<string, string>;

describe("extension manifest", () => {
  it("uses the hpa.workbench extension id", () => {
    expect(manifest.name).toBe("workbench");
    expect(manifest.publisher).toBe("hpa");
    expect(`${manifest.publisher}.${manifest.name}`).toBe("hpa.workbench");
    expect(manifest.displayName).toBe(nls["container.title"]);
  });

  it("contributes ten unique views", () => {
    const views = Object.values(manifest.contributes.views).flat();
    expect(views).toHaveLength(10);
    expect(views.map((view) => view.id)).toEqual([
      "hpa.overview",
      "hpa.workflows",
      "hpa.agents",
      "hpa.skills",
      "hpa.mcp",
      "hpa.plugins",
      "hpa.runtime",
      "hpa.executions",
      "hpa.artifacts",
      "hpa.settings",
    ]);
    expect(new Set(views.map((view) => view.id)).size).toBe(views.length);
  });

  it("contributes unique hpa commands", () => {
    const ids = manifest.contributes.commands.map((command) => command.command);
    expect(ids.length).toBeGreaterThan(0);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids.every((id) => id.startsWith("hpa."))).toBe(true);
    expect(ids).toEqual(
      expect.arrayContaining([
        "hpa.runtime.refresh",
        "hpa.runtime.connect",
        "hpa.workflow.run",
        "hpa.skill.install",
        "hpa.mcp.register",
        "hpa.marketplace.manage",
        "hpa.settings.open",
      ]),
    );
  });

  it("registers the hpa-local MCP provider", () => {
    const providers = manifest.contributes.mcpServerDefinitionProviders;
    expect(providers).toHaveLength(1);
    expect(providers[0]?.id).toBe("hpa-local");
    expect(providers[0]?.label).toBe("%mcp.provider.name%");
  });

  it("activates on startup", () => {
    expect(manifest.activationEvents).toContain("onStartupFinished");
  });
});

describe("vscode mock", () => {
  it("provides the core APIs used by views, installers, and activation", () => {
    expect(vscode.window.createStatusBarItem).toBeTypeOf("function");
    expect(vscode.window.registerTreeDataProvider).toBeTypeOf("function");
    expect(vscode.window.createOutputChannel).toBeTypeOf("function");
    expect(vscode.workspace.getConfiguration).toBeTypeOf("function");
    expect(vscode.commands.registerCommand).toBeTypeOf("function");
    expect(vscode.lm.registerMcpServerDefinitionProvider).toBeTypeOf("function");
    expect(vscode.l10n.t("Hello {0}", "world")).toBe("Hello world");
    expect(vscode.Uri.file("/tmp/demo").fsPath).toBe("/tmp/demo");
  });

  it("keeps status bar and event emitter state in memory", () => {
    const item = vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Right,
      5,
    ) as unknown as MockStatusBarItem;
    item.text = "Connected";
    item.show();
    expect(item.visible).toBe(true);
    item.hide();
    expect(item.visible).toBe(false);

    const emitter = new vscode.EventEmitter<number>();
    const received: number[] = [];
    const listener = emitter.event((value) => received.push(value));
    emitter.fire(3);
    listener.dispose();
    emitter.fire(4);
    expect(received).toEqual([3]);
  });

  it("supports command registration and execution", async () => {
    const registration = vscode.commands.registerCommand("hpa.mock.echo", async (value) => value);
    await expect(vscode.commands.executeCommand("hpa.mock.echo", "ok")).resolves.toBe("ok");
    await expect(vscode.commands.getCommands(true)).resolves.toContain("hpa.mock.echo");
    registration.dispose();
    await expect(vscode.commands.executeCommand("hpa.mock.echo")).rejects.toThrow(
      "command 'hpa.mock.echo' not found",
    );
  });

  it("stores secrets and configuration in memory", async () => {
    const storage = createSecretStorage();
    await storage.store("hpa.token", "abc");
    await expect(storage.get("hpa.token")).resolves.toBe("abc");
    await storage.delete("hpa.token");
    await expect(storage.get("hpa.token")).resolves.toBeUndefined();

    setMockConfiguration({ "hpa.mode": "local" });
    expect(vscode.workspace.getConfiguration("hpa").get("mode")).toBe("local");
  });
});
