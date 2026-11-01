import { randomBytes } from "crypto";

export interface DocumentOptions {
  title: string;
  nonce: string;
  body: string;
  script?: string;
}

export interface MetaRow {
  label: string;
  value: string;
}

export function createNonce(): string {
  return randomBytes(16).toString("base64");
}

export function escapeHtml(value: unknown): string {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

export function formatJson(value: unknown): string | undefined {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return undefined;
  }
}

export function metaList(rows: MetaRow[]): string {
  const body = rows
    .map(
      (row) =>
        `<div class="meta-row"><dt>${escapeHtml(row.label)}</dt><dd>${escapeHtml(row.value)}</dd></div>`,
    )
    .join("");
  return `<dl class="meta">${body}</dl>`;
}

export function renderDocument(options: DocumentOptions): string {
  const nonce = escapeHtml(options.nonce);
  const script = options.script
    ? `<script nonce="${nonce}">${options.script}</script>`
    : "";
  return `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'nonce-${nonce}'; script-src 'nonce-${nonce}';">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>${escapeHtml(options.title)}</title>
<style nonce="${nonce}">
body {
  margin: 0;
  padding: 16px 20px 32px;
  color: var(--vscode-foreground);
  background: var(--vscode-editor-background);
  font-family: var(--vscode-font-family);
  font-size: var(--vscode-font-size);
  line-height: 1.5;
}
h1, h2, h3 {
  font-weight: 600;
  margin: 0 0 10px;
}
h1 { font-size: 1.45em; }
h2 { font-size: 1.15em; margin-top: 22px; }
p { margin: 8px 0; }
section { margin-bottom: 18px; }
.actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin: 12px 0;
}
button {
  color: var(--vscode-button-foreground);
  background: var(--vscode-button-background);
  border: 1px solid transparent;
  border-radius: 2px;
  padding: 5px 12px;
  cursor: pointer;
  font-family: inherit;
  font-size: inherit;
}
button:hover { background: var(--vscode-button-hoverBackground); }
button.secondary {
  color: var(--vscode-button-secondaryForeground);
  background: var(--vscode-button-secondaryBackground);
}
button.secondary:hover { background: var(--vscode-button-secondaryHoverBackground); }
button:focus-visible {
  outline: 1px solid var(--vscode-focusBorder);
  outline-offset: 2px;
}
.meta {
  display: grid;
  grid-template-columns: max-content minmax(0, 1fr);
  gap: 6px 14px;
  margin: 0 0 14px;
}
.meta-row { display: contents; }
.meta dt {
  color: var(--vscode-descriptionForeground);
  font-weight: 600;
}
.meta dd {
  margin: 0;
  overflow-wrap: anywhere;
}
.badge {
  display: inline-block;
  border: 1px solid var(--vscode-panel-border);
  border-radius: 2px;
  padding: 1px 7px;
  font-size: 0.9em;
  background: var(--vscode-badge-background);
  color: var(--vscode-badge-foreground);
}
.error {
  border-left: 3px solid var(--vscode-errorForeground);
  background: var(--vscode-inputValidation-errorBackground);
  color: var(--vscode-inputValidation-errorForeground);
  padding: 8px 12px;
  margin: 12px 0;
  overflow-wrap: anywhere;
}
.approval {
  border: 1px solid var(--vscode-focusBorder);
  border-radius: 3px;
  padding: 12px 14px;
  margin: 12px 0 18px;
  background: var(--vscode-editorWidget-background);
}
.muted { color: var(--vscode-descriptionForeground); }
table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.95em;
}
th, td {
  text-align: left;
  vertical-align: top;
  padding: 6px 8px;
  border-bottom: 1px solid var(--vscode-panel-border);
  overflow-wrap: anywhere;
}
th {
  color: var(--vscode-descriptionForeground);
  font-weight: 600;
  position: sticky;
  top: 0;
  background: var(--vscode-editor-background);
}
code, pre {
  font-family: var(--vscode-editor-font-family);
  font-size: var(--vscode-editor-font-size);
}
pre {
  margin: 8px 0;
  padding: 12px;
  background: var(--vscode-textCodeBlock-background);
  border: 1px solid var(--vscode-panel-border);
  border-radius: 2px;
  overflow: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
details { margin: 10px 0; }
summary { cursor: pointer; font-weight: 600; }
.table-wrap { overflow-x: auto; }
</style>
</head>
<body>
${options.body}
${script}
</body>
</html>`;
}
