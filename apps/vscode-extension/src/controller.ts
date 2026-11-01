import * as vscode from "vscode";
import {
  DEFAULT_LOCAL_ENDPOINT,
  deriveMcpEndpoint,
  normalizeEndpoint,
  readConfig,
  resolveEndpoints,
  validateEndpoint,
  type HpaConfig,
} from "./config";
import type { HpaServices, InstalledSkillRecord, RuntimeDataEvents } from "./contracts";
import { I18n, localeFromLanguage } from "./i18n";
import { RuntimeClient } from "./runtime/client";
import type { RuntimeState } from "./runtime/types";

const API_TOKEN_KEY = "hpa.api.token";
const MCP_TOKEN_KEY = "hpa.mcp.token";
const MCP_TOKEN_ID_KEY = "hpa.mcp.tokenId";
const INSTALLED_SKILLS_KEY = "hpa.installedSkills";
const HEALTH_INTERVAL_MS = 10000;

class DataEvents implements RuntimeDataEvents {
  private readonly emitter = new vscode.EventEmitter<string | undefined>();

  onDidChange(listener: (scope?: string) => void): vscode.Disposable {
    return this.emitter.event(listener);
  }

  fire(scope?: string): void {
    this.emitter.fire(scope);
  }

  dispose(): void {
    this.emitter.dispose();
  }
}

export class RuntimeController implements HpaServices, vscode.Disposable {
  readonly client: RuntimeClient;
  readonly output: vscode.OutputChannel;
  readonly secrets: vscode.SecretStorage;
  readonly extensionUri: vscode.Uri;
  readonly dataEvents = new DataEvents();
  private readonly runtimeEmitter = new vscode.EventEmitter<RuntimeState>();
  private readonly statusBar: vscode.StatusBarItem;
  private readonly disposables: vscode.Disposable[] = [];
  private readonly context: vscode.ExtensionContext;
  private translations: I18n;
  private state: RuntimeState = {
    status: "connecting",
    checkedAt: new Date().toISOString(),
  };
  private timer?: NodeJS.Timeout;
  private activeMcpEndpoint = "";
  private mcpTokenCache?: string;
  private disposed = false;

  constructor(context: vscode.ExtensionContext) {
    this.context = context;
    this.extensionUri = context.extensionUri;
    this.secrets = context.secrets;
    this.output = vscode.window.createOutputChannel("hpa");
    this.translations = new I18n(
      localeFromLanguage(vscode.env.language, readConfig(config()).locale),
    );
    this.client = new RuntimeClient({
      baseUrl: readConfig(config()).localEndpoint || DEFAULT_LOCAL_ENDPOINT,
      tokenProvider: () => this.apiToken(),
    });
    this.statusBar = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
    this.statusBar.command = "hpa.runtime.status";
    this.disposables.push(
      this.output,
      this.statusBar,
      this.runtimeEmitter,
      this.dataEvents,
      vscode.workspace.onDidChangeConfiguration((event) => {
        if (event.affectsConfiguration("hpa")) void this.applyConfiguration();
      }),
      vscode.workspace.onDidGrantWorkspaceTrust(() => this.dataEvents.fire()),
      vscode.workspace.onDidChangeWorkspaceFolders(() => this.dataEvents.fire()),
    );
    this.renderStatus();
  }

  get i18n(): I18n {
    return this.translations;
  }

  config(): HpaConfig {
    return readConfig(config());
  }

  runtime(): RuntimeState {
    return this.state;
  }

  onDidChangeRuntime(listener: (state: RuntimeState) => void): vscode.Disposable {
    return this.runtimeEmitter.event(listener);
  }

  brandName(): string {
    return this.state.health?.brand?.productName || "hpa";
  }

  webUrl(path: string): string {
    const base = normalizeEndpoint(this.client.endpoint);
    const target = path.startsWith("/") ? path : `/${path}`;
    return `${base}${target}`;
  }

  async openWeb(path: string): Promise<void> {
    try {
      await vscode.env.openExternal(vscode.Uri.parse(this.webUrl(path)));
    } catch (error) {
      void vscode.window.showErrorMessage(
        `${this.i18n.t("web.openFailed")} ${errorMessage(error)}`,
      );
    }
  }

  isTrusted(): boolean {
    return vscode.workspace.isTrusted;
  }

  requireTrusted(): boolean {
    if (this.isTrusted()) return true;
    void vscode.window.showWarningMessage(this.i18n.t("trust.required"));
    return false;
  }

  workspaceRoot(): vscode.Uri | undefined {
    return vscode.workspace.workspaceFolders?.[0]?.uri;
  }

  mcpEndpoint(): string {
    if (this.activeMcpEndpoint) return this.activeMcpEndpoint;
    return resolveEndpoints(this.config()).mcpEndpoint;
  }

  async refreshRuntime(): Promise<RuntimeState> {
    if (this.disposed) return this.state;
    if (this.state.status !== "online") {
      this.setState({ status: "connecting", health: this.state.health, checkedAt: now() });
    }
    const config = this.config();
    const candidates = healthCandidates(config);
    let lastError = "endpoint-not-configured";
    for (const candidate of candidates) {
      this.client.setBaseUrl(candidate.apiEndpoint);
      try {
        const health = await this.client.health();
        if (health.internalId !== "hpa") {
          lastError = "runtime-identity-mismatch";
          continue;
        }
        this.activeMcpEndpoint = candidate.mcpEndpoint;
        this.setState({
          status: "online",
          health,
          checkedAt: now(),
        });
        this.output.appendLine(
          `connected ${candidate.apiEndpoint} version=${health.version} runtime=${health.runtime}`,
        );
        this.dataEvents.fire();
        return this.state;
      } catch (error) {
        lastError = error instanceof Error ? error.message : String(error);
        this.output.appendLine(`health failed ${candidate.apiEndpoint}: ${lastError}`);
      }
    }
    this.activeMcpEndpoint = "";
    this.setState({
      status: "offline",
      checkedAt: now(),
      error: this.describeProblem(lastError),
    });
    return this.state;
  }

  requireOnline(): RuntimeState {
    if (this.state.status !== "online") {
      throw new Error(this.i18n.t("runtime.notRunning"));
    }
    return this.state;
  }

  async mcpToken(): Promise<string> {
    if (this.mcpTokenCache) return this.mcpTokenCache;
    const stored = await this.secrets.get(MCP_TOKEN_KEY);
    const storedId = await this.secrets.get(MCP_TOKEN_ID_KEY);
    if (stored && storedId && (await this.isMcpTokenActive(storedId))) {
      this.mcpTokenCache = stored;
      return stored;
    }
    if (storedId) await this.revokeStoredMcpToken(storedId);
    this.requireOnline();
    const created = await this.client.createMcpToken("vscode-extension");
    if (!created.token.startsWith("hpa_pat_")) {
      throw new Error(this.i18n.t("mcp.tokenFailed"));
    }
    await this.secrets.store(MCP_TOKEN_KEY, created.token);
    await this.secrets.store(MCP_TOKEN_ID_KEY, created.record.id);
    this.mcpTokenCache = created.token;
    return created.token;
  }

  async resetMcpToken(): Promise<void> {
    const storedId = await this.secrets.get(MCP_TOKEN_ID_KEY);
    this.mcpTokenCache = undefined;
    await this.secrets.delete(MCP_TOKEN_KEY);
    await this.secrets.delete(MCP_TOKEN_ID_KEY);
    if (storedId) await this.revokeStoredMcpToken(storedId);
  }

  getInstalledSkills(): Record<string, InstalledSkillRecord> {
    return { ...(this.context.globalState.get<Record<string, InstalledSkillRecord>>(INSTALLED_SKILLS_KEY) ?? {}) };
  }

  async setInstalledSkill(record: InstalledSkillRecord): Promise<void> {
    const records = this.getInstalledSkills();
    records[record.skillId] = record;
    await this.context.globalState.update(INSTALLED_SKILLS_KEY, records);
    this.dataEvents.fire("skills");
  }

  async deleteInstalledSkill(skillId: string): Promise<void> {
    const records = this.getInstalledSkills();
    delete records[skillId];
    await this.context.globalState.update(INSTALLED_SKILLS_KEY, records);
    this.dataEvents.fire("skills");
  }

  async apiToken(): Promise<string | undefined> {
    const token = await this.secrets.get(API_TOKEN_KEY);
    return token && token.trim() !== "" ? token : undefined;
  }

  async setApiToken(token: string): Promise<void> {
    await this.secrets.store(API_TOKEN_KEY, token);
    await this.refreshRuntime();
  }

  async clearApiToken(): Promise<void> {
    await this.secrets.delete(API_TOKEN_KEY);
    await this.refreshRuntime();
  }

  startHealthLoop(): void {
    if (this.timer) return;
    this.timer = setInterval(() => {
      void this.refreshRuntime();
    }, HEALTH_INTERVAL_MS);
    void this.refreshRuntime();
  }

  dispose(): void {
    this.disposed = true;
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
    for (const disposable of this.disposables) disposable.dispose();
  }

  private async applyConfiguration(): Promise<void> {
    const config = this.config();
    this.translations.setLocale(localeFromLanguage(vscode.env.language, config.locale));
    this.client.setBaseUrl(config.localEndpoint || DEFAULT_LOCAL_ENDPOINT);
    this.dataEvents.fire();
    await this.refreshRuntime();
  }

  private async isMcpTokenActive(tokenId: string): Promise<boolean> {
    try {
      const response = await fetch(`${this.client.endpoint}/api/mcp/tokens`, {
        headers: await this.apiHeaders(),
        signal: AbortSignal.timeout(4000),
      });
      if (!response.ok) return false;
      const payload = (await response.json()) as {
        tokens?: { id?: string; revokedAt?: string | null }[];
      };
      const token = payload.tokens?.find((entry) => entry.id === tokenId);
      return Boolean(token && !token.revokedAt);
    } catch {
      return false;
    }
  }

  private async revokeStoredMcpToken(tokenId: string): Promise<void> {
    try {
      await this.client.revokeMcpToken(tokenId);
    } catch (error) {
      this.output.appendLine(`mcp token revoke failed: ${errorMessage(error)}`);
    }
  }

  private async apiHeaders(): Promise<Record<string, string>> {
    const token = await this.apiToken();
    return token ? { authorization: `Bearer ${token}` } : {};
  }

  private setState(state: RuntimeState): void {
    this.state = state;
    this.runtimeEmitter.fire(state);
    this.renderStatus();
  }

  private renderStatus(): void {
    if (this.state.status === "online") {
      this.statusBar.text = "$(pulse) hpa";
      this.statusBar.backgroundColor = undefined;
    } else if (this.state.status === "connecting") {
      this.statusBar.text = "$(sync~spin) hpa";
      this.statusBar.backgroundColor = undefined;
    } else if (this.state.status === "degraded") {
      this.statusBar.text = "$(warning) hpa";
      this.statusBar.backgroundColor = new vscode.ThemeColor("statusBarItem.warningBackground");
    } else {
      this.statusBar.text = "$(circle-slash) hpa";
      this.statusBar.backgroundColor = new vscode.ThemeColor("statusBarItem.errorBackground");
    }
    const lines = [
      `${this.i18n.t("common.status")}: ${this.i18n.t(`status.${this.state.status}`)}`,
      `${this.i18n.t("runtime.endpoint")}: ${this.client.endpoint}`,
    ];
    if (this.state.health) {
      lines.push(`${this.i18n.t("runtime.version")}: ${this.state.health.version}`);
    }
    if (this.state.error) {
      lines.push(`${this.i18n.t("common.error")}: ${this.state.error}`);
    }
    this.statusBar.tooltip = lines.join("\n");
    this.statusBar.show();
  }

  private describeProblem(code: string): string {
    if (code === "endpoint-not-configured" || code === "endpoint-invalid") {
      return this.i18n.t("runtime.notRunning");
    }
    if (code === "local-requires-loopback" || code === "remote-requires-https") {
      return this.i18n.t("runtime.healthFailed");
    }
    if (code === "runtime-identity-mismatch") {
      return this.i18n.t("runtime.healthFailed");
    }
    return this.i18n.t("errors.network", { message: code });
  }
}

interface EndpointCandidate {
  apiEndpoint: string;
  mcpEndpoint: string;
}

function healthCandidates(config: HpaConfig): EndpointCandidate[] {
  const candidates: EndpointCandidate[] = [];
  const local = config.localEndpoint || DEFAULT_LOCAL_ENDPOINT;
  const remote = config.remoteEndpoint;
  if (config.mode !== "remote" && !validateEndpoint(local, false)) {
    candidates.push({
      apiEndpoint: local,
      mcpEndpoint:
        config.mcpEndpoint && !validateEndpoint(config.mcpEndpoint, false)
          ? config.mcpEndpoint
          : deriveMcpEndpoint(local),
    });
  }
  if (config.mode !== "local" && remote !== "" && !validateEndpoint(remote, true)) {
    candidates.push({
      apiEndpoint: remote,
      mcpEndpoint:
        config.remoteMcpEndpoint && !validateEndpoint(config.remoteMcpEndpoint, true)
          ? config.remoteMcpEndpoint
          : deriveMcpEndpoint(remote),
    });
  }
  return candidates;
}

function config(): vscode.WorkspaceConfiguration {
  return vscode.workspace.getConfiguration("hpa");
}

function now(): string {
  return new Date().toISOString();
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
