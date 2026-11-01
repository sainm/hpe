import * as vscode from "vscode";
import type { HpaServices } from "../contracts";
import type { ExecutionDetail, ExecutionEvent } from "../runtime/types";
import {
  createNonce,
  escapeHtml,
  formatJson,
  metaList,
  renderDocument,
} from "./html";

type PanelMessage =
  | { type: "refresh" }
  | { type: "cancel" }
  | { type: "decide"; decision: "approved" | "rejected" };

interface LoadedExecution {
  detail: ExecutionDetail;
  events: ExecutionEvent[];
}

export async function openExecutionPanel(
  context: vscode.ExtensionContext,
  services: HpaServices,
  executionId: string,
): Promise<void> {
  const panel = vscode.window.createWebviewPanel(
    "hpa.execution",
    services.i18n.t("executions.open"),
    vscode.ViewColumn.Beside,
    {
      enableScripts: true,
      localResourceRoots: [vscode.Uri.joinPath(context.extensionUri, "media")],
    },
  );
  const nonce = createNonce();
  let disposed = false;
  let loading = false;
  let timer: ReturnType<typeof setInterval> | undefined;
  let loaded: LoadedExecution | undefined;
  let errorText: string | undefined;

  function render(): void {
    if (disposed) return;
    panel.webview.html = pageHtml(services, nonce, loaded, errorText);
  }

  function stopTimer(): void {
    if (timer) {
      clearInterval(timer);
      timer = undefined;
    }
  }

  function startTimer(): void {
    if (timer || disposed || !loaded) return;
    if (isTerminal(loaded.detail.summary.status)) return;
    timer = setInterval(() => {
      if (loaded && !isTerminal(loaded.detail.summary.status)) {
        void refresh();
      } else {
        stopTimer();
      }
    }, 2000);
  }

  async function refresh(): Promise<void> {
    if (disposed || loading) return;
    loading = true;
    try {
      const [detail, events] = await Promise.all([
        services.client.getExecution(executionId),
        services.client.listEvents(executionId),
      ]);
      if (disposed) return;
      loaded = { detail, events };
      errorText = undefined;
      render();
      if (isTerminal(detail.summary.status)) {
        stopTimer();
      } else {
        startTimer();
      }
    } catch (error) {
      if (disposed) return;
      errorText = describeError(error);
      render();
    } finally {
      loading = false;
    }
  }

  async function cancel(): Promise<void> {
    const summary = loaded?.detail.summary;
    if (!summary || (summary.status !== "running" && summary.status !== "waiting_approval")) {
      return;
    }
    if (!services.requireTrusted()) {
      errorText = services.i18n.t("trust.required");
      render();
      return;
    }
    const confirmLabel = services.i18n.t("common.confirm");
    const choice = await vscode.window.showWarningMessage(
      services.i18n.t("executions.cancelConfirm", { id: executionId }),
      { modal: true },
      confirmLabel,
      services.i18n.t("common.cancel"),
    );
    if (choice !== confirmLabel) return;
    try {
      await services.client.cancelExecution(executionId);
      if (disposed) return;
      errorText = undefined;
      services.dataEvents.fire("executions");
      void vscode.window.showInformationMessage(services.i18n.t("executions.cancelled"));
      await refresh();
    } catch (error) {
      if (disposed) return;
      errorText = `${services.i18n.t("executions.cancelFailed")} ${describeError(error)}`;
      render();
      void vscode.window.showErrorMessage(errorText);
    }
  }

  async function decide(decision: "approved" | "rejected"): Promise<void> {
    const waiting = loaded?.detail.summary.waiting;
    if (!waiting) {
      await refresh();
      return;
    }
    if (!services.requireTrusted()) {
      errorText = services.i18n.t("trust.required");
      render();
      return;
    }
    try {
      await services.client.decideApproval(executionId, waiting.nodeId, decision, {});
      if (disposed) return;
      errorText = undefined;
      services.dataEvents.fire("executions");
      void vscode.window.showInformationMessage(
        services.i18n.t(
          decision === "approved" ? "executions.approved" : "executions.rejected",
        ),
      );
      await refresh();
    } catch (error) {
      if (disposed) return;
      errorText = `${services.i18n.t("executions.approvalFailed")} ${describeError(error)}`;
      render();
      void vscode.window.showErrorMessage(errorText);
    }
  }

  function handleMessage(raw: unknown): void {
    const message = parseMessage(raw);
    if (!message) return;
    if (message.type === "refresh") {
      void refresh();
      return;
    }
    if (message.type === "cancel") {
      void cancel();
      return;
    }
    void decide(message.decision);
  }

  const messageSubscription = panel.webview.onDidReceiveMessage((message) => {
    handleMessage(message);
  });
  panel.onDidDispose(() => {
    disposed = true;
    stopTimer();
    messageSubscription.dispose();
  });
  render();
  await refresh();
}

function pageHtml(
  services: HpaServices,
  nonce: string,
  loaded: LoadedExecution | undefined,
  errorText: string | undefined,
): string {
  const t = services.i18n;
  const title = t.t("executions.open");
  const body: string[] = [`<h1>${escapeHtml(title)}</h1>`];
  const summary = loaded?.detail.summary;
  const canCancel =
    summary?.status === "running" || summary?.status === "waiting_approval";
  const canDecide = summary?.status === "waiting_approval" && Boolean(summary.waiting);
  body.push('<div class="actions">');
  body.push(
    `<button type="button" data-action="refresh">${escapeHtml(t.t("executions.refresh"))}</button>`,
  );
  if (canCancel) {
    body.push(
      `<button type="button" class="secondary" data-action="cancel">${escapeHtml(t.t("executions.cancel"))}</button>`,
    );
  }
  body.push("</div>");
  if (errorText) {
    body.push(
      `<div class="error" role="alert"><strong>${escapeHtml(t.t("common.error"))}</strong><br>${escapeHtml(errorText)}</div>`,
    );
  }
  if (!loaded) {
    body.push(`<p class="muted">${escapeHtml(t.t("common.loading"))}</p>`);
    return renderDocument({ title, nonce, body: body.join("\n"), script: panelScript() });
  }
  body.push(`<section><h2>${escapeHtml(t.t("executions.summary"))}</h2>`);
  body.push(
    metaList([
      { label: t.t("common.id"), value: summary?.id ?? "" },
      { label: t.t("common.status"), value: summary?.status ?? "" },
      { label: t.t("common.version"), value: summary?.workflowVersion ?? "" },
      { label: t.t("common.time"), value: summary?.startedAt ?? "" },
    ]),
  );
  body.push("</section>");
  if (summary?.error) {
    body.push(
      `<section><h2>${escapeHtml(t.t("common.error"))}</h2><div class="error">${escapeHtml(summary.error.message)}</div></section>`,
    );
  }
  if (summary?.waiting) {
    body.push('<section class="approval">');
    body.push(`<h2>${escapeHtml(t.t("executions.approvalTitle"))}</h2>`);
    body.push(`<p>${escapeHtml(summary.waiting.title)}</p>`);
    body.push(
      `<p class="muted">${escapeHtml(t.t("executions.approvalNode", { node: summary.waiting.nodeId }))}</p>`,
    );
    if (canDecide) {
      body.push('<div class="actions">');
      body.push(
        `<button type="button" data-action="approve">${escapeHtml(t.t("executions.approvalApprove"))}</button>`,
      );
      body.push(
        `<button type="button" class="secondary" data-action="reject">${escapeHtml(t.t("executions.approvalReject"))}</button>`,
      );
      body.push("</div>");
    }
    body.push("</section>");
  }
  const stateText = formatJson(loaded.detail.state) ?? "";
  body.push(
    `<details><summary>${escapeHtml(t.t("executions.state"))}</summary><pre>${escapeHtml(stateText)}</pre></details>`,
  );
  body.push(`<section><h2>${escapeHtml(t.t("executions.events"))}</h2>`);
  const events = [...loaded.events]
    .sort((left, right) => left.sequence - right.sequence)
    .slice(-200);
  if (events.length === 0) {
    body.push(`<p class="muted">${escapeHtml(t.t("executions.noEvents"))}</p>`);
  } else {
    body.push('<div class="table-wrap"><table><thead><tr>');
    body.push(
      `<th>#</th><th>${escapeHtml(t.t("common.time"))}</th><th>${escapeHtml(t.t("common.type"))}</th><th>${escapeHtml(t.t("common.id"))}</th><th>${escapeHtml(t.t("common.details"))}</th>`,
    );
    body.push("</tr></thead><tbody>");
    for (const event of events) {
      body.push("<tr>");
      body.push(`<td>${escapeHtml(event.sequence)}</td>`);
      body.push(`<td>${escapeHtml(event.at)}</td>`);
      body.push(`<td>${escapeHtml(event.kind)}</td>`);
      body.push(`<td>${escapeHtml(event.nodeId ?? "")}</td>`);
      body.push(`<td>${escapeHtml(event.message ?? "")}</td>`);
      body.push("</tr>");
    }
    body.push("</tbody></table></div>");
  }
  body.push("</section>");
  return renderDocument({ title, nonce, body: body.join("\n"), script: panelScript() });
}

function parseMessage(value: unknown): PanelMessage | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const record = value as Record<string, unknown>;
  const keys = Object.keys(record);
  if (keys.length === 1 && record.type === "refresh") return { type: "refresh" };
  if (keys.length === 1 && record.type === "cancel") return { type: "cancel" };
  if (
    keys.length === 2 &&
    record.type === "decide" &&
    (record.decision === "approved" || record.decision === "rejected")
  ) {
    return { type: "decide", decision: record.decision };
  }
  return undefined;
}

function isTerminal(status: string): boolean {
  return status === "succeeded" || status === "failed" || status === "cancelled";
}

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function panelScript(): string {
  return `(function () {
  const vscode = acquireVsCodeApi();
  document.addEventListener("click", function (event) {
    const target = event.target;
    if (!target || typeof target.closest !== "function") return;
    const button = target.closest("button[data-action]");
    if (!button) return;
    const action = button.getAttribute("data-action");
    if (action === "refresh") {
      vscode.postMessage({ type: "refresh" });
      return;
    }
    if (action === "cancel") {
      vscode.postMessage({ type: "cancel" });
      return;
    }
    if (action === "approve") {
      vscode.postMessage({ type: "decide", decision: "approved" });
      return;
    }
    if (action === "reject") {
      vscode.postMessage({ type: "decide", decision: "rejected" });
    }
  });
}());`;
}
