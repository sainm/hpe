import * as vscode from "vscode";
import type { HpaServices } from "../contracts";
import type {
  MarketplacePublisher,
  MarketplaceSource,
  PluginSummary,
  RemoteMarketplacePackage,
} from "../runtime/types";
import {
  actionItem,
  emptyItem,
  fieldItem,
  groupItem,
  treeItem,
  ViewProvider,
  type HpaTreeItem,
} from "./common";

export class PluginsProvider extends ViewProvider {
  private marketplaceMonitorStarted = false;
  private marketplaceAbort?: AbortController;
  private marketplaceSequence: number | null = null;

  constructor(services: HpaServices) {
    super(services);
  }

  protected async load(): Promise<HpaTreeItem[]> {
    this.startMarketplaceMonitor();
    const [plugins, sources, publishers, packages] = await Promise.all([
      this.services.client.listPlugins(),
      this.services.client.listMarketplaceSources(),
      this.services.client.listMarketplacePublishers(),
      this.services.client.listRemoteMarketplace(),
    ]);
    if (plugins.length === 0 && sources.length === 0 && publishers.length === 0) {
      return [emptyItem(this.services)];
    }
    const items: HpaTreeItem[] = [];
    if (plugins.length > 0) {
      items.push(
        groupItem(
          this.services.i18n.t("plugins.installedGroup"),
          "package",
          plugins.map((plugin) => this.pluginItem(plugin)),
          String(plugins.length),
        ),
      );
    }
    if (sources.length > 0) {
      items.push(
        groupItem(
          this.services.i18n.t("plugins.remoteSources"),
          "cloud",
          sources.map((source) =>
            this.sourceItem(
              source,
              packages.filter((entry) => entry.sourceId === source.id),
            ),
          ),
          String(sources.length),
        ),
      );
    }
    if (publishers.length > 0) {
      items.push(
        groupItem(
          this.services.i18n.t("plugins.publishers"),
          "verified",
          publishers.map((publisher) => this.publisherItem(publisher)),
          String(publishers.length),
        ),
      );
    }
    items.push(
      actionItem(
        this.services.i18n.t("plugins.manageRemote"),
        "gear",
        "hpa.marketplace.manage",
        this.services.i18n.t("plugins.manageRemote"),
      ),
    );
    return items;
  }

  override dispose(): void {
    this.marketplaceAbort?.abort();
    super.dispose();
  }

  private startMarketplaceMonitor(): void {
    if (this.marketplaceMonitorStarted) return;
    this.marketplaceMonitorStarted = true;
    this.marketplaceAbort = new AbortController();
    void this.monitorMarketplace();
  }

  private async monitorMarketplace(): Promise<void> {
    const signal = this.marketplaceAbort?.signal;
    if (!signal) return;
    while (!signal.aborted) {
      try {
        await this.services.client.streamMarketplaceEvents(
          this.marketplaceSequence,
          (event) => {
            this.marketplaceSequence = Math.max(
              this.marketplaceSequence ?? 0,
              event.sequence,
            );
            if (!event.kind.includes("withdrawn")) return;
            this.services.output.appendLine(
              `[marketplace] ${event.kind}: ${event.pluginId ?? ""}@${event.version ?? ""}`,
            );
            this.services.dataEvents.fire("plugins");
            void vscode.window.showWarningMessage(
              this.services.i18n.t("plugins.withdrawnPush", {
                plugin: `${event.pluginId ?? ""}@${event.version ?? ""}`,
                reason: String((event.detail as { reason?: string } | null)?.reason ?? ""),
              }),
            );
          },
          signal,
          (sequence) => {
            this.marketplaceSequence = Math.max(
              this.marketplaceSequence ?? 0,
              sequence,
            );
          },
        );
      } catch (error) {
        if (signal.aborted) return;
        this.services.output.appendLine(
          `[marketplace] event stream disconnected: ${
            error instanceof Error ? error.message : String(error)
          }`,
        );
      }
      if (!signal.aborted) {
        await new Promise((resolve) => setTimeout(resolve, 2000));
      }
    }
  }

  private pluginItem(plugin: PluginSummary): HpaTreeItem {
    const i18n = this.services.i18n;
    const installed = plugin.installedVersion ?? i18n.t("common.never");
    const item = treeItem(plugin.name);
    item.description = `${i18n.t("skills.installed")} ${installed} · ${i18n.t("plugins.version")} ${plugin.latestVersion} · ${i18n.t("plugins.trust")} ${plugin.trust}`;
    item.iconPath = new vscode.ThemeIcon("package");
    item.contextValue = "hpa.plugin";
    item.tooltip = [
      `${i18n.t("common.name")}: ${plugin.name}`,
      `${i18n.t("common.id")}: ${plugin.id}`,
      `${i18n.t("skills.installed")}: ${installed}`,
      `${i18n.t("plugins.version")}: ${plugin.latestVersion}`,
      `${i18n.t("plugins.trust")}: ${plugin.trust}`,
      `${i18n.t("common.status")}: ${plugin.status}`,
      `${i18n.t("common.time")}: ${plugin.updatedAt}`,
    ].join("\n");
    item.plugin = plugin;
    item.command = {
      command: "hpa.web.open",
      title: i18n.t("plugins.open"),
      arguments: ["/plugins"],
    };
    return item;
  }

  private sourceItem(source: MarketplaceSource, packages: RemoteMarketplacePackage[]): HpaTreeItem {
    const i18n = this.services.i18n;
    const item = treeItem(source.label, vscode.TreeItemCollapsibleState.Collapsed);
    item.description = `seq ${source.lastSequence} · ${source.packageCount} ${i18n.t("plugins.packages")}`;
    item.tooltip = [
      `${i18n.t("common.id")}: ${source.id}`,
      `${i18n.t("plugins.indexUri")}: ${source.indexUri}`,
      `${i18n.t("common.status")}: ${source.enabled ? i18n.t("plugins.enabled") : i18n.t("plugins.disabled")}`,
      `${i18n.t("plugins.keys")}: ${source.keys.map((key) => key.keyId).join(", ") || i18n.t("common.never")}`,
    ].join("\n");
    item.iconPath = new vscode.ThemeIcon(source.enabled ? "cloud" : "cloud-offline");
    item.contextValue = "hpa.marketplace.source";
    item.marketplaceSource = source;
    item.children = [
      ...packages.map((entry) => this.remotePackageItem(source, entry)),
      actionItem(
        i18n.t("plugins.refreshSource"),
        "refresh",
        "hpa.marketplace.refreshSource",
        i18n.t("plugins.refreshSource"),
        [item],
      ),
    ];
    return item;
  }

  private remotePackageItem(
    source: MarketplaceSource,
    entry: RemoteMarketplacePackage,
  ): HpaTreeItem {
    const i18n = this.services.i18n;
    const item = treeItem(`${entry.pluginId}@${entry.version}`);
    const withdrawn = Boolean(entry.withdrawnAt);
    item.description = [
      withdrawn ? i18n.t("plugins.withdrawn") : entry.reviewStatus,
      entry.provenance ? i18n.t("plugins.sigstore") : undefined,
      entry.oci ? i18n.t("plugins.oci") : undefined,
    ]
      .filter(Boolean)
      .join(" · ");
    item.tooltip = [
      `${i18n.t("plugins.source")}: ${source.label}`,
      `${i18n.t("plugins.publisher")}: ${entry.publisherId}`,
      `${i18n.t("plugins.signer")}: ${entry.signerKeyId}`,
      `${i18n.t("plugins.packageSha")}: ${entry.packageSha256}`,
      `${i18n.t("plugins.provenance")}: ${entry.provenance?.kind ?? i18n.t("common.never")}`,
      `${i18n.t("plugins.ociDigest")}: ${entry.oci?.digest ?? i18n.t("common.never")}`,
    ].join("\n");
    item.iconPath = new vscode.ThemeIcon(withdrawn ? "warning" : "package");
    item.contextValue = "hpa.marketplace.package";
    item.remotePackage = entry;
    item.command = {
      command: "hpa.marketplace.scan",
      title: i18n.t("plugins.scanVulnerabilities"),
      arguments: [item],
    };
    return item;
  }

  private publisherItem(publisher: MarketplacePublisher): HpaTreeItem {
    const i18n = this.services.i18n;
    const item = treeItem(publisher.name, vscode.TreeItemCollapsibleState.Collapsed);
    item.description = `${publisher.status} · ${publisher.keys.length} ${i18n.t("plugins.keys")}`;
    item.tooltip = [
      `${i18n.t("common.id")}: ${publisher.id}`,
      `${i18n.t("common.status")}: ${publisher.status}`,
    ].join("\n");
    item.iconPath = new vscode.ThemeIcon("verified");
    item.contextValue = "hpa.marketplace.publisher";
    item.marketplacePublisher = publisher;
    item.children = publisher.keys.map((key) =>
      fieldItem(
        this.services,
        key.keyId,
        key.revokedAt ? i18n.t("plugins.revoked") : key.label,
        key.revokedAt ? "error" : "key",
      ),
    );
    return item;
  }
}
