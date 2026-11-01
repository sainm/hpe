import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Boxes,
  Copy,
  FileKey2,
  Globe2,
  KeyRound,
  RefreshCw,
  ServerCog,
  ShieldCheck,
  TerminalSquare,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Badge,
  Button,
  EmptyState,
  Field,
  JsonView,
  PageHeader,
  Panel,
  Timestamp,
  cn,
  inputClass,
  textareaClass,
} from "../components/ui";
import { api, ApiError } from "../lib/api";
import type {
  JsonValue,
  McpProbe,
  McpServer,
  McpTool,
  McpToolResult,
} from "../lib/types";
import { useUiStore } from "../stores/ui";

function riskTone(risk: string): "danger" | "warning" | "neutral" {
  if (risk === "high") return "danger";
  if (risk === "medium") return "warning";
  return "neutral";
}

export function McpCenterPage() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const overview = useQuery({ queryKey: ["mcp-overview"], queryFn: api.mcpOverview, retry: 1 });
  const catalog = useQuery({ queryKey: ["mcp-catalog"], queryFn: api.mcpCatalog, retry: 1 });
  const approvals = useQuery({
    queryKey: ["mcp-approvals"],
    queryFn: () => api.mcpApprovals(),
    retry: 1,
    refetchInterval: 3000,
  });
  const tokens = useQuery({ queryKey: ["mcp-tokens"], queryFn: api.mcpTokens, retry: 1 });
  const servers = useQuery({ queryKey: ["mcp-servers"], queryFn: api.mcpServers, retry: 1 });
  const audit = useQuery({ queryKey: ["mcp-audit"], queryFn: () => api.mcpAudit(30), retry: 1 });

  const [selectedTool, setSelectedTool] = useState("");
  const [argumentsText, setArgumentsText] = useState("{}");
  const [callResult, setCallResult] = useState<McpToolResult | null>(null);
  const [pendingState, setPendingState] = useState<string | null>(null);
  const [pendingApproval, setPendingApproval] = useState<string | null>(null);
  const [tokenName, setTokenName] = useState("external client");
  const [newToken, setNewToken] = useState<string | null>(null);
  const [preview, setPreview] = useState<{ title: string; value: JsonValue } | null>(null);
  const [serverName, setServerName] = useState("");
  const [serverTransport, setServerTransport] = useState<"stdio" | "http">("http");
  const [serverUrl, setServerUrl] = useState("http://127.0.0.1:8787/mcp");
  const [serverCommand, setServerCommand] = useState("");
  const [serverTools, setServerTools] = useState("");
  const [serverNetwork, setServerNetwork] = useState("");
  const [serverCredential, setServerCredential] = useState("HPA_MCP_TOKEN");
  const [probes, setProbes] = useState<Record<string, McpProbe>>({});
  const [externalTool, setExternalTool] = useState<Record<string, string>>({});
  const [externalArgs, setExternalArgs] = useState<Record<string, string>>({});
  const [externalResult, setExternalResult] = useState<Record<string, McpToolResult>>({});

  const tools = catalog.data?.tools ?? [];
  const selected = useMemo(
    () => tools.find((tool) => tool.name === selectedTool),
    [tools, selectedTool],
  );

  useEffect(() => {
    if (!selectedTool && tools.length > 0) {
      setSelectedTool(tools[0]!.name);
    }
  }, [selectedTool, tools]);

  useEffect(() => {
    if (selected?.inputSchema) {
      const defaults = defaultsForSchema(selected.inputSchema);
      setArgumentsText(JSON.stringify(defaults, null, 2));
    }
  }, [selected?.name]);

  const callTool = useMutation({
    mutationFn: async ({
      name,
      args,
      state,
    }: {
      name: string;
      args: JsonValue;
      state?: string;
    }) => api.mcpCall(name, args, state),
    onSuccess: (result) => {
      setCallResult(result);
      if (result.resultType === "input_required") {
        setPendingState(result.requestState ?? null);
        const approvalId = result._meta?.["hpa/approvalId"];
        setPendingApproval(typeof approvalId === "string" ? approvalId : null);
        pushToast(t("mcp.approvalRequired"));
        void queryClient.invalidateQueries({ queryKey: ["mcp-approvals"] });
      } else {
        setPendingState(null);
        setPendingApproval(null);
        pushToast(result.isError ? t("mcp.callFailed") : t("mcp.callSucceeded"), result.isError ? "error" : "success");
        void queryClient.invalidateQueries({ queryKey: ["mcp-audit"] });
        void queryClient.invalidateQueries({ queryKey: ["mcp-overview"] });
      }
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  const decide = useMutation({
    mutationFn: ({ id, decision }: { id: string; decision: "approved" | "rejected" }) =>
      api.mcpDecideApproval(id, decision),
    onSuccess: () => {
      pushToast(t("mcp.approvalDecided"), "success");
      void queryClient.invalidateQueries({ queryKey: ["mcp-approvals"] });
      void queryClient.invalidateQueries({ queryKey: ["mcp-overview"] });
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  const createToken = useMutation({
    mutationFn: () => api.mcpCreateToken(tokenName.trim() || "client", ["mcp:read", "mcp:execute"]),
    onSuccess: (result) => {
      setNewToken(result.token);
      pushToast(t("mcp.tokenCreated"), "success");
      void queryClient.invalidateQueries({ queryKey: ["mcp-tokens"] });
      void queryClient.invalidateQueries({ queryKey: ["mcp-overview"] });
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  const revokeToken = useMutation({
    mutationFn: (id: string) => api.mcpRevokeToken(id),
    onSuccess: () => {
      pushToast(t("mcp.tokenRevoked"), "success");
      void queryClient.invalidateQueries({ queryKey: ["mcp-tokens"] });
      void queryClient.invalidateQueries({ queryKey: ["mcp-overview"] });
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  const saveServer = useMutation({
    mutationFn: () =>
      api.mcpSaveServer({
        name: serverName.trim(),
        transport: serverTransport,
        command: serverCommand.trim() ? serverCommand.trim().split(/\s+/) : [],
        url: serverTransport === "http" ? serverUrl.trim() : undefined,
        allowedTools: splitList(serverTools),
        network: splitList(serverNetwork),
        credentialEnv: serverCredential.trim() || undefined,
      }),
    onSuccess: () => {
      pushToast(t("mcp.serverSaved"), "success");
      setServerName("");
      void queryClient.invalidateQueries({ queryKey: ["mcp-servers"] });
      void queryClient.invalidateQueries({ queryKey: ["mcp-overview"] });
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  const deleteServer = useMutation({
    mutationFn: (id: string) => api.mcpDeleteServer(id),
    onSuccess: () => {
      pushToast(t("mcp.serverDeleted"), "success");
      void queryClient.invalidateQueries({ queryKey: ["mcp-servers"] });
      void queryClient.invalidateQueries({ queryKey: ["mcp-overview"] });
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  const probeServer = useMutation({
    mutationFn: (id: string) => api.mcpProbeServer(id),
    onSuccess: (result) => {
      setProbes((current) => ({ ...current, [result.serverId]: result }));
      pushToast(t("mcp.probeSucceeded"), "success");
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  const callExternal = useMutation({
    mutationFn: async ({ server, tool }: { server: McpServer; tool: string }) => {
      const raw = externalArgs[server.id] ?? "{}";
      const args = JSON.parse(raw) as JsonValue;
      return { serverId: server.id, result: await api.mcpCallServer(server.id, tool, args) };
    },
    onSuccess: ({ serverId, result }) => {
      setExternalResult((current) => ({ ...current, [serverId]: result }));
      pushToast(result.isError ? t("mcp.callFailed") : t("mcp.callSucceeded"), result.isError ? "error" : "success");
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  const pendingApprovals = approvals.data?.approvals.filter((approval) => approval.status === "pending") ?? [];

  function runSelectedTool(state?: string) {
    let args: JsonValue;
    try {
      args = JSON.parse(argumentsText) as JsonValue;
    } catch {
      pushToast(t("mcp.invalidArguments"), "error");
      return;
    }
    if (selected) {
      callTool.mutate({ name: selected.name, args, state });
    }
  }

  async function previewResource(uri: string, title: string) {
    try {
      const value = await api.mcpReadResource(uri);
      setPreview({ title, value });
    } catch (error) {
      pushToast(messageOf(error), "error");
    }
  }

  async function previewPrompt(name: string) {
    try {
      const value = await api.mcpGetPrompt(name, {});
      setPreview({ title: name, value });
    } catch (error) {
      pushToast(messageOf(error), "error");
    }
  }

  return (
    <div className="mx-auto grid max-w-[1560px] gap-3">
      <PageHeader
        title={t("mcp.title")}
        subtitle={t("mcp.subtitle")}
        actions={
          <Button
            variant="secondary"
            onClick={() => {
              void queryClient.invalidateQueries();
            }}
          >
            <RefreshCw className="h-3.5 w-3.5" />
            {t("common.refresh")}
          </Button>
        }
      />

      <Panel className="p-3">
        <div className="flex flex-wrap items-center gap-2">
          <Globe2 className="h-4 w-4 text-accent" />
          <span className="font-mono text-xs text-ink">{overview.data?.endpoint ?? "—"}</span>
          <Badge tone="success">{t("mcp.localGateway")}</Badge>
          <span className="ml-auto font-mono text-[10px] text-faint">
            {overview.data?.protocols.join(" · ") ?? ""}
          </span>
          <span className="font-mono text-[10px] text-faint">
            {t("skills.sandbox")} {overview.data?.sandbox ?? "process"}
          </span>
        </div>
        <div className="mt-3 grid gap-2 sm:grid-cols-3 xl:grid-cols-6">
          <Stat label={t("mcp.statTools")} value={overview.data?.stats.tools ?? 0} />
          <Stat label={t("mcp.statRequests")} value={overview.data?.stats.requests ?? 0} />
          <Stat label={t("mcp.statApprovals")} value={overview.data?.stats.pendingApprovals ?? 0} />
          <Stat label={t("mcp.statTokens")} value={overview.data?.stats.tokens ?? 0} />
          <Stat label={t("mcp.statServers")} value={overview.data?.stats.servers ?? 0} />
          <Stat label={t("mcp.statResources")} value={overview.data?.stats.resources ?? 0} />
        </div>
      </Panel>

      <div className="grid gap-3 xl:grid-cols-[minmax(0,1.15fr)_minmax(0,1fr)]">
        <Panel className="p-4">
          <div className="flex items-center gap-2">
            <TerminalSquare className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("mcp.tools")}</h2>
            <span className="ml-auto font-mono text-[10px] text-faint">{tools.length}</span>
          </div>
          {catalog.isError ? (
            <p className="mt-2 text-xs text-danger">{t("errors.loadFailed")}</p>
          ) : tools.length === 0 ? (
            <p className="mt-2 text-xs text-muted">{t("mcp.noTools")}</p>
          ) : (
            <div className="mt-3 grid gap-1.5">
              {tools.map((tool) => (
                <button
                  key={tool.name}
                  type="button"
                  onClick={() => setSelectedTool(tool.name)}
                  className={cn(
                    "rounded-sm border px-2.5 py-2 text-left transition-colors",
                    selectedTool === tool.name
                      ? "border-accent/45 bg-accent/10"
                      : "border-line bg-panel hover:border-line-strong",
                  )}
                >
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-mono text-[11px] text-ink">{tool.name}</span>
                    <Badge tone={riskTone(riskOf(tool))}>{riskOf(tool)}</Badge>
                    {approvalOf(tool) === "always" ? (
                      <Badge tone="danger">{t("mcp.approvalAlways")}</Badge>
                    ) : null}
                  </div>
                  <p className="mt-0.5 line-clamp-1 text-[10.5px] text-muted">{tool.description}</p>
                </button>
              ))}
            </div>
          )}

          {selected ? (
            <div className="mt-3 grid gap-2 border-t border-line pt-3">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-mono text-xs text-ink">{selected.name}</span>
                <Badge tone="neutral">{t("skills.runInput")}</Badge>
                <Button
                  size="sm"
                  variant="primary"
                  className="ml-auto"
                  loading={callTool.isPending}
                  onClick={() => runSelectedTool()}
                >
                  {t("mcp.callTool")}
                </Button>
                {pendingState ? (
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => runSelectedTool(pendingState)}
                  >
                    {t("mcp.retryAfterApproval")}
                  </Button>
                ) : null}
              </div>
              <textarea
                className={cn(textareaClass, "min-h-[120px] font-mono text-[11px]")}
                value={argumentsText}
                onChange={(event) => setArgumentsText(event.target.value)}
                spellCheck={false}
                aria-label={t("mcp.arguments")}
              />
              {pendingApproval ? (
                <p className="rounded-sm border border-warning/30 bg-warning/10 px-2 py-1.5 text-[10.5px] text-warning">
                  {t("mcp.approvalPending", { id: pendingApproval })}
                </p>
              ) : null}
              {callResult ? (
                <div className="grid gap-1.5">
                  <div className="flex items-center gap-2">
                    <Badge tone={callResult.isError ? "danger" : "success"}>
                      {callResult.resultType ?? (callResult.isError ? "error" : "complete")}
                    </Badge>
                    {callResult.error ? (
                      <span className="font-mono text-[10px] text-danger">
                        {callResult.error.code} {callResult.error.message}
                      </span>
                    ) : null}
                  </div>
                  <JsonView
                    value={callResult.structuredContent ?? callResult.content ?? callResult}
                    maxHeight="260px"
                  />
                </div>
              ) : null}
            </div>
          ) : null}
        </Panel>

        <div className="grid content-start gap-3">
          <Panel className="p-4">
            <div className="flex items-center gap-2">
              <ShieldCheck className="h-4 w-4 text-accent" />
              <h2 className="text-sm font-semibold text-ink">{t("mcp.approvals")}</h2>
              <Badge tone={pendingApprovals.length > 0 ? "warning" : "neutral"}>
                {pendingApprovals.length}
              </Badge>
            </div>
            {pendingApprovals.length === 0 ? (
              <p className="mt-2 text-xs text-muted">{t("mcp.noApprovals")}</p>
            ) : (
              <div className="mt-3 grid gap-1.5">
                {pendingApprovals.map((approval) => (
                  <div key={approval.id} className="rounded-sm border border-line px-2.5 py-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-mono text-[10.5px] text-ink">{approval.toolName}</span>
                      <Badge tone={riskTone(approval.risk)}>{approval.risk}</Badge>
                      <span className="ml-auto font-mono text-[9.5px] text-faint">
                        <Timestamp value={approval.createdAt} />
                      </span>
                    </div>
                    <JsonView value={approval.arguments} maxHeight="80px" className="mt-1" />
                    <div className="mt-1.5 flex gap-1.5">
                      <Button
                        size="sm"
                        variant="primary"
                        loading={decide.isPending && decide.variables?.id === approval.id}
                        onClick={() => decide.mutate({ id: approval.id, decision: "approved" })}
                      >
                        {t("common.approve")}
                      </Button>
                      <Button
                        size="sm"
                        variant="secondary"
                        onClick={() => decide.mutate({ id: approval.id, decision: "rejected" })}
                      >
                        {t("common.reject")}
                      </Button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </Panel>

          <Panel className="p-4">
            <div className="flex items-center gap-2">
              <KeyRound className="h-4 w-4 text-accent" />
              <h2 className="text-sm font-semibold text-ink">{t("mcp.tokens")}</h2>
              <span className="ml-auto font-mono text-[10px] text-faint">
                {tokens.data?.tokens.length ?? 0}
              </span>
            </div>
            <div className="mt-3 grid gap-2 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-end">
              <Field label={t("mcp.tokenName")}>
                <input
                  className={inputClass}
                  value={tokenName}
                  onChange={(event) => setTokenName(event.target.value)}
                />
              </Field>
              <Button
                size="sm"
                variant="secondary"
                loading={createToken.isPending}
                onClick={() => createToken.mutate()}
              >
                <FileKey2 className="h-3.5 w-3.5" />
                {t("mcp.createToken")}
              </Button>
            </div>
            {newToken ? (
              <div className="mt-2 rounded-sm border border-warning/30 bg-warning/5 p-2">
                <p className="text-[10px] text-warning">{t("mcp.tokenOnce")}</p>
                <div className="mt-1 flex items-center gap-1.5">
                  <code className="min-w-0 flex-1 truncate font-mono text-[10px] text-ink">
                    {newToken}
                  </code>
                  <Button size="sm" variant="ghost" onClick={() => void navigator.clipboard.writeText(newToken)}>
                    <Copy className="h-3 w-3" />
                  </Button>
                </div>
              </div>
            ) : null}
            <div className="mt-2 grid gap-1">
              {(tokens.data?.tokens ?? []).map((token) => (
                <div key={token.id} className="flex items-center gap-2 border-t border-line py-1.5">
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[11px] text-ink">{token.name}</span>
                    <span className="block truncate font-mono text-[9.5px] text-faint">
                      {token.id} · {token.scopes.join(",")}
                    </span>
                  </span>
                  {token.revokedAt ? (
                    <Badge tone="neutral">{t("mcp.revoked")}</Badge>
                  ) : (
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => revokeToken.mutate(token.id)}
                    >
                      {t("mcp.revoke")}
                    </Button>
                  )}
                </div>
              ))}
            </div>
          </Panel>
        </div>
      </div>

      <div className="grid gap-3 xl:grid-cols-2">
        <Panel className="p-4">
          <div className="flex items-center gap-2">
            <Boxes className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("mcp.resources")}</h2>
          </div>
          <div className="mt-2 grid gap-1">
            {(catalog.data?.resources ?? []).map((resource) => (
              <button
                key={resource.uri}
                type="button"
                onClick={() => void previewResource(resource.uri, resource.name)}
                className="flex items-center gap-2 rounded-sm border border-line px-2 py-1.5 text-left hover:border-accent/40"
              >
                <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-accent">
                  {resource.uri}
                </span>
                <span className="font-mono text-[9px] text-faint">{resource.mimeType}</span>
              </button>
            ))}
          </div>
          <h3 className="mt-3 text-xs font-semibold text-ink">{t("mcp.prompts")}</h3>
          <div className="mt-1.5 flex flex-wrap gap-1.5">
            {(catalog.data?.prompts ?? []).map((prompt) => (
              <button
                key={prompt.name}
                type="button"
                onClick={() => void previewPrompt(prompt.name)}
                className="inline-flex h-6 items-center rounded-sm border border-line bg-bg-elevated px-2 font-mono text-[10px] text-accent hover:border-accent/40"
              >
                {prompt.name}
              </button>
            ))}
          </div>
          {preview ? (
            <div className="mt-3">
              <div className="flex items-center gap-2">
                <span className="truncate font-mono text-[10px] text-muted">{preview.title}</span>
                <Button size="sm" variant="ghost" className="ml-auto" onClick={() => setPreview(null)}>
                  {t("common.close")}
                </Button>
              </div>
              <JsonView value={preview.value} maxHeight="220px" />
            </div>
          ) : null}
        </Panel>

        <Panel className="p-4">
          <div className="flex items-center gap-2">
            <ServerCog className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("mcp.servers")}</h2>
            <span className="ml-auto font-mono text-[10px] text-faint">
              {servers.data?.servers.length ?? 0}
            </span>
          </div>
          <div className="mt-3 grid gap-2">
            <div className="grid gap-2 sm:grid-cols-2">
              <Field label={t("mcp.serverName")}>
                <input className={inputClass} value={serverName} onChange={(event) => setServerName(event.target.value)} />
              </Field>
              <Field label={t("mcp.transport")}>
                <select
                  className={inputClass}
                  value={serverTransport}
                  onChange={(event) => setServerTransport(event.target.value as "stdio" | "http")}
                >
                  <option value="http">http</option>
                  <option value="stdio">stdio</option>
                </select>
              </Field>
            </div>
            {serverTransport === "http" ? (
              <div className="grid gap-2 sm:grid-cols-2">
                <Field label={t("mcp.serverUrl")}>
                  <input className={inputClass} value={serverUrl} onChange={(event) => setServerUrl(event.target.value)} />
                </Field>
                <Field label={t("mcp.credentialEnv")}>
                  <input
                    className={inputClass}
                    value={serverCredential}
                    onChange={(event) => setServerCredential(event.target.value)}
                  />
                </Field>
              </div>
            ) : (
              <Field label={t("mcp.serverCommand")}>
                <input
                  className={inputClass}
                  value={serverCommand}
                  onChange={(event) => setServerCommand(event.target.value)}
                  placeholder="/usr/local/bin/hpa mcp serve"
                />
              </Field>
            )}
            <div className="grid gap-2 sm:grid-cols-2">
              <Field label={t("mcp.allowedTools")}>
                <input
                  className={inputClass}
                  value={serverTools}
                  onChange={(event) => setServerTools(event.target.value)}
                  placeholder="workflow.list, skill.invoice-audit.audit"
                />
              </Field>
              <Field label={t("mcp.networkAllowlist")}>
                <input
                  className={inputClass}
                  value={serverNetwork}
                  onChange={(event) => setServerNetwork(event.target.value)}
                  placeholder="mcp.example.com:443"
                />
              </Field>
            </div>
            <Button
              size="sm"
              variant="secondary"
              loading={saveServer.isPending}
              disabled={!serverName.trim()}
              onClick={() => saveServer.mutate()}
            >
              {t("mcp.addServer")}
            </Button>
          </div>

          <div className="mt-3 grid gap-2">
            {(servers.data?.servers ?? []).map((server) => {
              const probe = probes[server.id];
              const tool = externalTool[server.id] ?? probe?.tools[0]?.name ?? "";
              return (
                <div key={server.id} className="rounded-sm border border-line p-2.5">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-xs font-medium text-ink">{server.name}</span>
                    <Badge tone="neutral">{server.transport}</Badge>
                    {probe ? <Badge tone="success">{probe.era}</Badge> : null}
                    <div className="ml-auto flex gap-1.5">
                      <Button
                        size="sm"
                        variant="ghost"
                        loading={probeServer.isPending && probeServer.variables === server.id}
                        onClick={() => probeServer.mutate(server.id)}
                      >
                        {t("mcp.probe")}
                      </Button>
                      <Button size="sm" variant="ghost" onClick={() => deleteServer.mutate(server.id)}>
                        {t("common.delete")}
                      </Button>
                    </div>
                  </div>
                  <p className="mt-0.5 break-all font-mono text-[9.5px] text-faint">
                    {server.url ?? server.command.join(" ")}
                  </p>
                  {probe ? (
                    <div className="mt-2 grid gap-1.5">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <select
                          className={cn(inputClass, "h-7 w-auto font-mono text-[10px]")}
                          value={tool}
                          onChange={(event) =>
                            setExternalTool((current) => ({ ...current, [server.id]: event.target.value }))
                          }
                        >
                          {probe.tools.map((entry) => (
                            <option key={entry.name} value={entry.name}>
                              {entry.name}
                            </option>
                          ))}
                        </select>
                        <Button
                          size="sm"
                          variant="secondary"
                          loading={callExternal.isPending && callExternal.variables?.server.id === server.id}
                          disabled={!tool}
                          onClick={() => callExternal.mutate({ server, tool })}
                        >
                          {t("mcp.callTool")}
                        </Button>
                        <span className="font-mono text-[9.5px] text-faint">
                          {probe.tools.length} {t("mcp.statTools")}
                        </span>
                      </div>
                      <textarea
                        className={cn(textareaClass, "min-h-[60px] font-mono text-[10px]")}
                        value={externalArgs[server.id] ?? "{}"}
                        onChange={(event) =>
                          setExternalArgs((current) => ({ ...current, [server.id]: event.target.value }))
                        }
                        aria-label={t("mcp.arguments")}
                      />
                      {externalResult[server.id] ? (
                        <JsonView
                          value={
                            externalResult[server.id]!.structuredContent ??
                            externalResult[server.id]!.content ??
                            externalResult[server.id]!
                          }
                          maxHeight="140px"
                        />
                      ) : null}
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
        </Panel>
      </div>

      <Panel className="p-4">
        <h2 className="text-sm font-semibold text-ink">{t("mcp.audit")}</h2>
        {audit.isLoading ? (
          <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
        ) : (audit.data?.audit ?? []).length === 0 ? (
          <EmptyState
            icon={<ShieldCheck className="h-5 w-5" />}
            title={t("mcp.noAudit")}
            hint={t("mcp.noAuditHint")}
          />
        ) : (
          <div className="mt-3 overflow-x-auto rounded-sm border border-line">
            <table className="w-full min-w-[600px] border-collapse text-left text-[11px]">
              <thead className="bg-panel-3 text-[9.5px] uppercase tracking-[0.12em] text-faint">
                <tr>
                  <th className="px-3 py-1.5 font-medium">{t("common.status")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("mcp.method")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("mcp.tool")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("skills.duration")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("skills.runs")}</th>
                </tr>
              </thead>
              <tbody>
                {(audit.data?.audit ?? []).map((entry) => (
                  <tr key={entry.id} className="border-t border-line">
                    <td className="px-3 py-1.5">
                      <Badge
                        tone={
                          entry.status === "ok"
                            ? "success"
                            : entry.status === "approval_pending"
                              ? "warning"
                              : "danger"
                        }
                      >
                        {entry.status}
                      </Badge>
                    </td>
                    <td className="px-3 py-1.5 font-mono text-ink">{entry.method}</td>
                    <td className="px-3 py-1.5 font-mono text-muted">{entry.toolName ?? "—"}</td>
                    <td className="px-3 py-1.5 font-mono text-muted">{entry.latencyMs} ms</td>
                    <td className="px-3 py-1.5 font-mono text-faint">
                      <Timestamp value={entry.at} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Panel>
    </div>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-sm border border-line bg-bg-elevated px-3 py-2">
      <p className="text-[9px] uppercase tracking-[0.14em] text-faint">{label}</p>
      <p className="mt-0.5 font-mono text-sm text-ink">{value}</p>
    </div>
  );
}

function riskOf(tool: McpTool): string {
  const value = tool._meta?.["hpa/risk"];
  return typeof value === "string" ? value : "low";
}

function approvalOf(tool: McpTool): string {
  const value = tool._meta?.["hpa/approval"];
  return typeof value === "string" ? value : "never";
}

function defaultsForSchema(schema: JsonValue): JsonValue {
  if (!schema || typeof schema !== "object") {
    return {};
  }
  const object = schema as Record<string, JsonValue>;
  const properties = object.properties;
  if (!properties || typeof properties !== "object") {
    return {};
  }
  const result: Record<string, JsonValue> = {};
  for (const [key, value] of Object.entries(properties as Record<string, JsonValue>)) {
    if (value && typeof value === "object") {
      const property = value as Record<string, JsonValue>;
      result[key] =
        property.type === "object"
          ? {}
          : property.type === "array"
            ? []
            : property.type === "number" || property.type === "integer"
              ? 0
              : property.type === "boolean"
                ? false
                : "";
    }
  }
  return result;
}

function splitList(text: string): string[] {
  return text
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function messageOf(error: unknown): string {
  if (error instanceof ApiError) return error.message;
  return error instanceof Error ? error.message : String(error);
}
