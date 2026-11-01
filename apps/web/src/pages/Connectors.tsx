import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Cable,
  FileJson2,
  Globe2,
  Play,
  Plus,
  RefreshCw,
  ScrollText,
  ShieldCheck,
  Sparkles,
  Trash2,
  Unplug,
} from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
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
  type BadgeTone,
} from "../components/ui";
import { api, ApiError } from "../lib/api";
import type {
  ConnectorAuditEntry,
  ConnectorDefinition,
  JsonValue,
} from "../lib/types";
import { useUiStore } from "../stores/ui";

type ConnectorTab = "catalog" | "connections" | "audit";

function riskTone(risk: string): BadgeTone {
  if (risk === "destructive") return "danger";
  if (risk === "write") return "warning";
  return "neutral";
}

function auditTone(status: string): BadgeTone {
  if (status === "succeeded" || status === "ok") return "success";
  if (status === "failed") return "danger";
  return "neutral";
}

export function ConnectorsPage() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [tab, setTab] = useState<ConnectorTab>("catalog");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const pendingSelect = useRef<string | null>(null);
  const [name, setName] = useState("");
  const [connectorId, setConnectorId] = useState("");
  const [configText, setConfigText] = useState("{}");
  const [credentialEnv, setCredentialEnv] = useState("");
  const [operation, setOperation] = useState("");
  const [argsText, setArgsText] = useState("{}");
  const [testResult, setTestResult] = useState<{ value: JsonValue; latencyMs: number } | null>(null);
  const [invokeResult, setInvokeResult] = useState<{
    value: JsonValue;
    latencyMs: number;
  } | null>(null);

  const connectors = useQuery({
    queryKey: ["connectors"],
    queryFn: api.listConnectors,
    retry: 1,
  });
  const connections = useQuery({
    queryKey: ["connections"],
    queryFn: api.listConnections,
    retry: 1,
  });
  const audit = useQuery({
    queryKey: ["connector-audit"],
    queryFn: () => api.connectorAudit(50),
    retry: 1,
  });

  const connectorList = connectors.data?.connectors ?? [];
  const connectionList = connections.data?.connections ?? [];
  const selected = connectionList.find((entry) => entry.id === selectedId) ?? null;
  const builtIn = connectorList.filter((entry) => !entry.pluginId);
  const contributed = connectorList.filter((entry) => Boolean(entry.pluginId));
  const activeConnector = connectorList.find((entry) => entry.connectorId === connectorId);
  const selectedOperation = selected?.operations.find((entry) => entry.name === operation);

  useEffect(() => {
    if (!connectorId && connectorList.length > 0) {
      setConnectorId(connectorList[0]!.connectorId);
    }
  }, [connectorList, connectorId]);

  useEffect(() => {
    if (!connectorId) return;
    const connector = connectorList.find((entry) => entry.connectorId === connectorId);
    if (connector) {
      setConfigText(JSON.stringify(connector.configShape, null, 2));
    }
  }, [connectorId, connectorList]);

  useEffect(() => {
    if (pendingSelect.current) {
      if (connectionList.some((entry) => entry.id === pendingSelect.current)) {
        pendingSelect.current = null;
      }
      return;
    }
    if (connectionList.length === 0) {
      setSelectedId(null);
      return;
    }
    if (selectedId && connectionList.some((entry) => entry.id === selectedId)) return;
    setSelectedId(connectionList[0]!.id);
  }, [connectionList, selectedId]);

  useEffect(() => {
    if (!selected) {
      setOperation("");
      return;
    }
    setOperation((current) =>
      selected.operations.some((entry) => entry.name === current)
        ? current
        : (selected.operations[0]?.name ?? ""),
    );
  }, [selected?.id, selected?.operations.length]);

  useEffect(() => {
    setArgsText(JSON.stringify(defaultsForSchema(selectedOperation?.inputSchema), null, 2));
  }, [selected?.id, selectedOperation?.name]);

  useEffect(() => {
    setInvokeResult(null);
  }, [selected?.id, operation]);

  function invalidateConnectorQueries() {
    void queryClient.invalidateQueries({ queryKey: ["connectors"] });
    void queryClient.invalidateQueries({ queryKey: ["connections"] });
    void queryClient.invalidateQueries({ queryKey: ["connector-audit"] });
  }

  const saveConnection = useMutation({
    mutationFn: async () => {
      let config: JsonValue;
      try {
        config = JSON.parse(configText) as JsonValue;
      } catch {
        throw new ApiError(0, "CONFIG_JSON", t("connectors.invalidJson"));
      }
      return api.saveConnection({
        name: name.trim(),
        connectorId,
        config,
        credentialEnv: credentialEnv.trim() || undefined,
      });
    },
    onSuccess: (result) => {
      pushToast(t("connectors.saved"), "success");
      setName("");
      setCredentialEnv("");
      pendingSelect.current = result.id;
      setSelectedId(result.id);
      invalidateConnectorQueries();
    },
    onError: (error) => pushToast(errorMessage(error, t("connectors.saveFailed")), "error"),
  });

  const sampleConnection = useMutation({
    mutationFn: api.sampleConnection,
    onSuccess: (result) => {
      pushToast(t("connectors.sampleAdded"), "success");
      pendingSelect.current = result.id;
      setSelectedId(result.id);
      invalidateConnectorQueries();
    },
    onError: (error) => pushToast(errorMessage(error, t("connectors.sampleFailed")), "error"),
  });

  const deleteConnection = useMutation({
    mutationFn: (id: string) => api.deleteConnection(id),
    onSuccess: (result, id) => {
      if (!result.deleted) {
        pushToast(t("connectors.deleteFailed"), "error");
        return;
      }
      pushToast(t("connectors.deleted"), "success");
      if (selectedId === id) {
        setTestResult(null);
        setInvokeResult(null);
      }
      invalidateConnectorQueries();
    },
    onError: (error) => pushToast(errorMessage(error, t("connectors.deleteFailed")), "error"),
  });

  const testConnection = useMutation({
    mutationFn: async (id: string) => {
      const started = performance.now();
      const result = await api.testConnection(id);
      return {
        value: result.test,
        latencyMs: Math.round(performance.now() - started),
      };
    },
    onSuccess: (result) => {
      setTestResult(result);
      pushToast(t("connectors.tested"), "success");
      void queryClient.invalidateQueries({ queryKey: ["connector-audit"] });
    },
    onError: (error) => pushToast(errorMessage(error, t("connectors.testFailed")), "error"),
  });

  const invokeConnection = useMutation({
    mutationFn: async () => {
      let args: JsonValue;
      try {
        args = JSON.parse(argsText) as JsonValue;
      } catch {
        throw new ApiError(0, "ARGS_JSON", t("connectors.invalidJson"));
      }
      const started = performance.now();
      const result = await api.invokeConnection(selectedId ?? "", operation, args);
      return {
        value: result.result,
        latencyMs: Math.round(performance.now() - started),
      };
    },
    onSuccess: (result) => {
      setInvokeResult(result);
      pushToast(t("connectors.invoked"), "success");
      void queryClient.invalidateQueries({ queryKey: ["connector-audit"] });
    },
    onError: (error) => pushToast(errorMessage(error, t("connectors.invokeFailed")), "error"),
  });

  function selectConnection(id: string) {
    if (id === selectedId) return;
    setSelectedId(id);
    setTestResult(null);
    setInvokeResult(null);
  }

  return (
    <div className="mx-auto grid max-w-[1560px] gap-3">
      <PageHeader
        title={t("connectors.title")}
        subtitle={t("connectors.subtitle")}
        actions={
          <>
            <Button variant="secondary" onClick={() => void queryClient.invalidateQueries()}>
              <RefreshCw className="h-3.5 w-3.5" />
              {t("common.refresh")}
            </Button>
            <Button
              variant="secondary"
              loading={sampleConnection.isPending}
              onClick={() => sampleConnection.mutate()}
            >
              <Sparkles className="h-3.5 w-3.5" />
              {t("connectors.addSample")}
            </Button>
          </>
        }
      />

      <div className="flex flex-wrap gap-1">
        {(
          [
            ["catalog", t("connectors.tabs.catalog"), <Cable key="c" className="h-3 w-3" />],
            ["connections", t("connectors.tabs.connections"), <Unplug key="n" className="h-3 w-3" />],
            ["audit", t("connectors.tabs.audit"), <ScrollText key="a" className="h-3 w-3" />],
          ] as [ConnectorTab, string, ReactNode][]
        ).map(([key, label, icon]) => (
          <button
            key={key}
            type="button"
            onClick={() => setTab(key)}
            className={cn(
              "inline-flex h-7 items-center gap-1.5 rounded-sm border px-2.5 text-[11.5px] font-medium",
              tab === key
                ? "border-accent/40 bg-accent/10 text-accent"
                : "border-line bg-panel text-muted hover:text-ink",
            )}
          >
            {icon}
            {label}
          </button>
        ))}
        <span className="ml-auto self-center font-mono text-[10px] text-faint">
          {connectorList.length} · {connectionList.length}
        </span>
      </div>

      <Panel className="px-3 py-2">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[10px] text-faint">
          <span className="inline-flex items-center gap-1">
            <ShieldCheck className="h-3 w-3 text-accent" />
            {t("connectors.credentialNote")}
          </span>
          <span className="hidden h-3 w-px bg-line sm:block" />
          <span>{t("connectors.fileRootNote")}</span>
          <span className="hidden h-3 w-px bg-line sm:block" />
          <span className="text-warning">{t("connectors.writeNote")}</span>
        </div>
      </Panel>

      {tab === "catalog" ? (
        <>
          <div className="grid gap-3 xl:grid-cols-2">
            <ConnectorGroup
              icon={<Cable className="h-4 w-4 text-accent" />}
              title={t("connectors.builtIn")}
              entries={builtIn}
              loading={connectors.isLoading}
              error={connectors.isError}
            />
            <ConnectorGroup
              icon={<FileJson2 className="h-4 w-4 text-accent" />}
              title={t("connectors.fromPlugins")}
              entries={contributed}
              loading={connectors.isLoading}
              error={connectors.isError}
            />
          </div>
          <p className="text-[10px] text-faint">{t("connectors.agentNote")}</p>
        </>
      ) : null}

      {tab === "connections" ? (
        <div className="grid gap-3 lg:grid-cols-[320px_minmax(0,1fr)]">
          <div className="grid content-start gap-3">
            <Panel className="p-3">
              <div className="flex items-center gap-2">
                <Plus className="h-4 w-4 text-accent" />
                <h2 className="text-sm font-semibold text-ink">{t("connectors.newConnection")}</h2>
              </div>
              <div className="mt-2 grid gap-2">
                <Field label={t("connectors.name")}>
                  <input
                    className={inputClass}
                    value={name}
                    onChange={(event) => setName(event.target.value)}
                  />
                </Field>
                <Field label={t("connectors.connector")}>
                  <select
                    className={inputClass}
                    value={connectorId}
                    onChange={(event) => setConnectorId(event.target.value)}
                  >
                    {connectorList.map((entry) => (
                      <option key={entry.connectorId} value={entry.connectorId}>
                        {entry.name} · {entry.kind} · {entry.version}
                      </option>
                    ))}
                  </select>
                </Field>
                <Field label={t("connectors.config")} hint={t("connectors.configHint")}>
                  <textarea
                    className={cn(textareaClass, "min-h-[150px]")}
                    value={configText}
                    onChange={(event) => setConfigText(event.target.value)}
                    spellCheck={false}
                    aria-label={t("connectors.config")}
                  />
                </Field>
                <Field label={t("connectors.credentialEnv")} hint={t("connectors.credentialEnvHint")}>
                  <input
                    className={cn(inputClass, "font-mono text-[11px]")}
                    value={credentialEnv}
                    onChange={(event) => setCredentialEnv(event.target.value)}
                    placeholder="HPA_CONNECTOR_TOKEN"
                  />
                </Field>
                <Button
                  size="sm"
                  variant="primary"
                  loading={saveConnection.isPending}
                  disabled={!name.trim() || !connectorId}
                  onClick={() => saveConnection.mutate()}
                >
                  <Plus className="h-3.5 w-3.5" />
                  {t("common.save")}
                </Button>
                {activeConnector ? (
                  <p className="text-[10px] text-faint">
                    {t("connectors.configShapeHint", {
                      connector: `${activeConnector.connectorId}@${activeConnector.version}`,
                    })}
                  </p>
                ) : null}
              </div>
            </Panel>

            <Panel className="p-3">
              <div className="flex items-center gap-2">
                <Unplug className="h-4 w-4 text-accent" />
                <h2 className="text-sm font-semibold text-ink">{t("connectors.connectionsTitle")}</h2>
                <span className="ml-auto font-mono text-[10px] text-faint">
                  {connectionList.length}
                </span>
              </div>
              {connections.isLoading ? (
                <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
              ) : connections.isError ? (
                <p className="mt-2 text-xs text-danger">{t("errors.loadFailed")}</p>
              ) : connectionList.length === 0 ? (
                <EmptyState
                  icon={<Unplug className="h-4 w-4" />}
                  title={t("connectors.noConnections")}
                  hint={t("connectors.noConnectionsHint")}
                />
              ) : (
                <div className="mt-2 grid gap-1.5">
                  {connectionList.map((connection) => (
                    <div
                      key={connection.id}
                      className={cn(
                        "rounded-sm border px-2.5 py-2",
                        selectedId === connection.id
                          ? "border-accent/45 bg-accent/10"
                          : "border-line bg-panel",
                      )}
                    >
                      <button
                        type="button"
                        onClick={() => selectConnection(connection.id)}
                        className="block w-full text-left"
                      >
                        <div className="flex items-center gap-1.5">
                          <span className="min-w-0 flex-1 truncate text-[12px] font-semibold text-ink">
                            {connection.name}
                          </span>
                          <Badge tone={connection.status === "active" ? "success" : "neutral"}>
                            {t(`connectors.status.${connection.status}`, {
                              defaultValue: connection.status,
                            })}
                          </Badge>
                        </div>
                        <div className="mt-0.5 flex items-center gap-1.5 font-mono text-[9.5px] text-faint">
                          <span className="min-w-0 truncate">
                            {connection.connectorId}@{connection.connectorVersion}
                          </span>
                          {connection.credentialEnv ? (
                            <span className="shrink-0 text-accent">{connection.credentialEnv}</span>
                          ) : null}
                        </div>
                      </button>
                      <div className="mt-1 flex justify-end">
                        <Button
                          size="xs"
                          variant="ghost"
                          loading={
                            deleteConnection.isPending && deleteConnection.variables === connection.id
                          }
                          onClick={() => deleteConnection.mutate(connection.id)}
                        >
                          <Trash2 className="h-3 w-3" />
                          {t("common.delete")}
                        </Button>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </Panel>
          </div>

          <div className="grid min-w-0 content-start gap-3">
            {!selected ? (
              <Panel className="p-4">
                <EmptyState
                  icon={<Unplug className="h-4 w-4" />}
                  title={t("connectors.selectConnection")}
                  hint={t("connectors.selectConnectionHint")}
                />
              </Panel>
            ) : (
              <>
                <Panel className="p-4">
                  <div className="flex flex-wrap items-start gap-2">
                    <div className="min-w-0">
                      <h2 className="truncate text-[15px] font-semibold text-ink-strong">
                        {selected.name}
                      </h2>
                      <p className="mt-0.5 break-all font-mono text-[10px] text-faint">
                        {selected.connectorId}@{selected.connectorVersion} · {selected.id}
                      </p>
                    </div>
                    <Badge tone={selected.status === "active" ? "success" : "neutral"}>
                      {t(`connectors.status.${selected.status}`, { defaultValue: selected.status })}
                    </Badge>
                    <div className="ml-auto flex flex-wrap gap-1.5">
                      <Button
                        size="sm"
                        variant="secondary"
                        loading={testConnection.isPending}
                        onClick={() => testConnection.mutate(selected.id)}
                      >
                        <ShieldCheck className="h-3.5 w-3.5" />
                        {t("connectors.test")}
                      </Button>
                      <Button
                        size="sm"
                        variant="danger"
                        loading={
                          deleteConnection.isPending && deleteConnection.variables === selected.id
                        }
                        onClick={() => deleteConnection.mutate(selected.id)}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                        {t("common.delete")}
                      </Button>
                    </div>
                  </div>
                  <div className="mt-2 grid gap-2 md:grid-cols-2">
                    <div>
                      <SubTitle>{t("connectors.config")}</SubTitle>
                      <JsonView value={selected.config} maxHeight="150px" className="mt-1" />
                    </div>
                    <div className="grid content-start gap-1.5">
                      <div className="flex flex-wrap items-center gap-1.5 rounded-sm border border-line bg-panel-2 px-2 py-1.5">
                        <span className="text-[10px] uppercase tracking-[0.12em] text-faint">
                          {t("connectors.credentialEnv")}
                        </span>
                        <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-ink">
                          {selected.credentialEnv ?? t("common.none")}
                        </span>
                        <Badge tone={selected.hasCredential ? "accent" : "neutral"}>
                          {selected.hasCredential
                            ? t("connectors.credentialReference")
                            : t("connectors.noCredential")}
                        </Badge>
                      </div>
                      <p className="text-[10px] text-faint">{t("connectors.credentialEnvHint")}</p>
                      <p className="text-[10px] text-faint">{t("connectors.writeNote")}</p>
                    </div>
                  </div>
                  {testResult ? (
                    <div className="mt-2 grid gap-1 rounded-sm border border-line bg-panel-2 p-2">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <Badge tone="success">{t("connectors.test")}</Badge>
                        <span className="font-mono text-[10px] text-muted">
                          {t("connectors.latency")}: {testResult.latencyMs} ms
                        </span>
                      </div>
                      <JsonView value={testResult.value} maxHeight="140px" />
                    </div>
                  ) : null}
                </Panel>

                <Panel className="p-4">
                  <div className="flex flex-wrap items-center gap-2">
                    <Play className="h-4 w-4 text-accent" />
                    <h2 className="text-sm font-semibold text-ink">{t("connectors.invokeTitle")}</h2>
                    <select
                      className={cn(inputClass, "ml-auto h-7 w-auto font-mono text-[11px]")}
                      value={operation}
                      onChange={(event) => setOperation(event.target.value)}
                      aria-label={t("connectors.operation")}
                    >
                      {selected.operations.map((entry) => (
                        <option key={entry.name} value={entry.name}>
                          {entry.name} ·{" "}
                          {t(`connectors.risk.${entry.risk}`, { defaultValue: entry.risk })}
                        </option>
                      ))}
                    </select>
                    {selectedOperation ? (
                      <Badge tone={riskTone(selectedOperation.risk)}>
                        {t(`connectors.risk.${selectedOperation.risk}`, {
                          defaultValue: selectedOperation.risk,
                        })}
                      </Badge>
                    ) : null}
                  </div>
                  {selectedOperation?.description ? (
                    <p className="mt-1 text-[10.5px] text-faint">{selectedOperation.description}</p>
                  ) : null}
                  {selectedOperation && selectedOperation.risk !== "read" ? (
                    <p className="mt-1 rounded-sm border border-warning/30 bg-warning/10 px-2 py-1 text-[10px] text-warning">
                      {t("connectors.approvalRequired")}
                    </p>
                  ) : null}
                  <div className="mt-2 grid gap-2 xl:grid-cols-2">
                    <div className="grid content-start gap-1">
                      <SubTitle>{t("connectors.arguments")}</SubTitle>
                      <textarea
                        className={cn(textareaClass, "min-h-[130px]")}
                        value={argsText}
                        onChange={(event) => setArgsText(event.target.value)}
                        spellCheck={false}
                        aria-label={t("connectors.arguments")}
                      />
                      <div className="flex flex-wrap items-center gap-1.5">
                        <Button
                          size="sm"
                          variant="primary"
                          loading={invokeConnection.isPending}
                          disabled={!operation}
                          onClick={() => invokeConnection.mutate()}
                        >
                          <Play className="h-3.5 w-3.5" />
                          {t("connectors.invoke")}
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() =>
                            setArgsText(
                              JSON.stringify(defaultsForSchema(selectedOperation?.inputSchema), null, 2),
                            )
                          }
                        >
                          {t("connectors.resetArgs")}
                        </Button>
                        {invokeResult ? (
                          <span className="ml-auto font-mono text-[10px] text-muted">
                            {t("connectors.latency")}: {invokeResult.latencyMs} ms
                          </span>
                        ) : null}
                      </div>
                    </div>
                    <div className="grid content-start gap-1">
                      <SubTitle>{t("connectors.result")}</SubTitle>
                      {invokeResult ? (
                        <JsonView value={invokeResult.value} maxHeight="240px" />
                      ) : (
                        <p className="text-[11px] text-faint">{t("connectors.noResult")}</p>
                      )}
                    </div>
                  </div>
                </Panel>
              </>
            )}
          </div>
        </div>
      ) : null}

      {tab === "audit" ? (
        <Panel className="p-4">
          <div className="flex items-center gap-2">
            <ScrollText className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("connectors.auditTitle")}</h2>
            <span className="ml-auto font-mono text-[10px] text-faint">
              {audit.data?.entries.length ?? 0}
            </span>
          </div>
          {audit.isLoading ? (
            <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
          ) : audit.isError ? (
            <p className="mt-2 text-xs text-danger">{t("errors.loadFailed")}</p>
          ) : (audit.data?.entries ?? []).length === 0 ? (
            <EmptyState
              icon={<ScrollText className="h-4 w-4" />}
              title={t("connectors.noAudit")}
              hint={t("connectors.noAuditHint")}
            />
          ) : (
            <div className="mt-3 overflow-x-auto rounded-sm border border-line">
              <table className="w-full min-w-[620px] border-collapse text-left text-[11px]">
                <thead className="bg-panel-3 text-[9.5px] uppercase tracking-[0.12em] text-faint">
                  <tr>
                    <th className="px-3 py-1.5 font-medium">{t("common.status")}</th>
                    <th className="px-3 py-1.5 font-medium">{t("connectors.connection")}</th>
                    <th className="px-3 py-1.5 font-medium">{t("connectors.operation")}</th>
                    <th className="px-3 py-1.5 font-medium">{t("connectors.latency")}</th>
                    <th className="px-3 py-1.5 font-medium">{t("common.startedAt")}</th>
                    <th className="px-3 py-1.5 font-medium">{t("connectors.detail")}</th>
                  </tr>
                </thead>
                <tbody>
                  {(audit.data?.entries ?? []).map((entry) => (
                    <AuditRow key={entry.id} entry={entry} />
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Panel>
      ) : null}
    </div>
  );
}

function ConnectorGroup({
  icon,
  title,
  entries,
  loading,
  error,
}: {
  icon: ReactNode;
  title: string;
  entries: ConnectorDefinition[];
  loading: boolean;
  error: boolean;
}) {
  const { t } = useTranslation();
  return (
    <Panel className="p-4">
      <div className="flex items-center gap-2">
        {icon}
        <h2 className="text-sm font-semibold text-ink">{title}</h2>
        <span className="ml-auto font-mono text-[10px] text-faint">{entries.length}</span>
      </div>
      {loading ? (
        <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
      ) : error ? (
        <p className="mt-2 text-xs text-danger">{t("errors.loadFailed")}</p>
      ) : entries.length === 0 ? (
        <p className="mt-2 text-xs text-muted">{t("connectors.noConnectors")}</p>
      ) : (
        <div className="mt-2 grid gap-2">
          {entries.map((connector) => (
            <ConnectorCard key={connector.connectorId} connector={connector} />
          ))}
        </div>
      )}
    </Panel>
  );
}

function ConnectorCard({ connector }: { connector: ConnectorDefinition }) {
  const { t } = useTranslation();
  return (
    <div className="rounded-sm border border-line bg-panel-2 p-2.5">
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="text-[12px] font-semibold text-ink">{connector.name}</span>
        <Badge tone="accent">{connector.kind}</Badge>
        <Badge tone={connector.status === "enabled" ? "success" : "neutral"}>
          {connector.status}
        </Badge>
        <span className="ml-auto font-mono text-[9.5px] text-faint">
          {connector.connectorId}@{connector.version}
        </span>
      </div>
      {connector.pluginId ? (
        <p className="mt-0.5 font-mono text-[9px] text-faint">
          {t("connectors.plugin")}: {connector.pluginId}
          {connector.pluginVersion ? `@${connector.pluginVersion}` : ""}
        </p>
      ) : null}
      {connector.manifest.description ? (
        <p className="mt-1 text-[10.5px] text-muted">{connector.manifest.description}</p>
      ) : null}
      <div className="mt-1.5 grid gap-1">
        {connector.manifest.operations.map((operation) => (
          <details key={operation.name} className="group rounded-xs border border-line bg-panel">
            <summary className="flex cursor-pointer flex-wrap items-center gap-1.5 px-2 py-1.5">
              <span className="font-mono text-[10.5px] text-ink">{operation.name}</span>
              <Badge tone={riskTone(operation.risk)}>
                {t(`connectors.risk.${operation.risk}`, { defaultValue: operation.risk })}
              </Badge>
              {operation.risk !== "read" ? (
                <Badge tone="warning">{t("connectors.approvalShort")}</Badge>
              ) : null}
              <span className="min-w-0 flex-1 truncate text-[10px] text-faint">
                {operation.description}
              </span>
            </summary>
            <div className="border-t border-line px-2 py-1.5">
              <SubTitle>{t("connectors.inputSchema")}</SubTitle>
              <JsonView value={operation.inputSchema} maxHeight="150px" className="mt-1" />
            </div>
          </details>
        ))}
      </div>
      <div className="mt-1.5">
        <SubTitle>{t("connectors.configShape")}</SubTitle>
        <JsonView value={connector.configShape} maxHeight="130px" className="mt-1" />
      </div>
      <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
        <Globe2 className="h-3 w-3 text-faint" />
        <span className="font-mono text-[9px] text-faint">
          {t("connectors.created")}: <Timestamp value={connector.createdAt} />
        </span>
      </div>
    </div>
  );
}

function AuditRow({ entry }: { entry: ConnectorAuditEntry }) {
  const { t } = useTranslation();
  const summary =
    typeof entry.detail === "string" ? entry.detail : JSON.stringify(entry.detail ?? null);
  return (
    <tr className="border-t border-line">
      <td className="px-3 py-1.5">
        <Badge tone={auditTone(entry.status)}>
          {t(`connectors.status.${entry.status}`, { defaultValue: entry.status })}
        </Badge>
      </td>
      <td className="px-3 py-1.5 font-mono text-ink">{entry.connectionId}</td>
      <td className="px-3 py-1.5 font-mono text-muted">{entry.operation}</td>
      <td className="px-3 py-1.5 font-mono text-muted">{entry.latencyMs} ms</td>
      <td className="px-3 py-1.5 font-mono text-faint">
        <Timestamp value={entry.at} />
      </td>
      <td
        className="max-w-[240px] truncate px-3 py-1.5 font-mono text-[9.5px] text-faint"
        title={summary}
      >
        {summary}
      </td>
    </tr>
  );
}

function SubTitle({ children }: { children: ReactNode }) {
  return (
    <p className="text-[10px] font-semibold uppercase tracking-[0.12em] text-faint">{children}</p>
  );
}

function defaultsForSchema(schema: JsonValue | undefined): JsonValue {
  if (!schema || typeof schema !== "object") return {};
  const properties = (schema as Record<string, JsonValue>).properties;
  if (!properties || typeof properties !== "object") return {};
  const result: Record<string, JsonValue> = {};
  for (const [key, value] of Object.entries(properties as Record<string, JsonValue>)) {
    if (!value || typeof value !== "object") {
      result[key] = "";
      continue;
    }
    const property = value as Record<string, JsonValue>;
    const fallback = property.default;
    if (fallback !== undefined) {
      result[key] = fallback;
    } else if (property.enum && Array.isArray(property.enum) && property.enum.length > 0) {
      result[key] = property.enum[0] ?? "";
    } else if (property.type === "object") {
      result[key] = {};
    } else if (property.type === "array") {
      result[key] = [];
    } else if (property.type === "integer" || property.type === "number") {
      result[key] = 0;
    } else if (property.type === "boolean") {
      result[key] = false;
    } else {
      result[key] = "";
    }
  }
  return result;
}

function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof ApiError) return error.message || fallback;
  return error instanceof Error ? error.message : fallback;
}
