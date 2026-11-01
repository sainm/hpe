import * as vscode from "vscode";
import type { HpaServices } from "../contracts";
import type { Artifact, ArtifactDetail } from "../runtime/types";
import {
  createNonce,
  escapeHtml,
  formatJson,
  metaList,
  renderDocument,
} from "./html";

export async function openArtifactPanel(
  context: vscode.ExtensionContext,
  services: HpaServices,
  artifactId: string,
): Promise<void> {
  const panel = vscode.window.createWebviewPanel(
    "hpa.artifact",
    services.i18n.t("artifacts.preview"),
    vscode.ViewColumn.Beside,
    {
      enableScripts: true,
      localResourceRoots: [vscode.Uri.joinPath(context.extensionUri, "media")],
    },
  );
  const nonce = createNonce();
  let disposed = false;
  panel.onDidDispose(() => {
    disposed = true;
  });
  try {
    const detail = await services.client.getArtifact(artifactId);
    if (disposed) return;
    panel.webview.html = pageHtml(services, nonce, detail);
  } catch (error) {
    if (disposed) return;
    panel.webview.html = errorHtml(services, nonce, error);
  }
}

function pageHtml(services: HpaServices, nonce: string, detail: ArtifactDetail): string {
  const t = services.i18n;
  const artifact = detail.artifact;
  const content = formatJson(detail.content);
  const previewable = content !== undefined && isPreviewable(artifact);
  const body: string[] = [
    `<h1>${escapeHtml(artifact.name)}</h1>`,
    metaList([
      { label: t.t("common.name"), value: artifact.name },
      {
        label: t.t("common.type"),
        value: `${artifact.kind} · ${artifact.mimeType}`,
      },
      { label: t.t("common.size"), value: formatSize(artifact.size) },
      { label: t.t("common.time"), value: artifact.createdAt },
      { label: t.t("common.id"), value: artifact.id },
    ]),
    `<h2>${escapeHtml(t.t("common.details"))}</h2>`,
  ];
  if (previewable && content !== undefined) {
    body.push(`<pre>${escapeHtml(content)}</pre>`);
  } else {
    body.push(`<p class="muted">${escapeHtml(t.t("artifacts.notPreviewable"))}</p>`);
  }
  return renderDocument({
    title: t.t("artifacts.preview"),
    nonce,
    body: body.join("\n"),
  });
}

function errorHtml(services: HpaServices, nonce: string, error: unknown): string {
  const t = services.i18n;
  const body = [
    `<h1>${escapeHtml(t.t("artifacts.preview"))}</h1>`,
    `<div class="error" role="alert"><strong>${escapeHtml(t.t("artifacts.openFailed"))}</strong><br>${escapeHtml(describeError(error))}</div>`,
  ].join("\n");
  return renderDocument({
    title: t.t("artifacts.preview"),
    nonce,
    body,
  });
}

function isPreviewable(artifact: Artifact): boolean {
  const mime = artifact.mimeType.toLowerCase();
  const kind = artifact.kind.toLowerCase();
  if (mime === "application/json" || mime.endsWith("+json")) return true;
  if (mime.startsWith("text/")) return true;
  if (mime.includes("yaml") || mime.includes("xml")) return true;
  return /json|text|markdown|yaml|xml|csv|report|table/.test(kind);
}

function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return String(bytes);
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(1)} ${units[index] ?? "KB"}`;
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
