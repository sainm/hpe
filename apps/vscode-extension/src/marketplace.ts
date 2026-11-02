import * as vscode from "vscode";
import type { HpaServices } from "./contracts";
import { RuntimeApiError } from "./runtime/client";
import type {
  MarketplacePublisher,
  MarketplaceSource,
  RemoteMarketplacePackage,
} from "./runtime/types";
import type { HpaTreeItem } from "./views/common";

type ManageAction =
  | "addSource"
  | "refreshSource"
  | "rotateSourceKey"
  | "addPublisher"
  | "addPublisherKey"
  | "revokePublisherKey"
  | "installPackage"
  | "scanPackage";

export function registerMarketplaceCommands(services: HpaServices): vscode.Disposable[] {
  return [
    vscode.commands.registerCommand("hpa.marketplace.manage", () =>
      manageMarketplace(services),
    ),
    vscode.commands.registerCommand(
      "hpa.marketplace.refreshSource",
      async (argument?: unknown) => {
        const source =
          sourceFrom(argument) ??
          (await pickSource(services, services.i18n.t("plugins.refreshSource")));
        if (!source) return;
        await runWrite(services, async () => {
          await services.client.refreshMarketplaceSource(source.id);
          changed(services);
          void vscode.window.showInformationMessage(
            services.i18n.t("plugins.sourceRefreshed", { id: source.id }),
          );
        });
      },
    ),
    vscode.commands.registerCommand(
      "hpa.marketplace.scan",
      async (argument?: unknown) => {
        const entry =
          packageFrom(argument) ??
          (await pickRemotePackage(services, services.i18n.t("plugins.scanVulnerabilities")));
        if (!entry) return;
        await scanPackage(services, entry);
      },
    ),
    vscode.commands.registerCommand(
      "hpa.marketplace.install",
      async (argument?: unknown) => {
        const entry =
          packageFrom(argument) ??
          (await pickRemotePackage(
            services,
            services.i18n.t("plugins.installRemote"),
            true,
          ));
        if (!entry) return;
        await installPackage(services, entry);
      },
    ),
  ];
}

async function manageMarketplace(services: HpaServices): Promise<void> {
  const actions: { label: string; description: string; action: ManageAction }[] = [
    {
      label: services.i18n.t("plugins.addSource"),
      description: services.i18n.t("plugins.addSourceHint"),
      action: "addSource",
    },
    {
      label: services.i18n.t("plugins.refreshSource"),
      description: services.i18n.t("plugins.refreshSourceHint"),
      action: "refreshSource",
    },
    {
      label: services.i18n.t("plugins.rotateSourceKey"),
      description: services.i18n.t("plugins.rotateSourceKeyHint"),
      action: "rotateSourceKey",
    },
    {
      label: services.i18n.t("plugins.addPublisher"),
      description: services.i18n.t("plugins.addPublisherHint"),
      action: "addPublisher",
    },
    {
      label: services.i18n.t("plugins.addPublisherKey"),
      description: services.i18n.t("plugins.addPublisherKeyHint"),
      action: "addPublisherKey",
    },
    {
      label: services.i18n.t("plugins.revokePublisherKey"),
      description: services.i18n.t("plugins.revokePublisherKeyHint"),
      action: "revokePublisherKey",
    },
    {
      label: services.i18n.t("plugins.installRemote"),
      description: services.i18n.t("plugins.installRemoteHint"),
      action: "installPackage",
    },
    {
      label: services.i18n.t("plugins.scanVulnerabilities"),
      description: services.i18n.t("plugins.scanVulnerabilitiesHint"),
      action: "scanPackage",
    },
  ];
  const selected = await vscode.window.showQuickPick(actions, {
    title: services.i18n.t("plugins.manageRemote"),
    matchOnDescription: true,
  });
  if (!selected) return;
  switch (selected.action) {
    case "addSource":
      return addSource(services);
    case "refreshSource":
      return executeCommand("hpa.marketplace.refreshSource");
    case "rotateSourceKey":
      return rotateSourceKey(services);
    case "addPublisher":
      return addPublisher(services);
    case "addPublisherKey":
      return addPublisherKey(services);
    case "revokePublisherKey":
      return revokePublisherKey(services);
    case "installPackage":
      return executeCommand("hpa.marketplace.install");
    case "scanPackage":
      return executeCommand("hpa.marketplace.scan");
  }
}

async function addSource(services: HpaServices): Promise<void> {
  const id = await requiredInput(services, services.i18n.t("plugins.sourceId"));
  if (!id) return;
  const label = await requiredInput(services, services.i18n.t("plugins.sourceLabel"));
  if (!label) return;
  const indexUri = await requiredInput(services, services.i18n.t("plugins.indexUri"));
  if (!indexUri) return;
  const keyId = await requiredInput(services, services.i18n.t("plugins.indexKeyId"));
  if (!keyId) return;
  const publicKey = await requiredInput(services, services.i18n.t("plugins.publicKey"));
  if (!publicKey) return;
  const keyLabel =
    (await optionalInput(services, services.i18n.t("plugins.keyLabel"))) || "primary";
  await runWrite(services, async () => {
    await services.client.addMarketplaceSource({
      id,
      label,
      indexUri,
      keyId,
      publicKey,
      keyLabel,
    });
    changed(services);
    void vscode.window.showInformationMessage(
      services.i18n.t("plugins.sourceAdded", { id }),
    );
  });
}

async function rotateSourceKey(services: HpaServices): Promise<void> {
  const source = await pickSource(services, services.i18n.t("plugins.rotateSourceKey"));
  if (!source) return;
  const keyId = await requiredInput(services, services.i18n.t("plugins.indexKeyId"));
  if (!keyId) return;
  const publicKey = await requiredInput(services, services.i18n.t("plugins.publicKey"));
  if (!publicKey) return;
  const label =
    (await optionalInput(services, services.i18n.t("plugins.keyLabel"))) || "rotated";
  await runWrite(services, async () => {
    await services.client.rotateMarketplaceSourceKey(source.id, {
      keyId,
      publicKey,
      label,
    });
    changed(services);
    void vscode.window.showInformationMessage(
      services.i18n.t("plugins.sourceKeyRotated", { id: source.id }),
    );
  });
}

async function addPublisher(services: HpaServices): Promise<void> {
  const id = await requiredInput(services, services.i18n.t("plugins.publisherId"));
  if (!id) return;
  const name = await requiredInput(services, services.i18n.t("plugins.publisherName"));
  if (!name) return;
  await runWrite(services, async () => {
    await services.client.addMarketplacePublisher({ id, name });
    changed(services);
    void vscode.window.showInformationMessage(
      services.i18n.t("plugins.publisherAdded", { id }),
    );
  });
}

async function addPublisherKey(services: HpaServices): Promise<void> {
  const publisher = await pickPublisher(
    services,
    services.i18n.t("plugins.addPublisherKey"),
  );
  if (!publisher) return;
  const keyId = await requiredInput(services, services.i18n.t("plugins.publisherKeyId"));
  if (!keyId) return;
  const publicKey = await requiredInput(services, services.i18n.t("plugins.publicKey"));
  if (!publicKey) return;
  const label =
    (await optionalInput(services, services.i18n.t("plugins.keyLabel"))) || "primary";
  const validFrom = await optionalInput(services, services.i18n.t("plugins.validFrom"));
  const validUntil = await optionalInput(services, services.i18n.t("plugins.validUntil"));
  await runWrite(services, async () => {
    await services.client.addMarketplacePublisherKey(publisher.id, {
      keyId,
      publicKey,
      label,
      validFrom: validFrom || undefined,
      validUntil: validUntil || undefined,
    });
    changed(services);
    void vscode.window.showInformationMessage(
      services.i18n.t("plugins.publisherKeyAdded", { id: keyId }),
    );
  });
}

async function revokePublisherKey(services: HpaServices): Promise<void> {
  const publisher = await pickPublisher(
    services,
    services.i18n.t("plugins.revokePublisherKey"),
  );
  if (!publisher) return;
  const activeKeys = publisher.keys.filter((key) => !key.revokedAt);
  const selected = await vscode.window.showQuickPick(
    activeKeys.map((key) => ({
      label: key.keyId,
      description: key.label,
      key,
    })),
    { title: services.i18n.t("plugins.revokePublisherKey") },
  );
  if (!selected) return;
  const confirmation = await vscode.window.showWarningMessage(
    services.i18n.t("plugins.revokePublisherKeyConfirm", {
      id: selected.key.keyId,
      publisher: publisher.id,
    }),
    { modal: true },
    services.i18n.t("common.confirm"),
    services.i18n.t("common.cancel"),
  );
  if (confirmation !== services.i18n.t("common.confirm")) return;
  await runWrite(services, async () => {
    await services.client.revokeMarketplacePublisherKey(
      publisher.id,
      selected.key.keyId,
    );
    changed(services);
    void vscode.window.showInformationMessage(
      services.i18n.t("plugins.publisherKeyRevoked", { id: selected.key.keyId }),
    );
  });
}

async function installPackage(
  services: HpaServices,
  entry: RemoteMarketplacePackage,
): Promise<void> {
  if (!services.requireTrusted()) return;
  const confirmation = await vscode.window.showWarningMessage(
    services.i18n.t("plugins.installRemoteConfirm", {
      package: `${entry.pluginId}@${entry.version}`,
    }),
    { modal: true },
    services.i18n.t("common.confirm"),
    services.i18n.t("common.cancel"),
  );
  if (confirmation !== services.i18n.t("common.confirm")) return;
  await runWrite(services, async () => {
    await services.client.installRemoteMarketplacePackage(
      entry.sourceId,
      entry.pluginId,
      entry.version,
    );
    changed(services);
    void vscode.window.showInformationMessage(
      services.i18n.t("plugins.installed", {
        package: `${entry.pluginId}@${entry.version}`,
      }),
    );
  });
}

async function scanPackage(
  services: HpaServices,
  entry: RemoteMarketplacePackage,
): Promise<void> {
  try {
    services.requireOnline();
    const report = await services.client.marketplaceVulnerability(
      entry.pluginId,
      entry.version,
    );
    const lines = [
      `${entry.pluginId}@${entry.version}`,
      `${services.i18n.t("common.status")}: ${report.status}`,
      `${services.i18n.t("plugins.vulnerabilitySource")}: ${report.source}`,
      `${services.i18n.t("plugins.vulnerabilities")}: ${report.vulnerabilities.map((item) => item.id).join(", ") || services.i18n.t("common.never")}`,
      `${services.i18n.t("plugins.provenance")}: ${entry.provenance?.kind ?? services.i18n.t("common.never")}`,
      `${services.i18n.t("plugins.ociDigest")}: ${entry.oci?.digest ?? services.i18n.t("common.never")}`,
    ];
    services.output.appendLine(`[marketplace] ${lines.join(" | ")}`);
    if (report.status === "vulnerable") {
      void vscode.window.showWarningMessage(
        services.i18n.t("plugins.vulnerablePackage", {
          package: `${entry.pluginId}@${entry.version}`,
          count: report.vulnerabilities.length,
        }),
      );
    } else {
      void vscode.window.showInformationMessage(lines.slice(0, 3).join(" · "));
    }
  } catch (error) {
    showError(services, error, services.i18n.t("plugins.scanFailed"));
  }
}

async function pickSource(
  services: HpaServices,
  title: string,
): Promise<MarketplaceSource | undefined> {
  try {
    services.requireOnline();
    const sources = await services.client.listMarketplaceSources();
    const selected = await vscode.window.showQuickPick(
      sources.map((source) => ({
        label: source.label,
        description: source.id,
        detail: source.indexUri,
        source,
      })),
      { title, matchOnDescription: true, matchOnDetail: true },
    );
    return selected?.source;
  } catch (error) {
    showError(services, error, services.i18n.t("view.error"));
    return undefined;
  }
}

async function pickPublisher(
  services: HpaServices,
  title: string,
): Promise<MarketplacePublisher | undefined> {
  try {
    services.requireOnline();
    const publishers = await services.client.listMarketplacePublishers();
    const selected = await vscode.window.showQuickPick(
      publishers.map((publisher) => ({
        label: publisher.name,
        description: publisher.id,
        detail: `${publisher.status} · ${publisher.keys.length}`,
        publisher,
      })),
      { title, matchOnDescription: true, matchOnDetail: true },
    );
    return selected?.publisher;
  } catch (error) {
    showError(services, error, services.i18n.t("view.error"));
    return undefined;
  }
}

async function pickRemotePackage(
  services: HpaServices,
  title: string,
  installableOnly = false,
): Promise<RemoteMarketplacePackage | undefined> {
  try {
    services.requireOnline();
    const packages = (await services.client.listRemoteMarketplace()).filter(
      (entry) =>
        !installableOnly ||
        (entry.reviewStatus === "approved" && !entry.withdrawnAt),
    );
    const selected = await vscode.window.showQuickPick(
      packages.map((entry) => ({
        label: `${entry.pluginId}@${entry.version}`,
        description: entry.sourceId,
        detail: [
          entry.reviewStatus,
          entry.provenance ? "Sigstore" : undefined,
          entry.oci ? "OCI" : undefined,
        ]
          .filter(Boolean)
          .join(" · "),
        entry,
      })),
      { title, matchOnDescription: true, matchOnDetail: true },
    );
    return selected?.entry;
  } catch (error) {
    showError(services, error, services.i18n.t("view.error"));
    return undefined;
  }
}

async function requiredInput(
  services: HpaServices,
  prompt: string,
): Promise<string | undefined> {
  const value = await vscode.window.showInputBox({
    prompt,
    ignoreFocusOut: true,
    validateInput: (input) =>
      input.trim() === "" ? services.i18n.t("plugins.valueRequired") : undefined,
  });
  return value?.trim();
}

async function optionalInput(
  services: HpaServices,
  prompt: string,
): Promise<string | undefined> {
  return (
    await vscode.window.showInputBox({
      prompt,
      ignoreFocusOut: true,
    })
  )?.trim();
}

async function runWrite(
  services: HpaServices,
  operation: () => Promise<void>,
): Promise<void> {
  if (!services.requireTrusted()) return;
  try {
    services.requireOnline();
    await operation();
  } catch (error) {
    showError(services, error, services.i18n.t("plugins.operationFailed"));
  }
}

function changed(services: HpaServices): void {
  services.dataEvents.fire("plugins");
}

function sourceFrom(value: unknown): MarketplaceSource | undefined {
  return (value as HpaTreeItem | undefined)?.marketplaceSource;
}

function packageFrom(value: unknown): RemoteMarketplacePackage | undefined {
  return (value as HpaTreeItem | undefined)?.remotePackage;
}

async function executeCommand(command: string): Promise<void> {
  await vscode.commands.executeCommand(command);
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
        ? error.message
        : String(error);
  services.output.appendLine(`[marketplace] ${message}`);
  void vscode.window.showErrorMessage(`${fallback}: ${message}`);
}
