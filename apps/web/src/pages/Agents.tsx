import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Bot,
  Boxes,
  Braces,
  ChevronRight,
  FileJson,
  FlaskConical,
  History,
  Network,
  Play,
  Plus,
  Puzzle,
  RefreshCw,
  Rocket,
  RotateCcw,
  ShieldCheck,
  Sparkles,
  Square,
  Workflow,
} from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  Badge,
  Button,
  EmptyState,
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
  AgentSessionDetail,
  AgentStatus,
  JsonValue,
} from "../lib/types";
import { useUiStore } from "../stores/ui";

type AgentTab = "overview" | "designer" | "sessions";

function statusTone(status: AgentStatus): "success" | "warning" | "danger" | "info" | "neutral" {
  if (status === "succeeded") return "success";
  if (status === "waiting_approval" || status === "interrupted") return "warning";
  if (status === "failed" || status === "budget_exhausted" || status === "timeout") return "danger";
  if (status === "running") return "info";
  return "neutral";
}

function riskTone(risk: string): "danger" | "warning" | "neutral" {
  if (risk === "high") return "danger";
  if (risk === "medium") return "warning";
  return "neutral";
}

function invoiceInput(): JsonValue {
  return {
    invoiceId: "inv_1001",
    items: [
      { name: "license", amount: 120.5 },
      { name: "support", amount: 79.5 },
    ],
  };
}

function workflowInput(): JsonValue {
  return {
    workflowId: "wf_sample_approval",
    workflowVersion: "1.0.0",
    input: {},
  };
}

function agentConfigTemplate(kind: "deterministic" | "model" | "supervisor"): JsonValue {
  if (kind === "model") {
    return {
      schemaVersion: "hpa.agent/v1",
      id: "agent_invoice_model",
      name: "Invoice Model Agent",
      version: "1.0.0",
      description: "Optional OpenAI-compatible planner with the invoice-audit skill.",
      instructions: "Use the invoice-audit skill for exact totals. Return a short JSON summary.",
      tools: ["skill.invoice-audit.audit"],
      planner: {
        kind: "openAi",
        endpoint: "http://127.0.0.1:11434/v1/chat/completions",
        model: "local-model",
        temperature: 0,
        inputCostPer1k: 0,
        outputCostPer1k: 0,
        network: [],
      },
      limits: {
        maxSteps: 10,
        maxToolCalls: 4,
        timeoutMs: 60000,
        maxTokens: 16000,
        maxCostUsd: 0.5,
        maxHandoffs: 2,
        maxDepth: 2,
      },
      approval: [],
      subAgents: [],
    };
  }
  if (kind === "supervisor") {
    return {
      schemaVersion: "hpa.agent/v1",
      id: "agent_invoice_supervisor",
      name: "Invoice Supervisor",
      version: "1.0.0",
      description: "Hands the invoice to a declared sub-agent and inherits the remaining budget.",
      tools: [],
      planner: {
        kind: "deterministic",
        plan: [
          {
            kind: "handoff",
            agentId: "agent_invoice_auditor",
            input: { $input: true },
          },
          { kind: "respond", output: { $last: true } },
        ],
      },
      limits: {
        maxSteps: 8,
        maxToolCalls: 4,
        timeoutMs: 60000,
        maxTokens: 8000,
        maxCostUsd: 0.25,
        maxHandoffs: 2,
        maxDepth: 2,
      },
      approval: [],
      subAgents: ["agent_invoice_auditor"],
    };
  }
  return {
    schemaVersion: "hpa.agent/v1",
    id: "agent_invoice_auditor",
    name: "Invoice Auditor",
    version: "1.0.0",
    description: "Runs the published invoice-audit skill deterministically.",
    tools: ["skill.invoice-audit.audit"],
    planner: {
      kind: "deterministic",
      plan: [
        {
          kind: "tool",
          tool: "skill.invoice-audit.audit",
          arguments: {
            invoiceId: { $input: "invoiceId" },
            items: { $input: "items" },
          },
        },
        { kind: "respond", output: { audit: { $last: true } } },
      ],
    },
    limits: {
      maxSteps: 8,
      maxToolCalls: 4,
      timeoutMs: 60000,
      maxTokens: 8000,
      maxCostUsd: 0.25,
      maxHandoffs: 2,
      maxDepth: 2,
    },
    approval: [],
    subAgents: [],
  };
}

function sampleAgents(): JsonValue[] {
  return [
    agentConfigTemplate("deterministic"),
    {
      schemaVersion: "hpa.agent/v1",
      id: "agent_workflow_supervisor",
      name: "Workflow Supervisor",
      version: "1.0.0",
      description: "Starts a published workflow and waits for a checkpoint or terminal status.",
      tools: ["workflow.execute", "execution.wait", "execution.get"],
      planner: {
        kind: "deterministic",
        plan: [
          {
            kind: "tool",
            tool: "workflow.execute",
            arguments: {
              workflowId: { $input: "workflowId" },
              version: { $input: "workflowVersion" },
              input: { $input: "input" },
            },
          },
          {
            kind: "tool",
            tool: "execution.wait",
            arguments: {
              executionId: { $last: "executionId" },
              timeoutMs: 15000,
            },
          },
          { kind: "respond", output: { execution: { $last: true } } },
        ],
      },
      limits: {
        maxSteps: 8,
        maxToolCalls: 4,
        timeoutMs: 60000,
        maxTokens: 8000,
        maxCostUsd: 0.25,
        maxHandoffs: 2,
        maxDepth: 2,
      },
      approval: [],
      subAgents: [],
    },
    agentConfigTemplate("supervisor"),
    agentConfigTemplate("model"),
  ];
}

export function AgentsPage() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [version, setVersion] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [tab, setTab] = useState<AgentTab>("overview");
  const [configText, setConfigText] = useState("");
  const [inputText, setInputText] = useState(JSON.stringify(invoiceInput(), null, 2));
  const [session, setSession] = useState<AgentSessionDetail | null>(null);
  const [replay, setReplay] = useState<JsonValue | null>(null);
  const [newOpen, setNewOpen] = useState(false);
  const [newConfig, setNewConfig] = useState(() =>
    JSON.stringify(agentConfigTemplate("deterministic"), null, 2),
  );
  const [busySamples, setBusySamples] = useState(false);
  const [configError, setConfigError] = useState<string | null>(null);

  const agents = useQuery({ queryKey: ["agents"], queryFn: api.listAgents, retry: 1 });
  const detail = useQuery({
    queryKey: ["agent", selectedId],
    queryFn: () => api.getAgent(selectedId ?? ""),
    enabled: Boolean(selectedId),
    retry: 1,
  });
  const versionDetail = useQuery({
    queryKey: ["agent-version", selectedId, version],
    queryFn: () => api.getAgentVersion(selectedId ?? "", version ?? ""),
    enabled: Boolean(selectedId && version),
    retry: 1,
  });
  const sessions = useQuery({
    queryKey: ["agent-sessions", selectedId],
    queryFn: () => api.listAgentSessions(selectedId ?? undefined, 30),
    enabled: Boolean(selectedId),
    retry: 1,
    refetchInterval: 5000,
  });
  const allSessions = useQuery({
    queryKey: ["agent-sessions", "all"],
    queryFn: () => api.listAgentSessions(undefined, 100),
    retry: 1,
    refetchInterval: 5000,
  });
  const approvals = useQuery({
    queryKey: ["agent-approvals"],
    queryFn: () => api.listAgentApprovals("pending"),
    retry: 1,
    refetchInterval: 3000,
  });
  const tools = useQuery({ queryKey: ["agent-tools"], queryFn: api.agentTools, retry: 1 });
  const runtime = useQuery({ queryKey: ["agent-runtime"], queryFn: api.agentRuntime, retry: 1 });

  const agentList = agents.data?.agents ?? [];
  const sessionList = sessions.data?.sessions ?? [];
  const allSessionList = allSessions.data?.sessions ?? [];
  const pendingApprovals = approvals.data?.approvals ?? [];
  const publishedCount = agentList.filter((agent) => agent.status === "published").length;
  const activeCount = allSessionList.filter((row) =>
    ["running", "interrupted", "waiting_approval"].includes(row.status),
  ).length;

  useEffect(() => {
    if (!selectedId && agentList.length > 0) {
      setSelectedId(agentList[0]!.id);
    }
  }, [agentList, selectedId]);

  useEffect(() => {
    if (!detail.data) return;
    setVersion((current) => {
      if (current && detail.data?.versions.some((entry) => entry.version === current)) {
        return current;
      }
      return (
        detail.data?.publishedVersion?.version ??
        detail.data?.summary.latestVersion ??
        detail.data?.versions[0]?.version ??
        null
      );
    });
  }, [detail.data]);

  useEffect(() => {
    if (versionDetail.data) {
      setConfigText(JSON.stringify(versionDetail.data.config, null, 2));
      setConfigError(null);
    }
  }, [versionDetail.data]);

  useEffect(() => {
    const config = versionDetail.data?.config ?? detail.data?.latestConfig;
    if (!config || typeof config !== "object") return;
    const tools = (config as { tools?: unknown }).tools;
    const entries = Array.isArray(tools) ? tools.filter((tool): tool is string => typeof tool === "string") : [];
    if (entries.includes("workflow.execute")) {
      setInputText(JSON.stringify(workflowInput(), null, 2));
    } else if (entries.some((tool) => tool.startsWith("skill."))) {
      setInputText(JSON.stringify(invoiceInput(), null, 2));
    }
  }, [selectedId, versionDetail.data?.version, detail.data?.latestConfig]);

  function selectAgent(id: string) {
    if (id === selectedId) return;
    setSelectedId(id);
    setVersion(null);
    setSession(null);
    setReplay(null);
    setTab("overview");
  }

  const createAgent = useMutation({
    mutationFn: async (text: string) => {
      let config: JsonValue;
      try {
        config = JSON.parse(text) as JsonValue;
      } catch {
        throw new ApiError(0, "AGENT_JSON", t("agents.configInvalid"));
      }
      return api.createAgent(config);
    },
    onSuccess: (result) => {
      pushToast(
        result.inserted ? t("agents.versionCreated") : t("agents.versionExists"),
        "success",
      );
      setNewOpen(false);
      setConfigError(null);
      setSelectedId(result.summary.id);
      setVersion(result.version.version);
      setTab("designer");
      void queryClient.invalidateQueries({ queryKey: ["agents"] });
      void queryClient.invalidateQueries({ queryKey: ["agent", result.summary.id] });
      void queryClient.invalidateQueries({ queryKey: ["agent-version", result.summary.id] });
    },
    onError: (error) => {
      const message = errorMessage(error, t("agents.createFailed"));
      setConfigError(message);
      pushToast(message, "error");
    },
  });

  const publishAgent = useMutation({
    mutationFn: () => api.publishAgent(selectedId ?? "", version ?? undefined),
    onSuccess: () => {
      pushToast(t("agents.publishedToast"), "success");
      void queryClient.invalidateQueries({ queryKey: ["agents"] });
      void queryClient.invalidateQueries({ queryKey: ["agent", selectedId] });
    },
    onError: (error) => pushToast(errorMessage(error, t("agents.publishFailed")), "error"),
  });

  const runAgent = useMutation({
    mutationFn: async () => {
      let input: JsonValue;
      try {
        input = JSON.parse(inputText) as JsonValue;
      } catch {
        throw new ApiError(0, "INPUT_JSON", t("agents.inputInvalid"));
      }
      return api.runAgent(selectedId ?? "", input, version ?? undefined);
    },
    onSuccess: (result) => {
      setSession(result);
      setReplay(null);
      setTab("overview");
      pushToast(
        result.status === "waiting_approval"
          ? t("agents.runWaiting")
          : result.status === "succeeded"
            ? t("agents.runDone")
            : t("agents.runFailed"),
        result.status === "succeeded"
          ? "success"
          : result.status === "waiting_approval"
            ? "warning"
            : "error",
      );
      void queryClient.invalidateQueries({ queryKey: ["agent-sessions", selectedId] });
      void queryClient.invalidateQueries({ queryKey: ["agent-approvals"] });
      void queryClient.invalidateQueries({ queryKey: ["agents"] });
    },
    onError: (error) => pushToast(errorMessage(error, t("agents.runFailed")), "error"),
  });

  const openSession = useMutation({
    mutationFn: (id: string) => api.getAgentSession(id),
    onSuccess: (result) => {
      setSession((current) => {
        if (
          current &&
          current.agentId === result.agentId &&
          current.createdAt > result.createdAt
        ) {
          return current;
        }
        return result;
      });
      setReplay(null);
      setTab("overview");
    },
    onError: (error) => pushToast(errorMessage(error, t("agents.loadFailed")), "error"),
  });

  useEffect(() => {
    if (!selectedId) return;
    if (session?.agentId === selectedId) return;
    const latest = sessions.data?.sessions[0];
    if (latest) {
      openSession.mutate(latest.id);
    }
  }, [selectedId, sessions.data?.sessions.length, session?.agentId]);

  const continueSession = useMutation({
    mutationFn: (id: string) => api.continueAgentSession(id),
    onSuccess: (result) => {
      setSession(result);
      pushToast(t("agents.continued"), "success");
      void queryClient.invalidateQueries({ queryKey: ["agent-sessions", selectedId] });
      void queryClient.invalidateQueries({ queryKey: ["agent-approvals"] });
    },
    onError: (error) => pushToast(errorMessage(error, t("agents.actionFailed")), "error"),
  });

  const cancelSession = useMutation({
    mutationFn: (id: string) => api.cancelAgentSession(id),
    onSuccess: (result) => {
      setSession(result);
      pushToast(t("agents.cancelled"), "success");
      void queryClient.invalidateQueries({ queryKey: ["agent-sessions", selectedId] });
    },
    onError: (error) => pushToast(errorMessage(error, t("agents.actionFailed")), "error"),
  });

  const replaySession = useMutation({
    mutationFn: (id: string) => api.replayAgentSession(id),
    onSuccess: (result) => {
      setReplay(result as unknown as JsonValue);
      pushToast(
        result.matches ? t("agents.replayMatch") : t("agents.replayMismatch"),
        result.matches ? "success" : "warning",
      );
    },
    onError: (error) => pushToast(errorMessage(error, t("agents.replayFailed")), "error"),
  });

  const decideApproval = useMutation({
    mutationFn: ({ id, decision }: { id: string; decision: "approved" | "rejected" }) =>
      api.decideAgentApproval(id, decision),
    onSuccess: (result) => {
      setSession(result.session);
      pushToast(t("agents.approvalDecided"), "success");
      void queryClient.invalidateQueries({ queryKey: ["agent-approvals"] });
      void queryClient.invalidateQueries({ queryKey: ["agent-sessions", selectedId] });
      void queryClient.invalidateQueries({ queryKey: ["agents"] });
    },
    onError: (error) => pushToast(errorMessage(error, t("agents.approvalFailed")), "error"),
  });

  async function loadSamples() {
    setBusySamples(true);
    try {
      let skillReady = false;
      try {
        const imported = await api.importSkill("fixtures/skills/invoice-audit");
        await api.publishSkill(
          imported.skill.skill.id,
          imported.skill.skill.version,
        );
        skillReady = true;
      } catch {
        const existing = await api.listSkills();
        skillReady = existing.skills.some(
          (skill) => skill.name === "invoice-audit" && skill.status === "published",
        );
      }
      for (const config of sampleAgents()) {
        const result = await api.createAgent(config);
        await api.publishAgent(result.summary.id, result.version.version);
      }
      pushToast(
        skillReady ? t("agents.samplesAdded") : t("agents.samplesPartial"),
        skillReady ? "success" : "warning",
      );
      void queryClient.invalidateQueries();
      setSelectedId("agent_invoice_auditor");
      setVersion("1.0.0");
      setInputText(JSON.stringify(invoiceInput(), null, 2));
    } catch (error) {
      pushToast(errorMessage(error, t("agents.samplesFailed")), "error");
    } finally {
      setBusySamples(false);
    }
  }

  const filtered = useMemo(
    () =>
      agentList.filter((agent) => {
        const query = search.trim().toLowerCase();
        if (!query) return true;
        return (
          agent.id.toLowerCase().includes(query) ||
          agent.name.toLowerCase().includes(query)
        );
      }),
    [agentList, search],
  );

  const selected = detail.data;
  const selectedVersion = detail.data?.versions.find((entry) => entry.version === version);
  const selectedConfig = versionDetail.data?.config ?? detail.data?.latestConfig;

  return (
    <div className="mx-auto grid max-w-[1560px] gap-3">
      <PageHeader
        title={t("agents.title")}
        subtitle={t("agents.subtitle")}
        actions={
          <>
            <Button
              variant="secondary"
              onClick={() => void queryClient.invalidateQueries()}
            >
              <RefreshCw className="h-3.5 w-3.5" />
              {t("common.refresh")}
            </Button>
            <Button
              variant="secondary"
              loading={busySamples}
              onClick={() => void loadSamples()}
            >
              <Sparkles className="h-3.5 w-3.5" />
              {t("agents.loadSamples")}
            </Button>
            <Button variant="primary" onClick={() => setNewOpen((value) => !value)}>
              <Plus className="h-3.5 w-3.5" />
              {t("agents.newAgent")}
            </Button>
          </>
        }
      />

      {newOpen ? (
        <Panel className="p-4">
          <div className="flex flex-wrap items-center gap-2">
            <FileJson className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("agents.newTitle")}</h2>
            <span className="text-[10.5px] text-faint">{t("agents.newHint")}</span>
          </div>
          <div className="mt-2 flex flex-wrap gap-1.5">
            <Button
              size="sm"
              variant="subtle"
              onClick={() => setNewConfig(JSON.stringify(agentConfigTemplate("deterministic"), null, 2))}
            >
              {t("agents.template.deterministic")}
            </Button>
            <Button
              size="sm"
              variant="subtle"
              onClick={() => setNewConfig(JSON.stringify(agentConfigTemplate("model"), null, 2))}
            >
              {t("agents.template.model")}
            </Button>
            <Button
              size="sm"
              variant="subtle"
              onClick={() => setNewConfig(JSON.stringify(agentConfigTemplate("supervisor"), null, 2))}
            >
              {t("agents.template.supervisor")}
            </Button>
          </div>
          <textarea
            className={cn(textareaClass, "mt-2 min-h-[260px]")}
            value={newConfig}
            onChange={(event) => setNewConfig(event.target.value)}
            spellCheck={false}
            aria-label={t("agents.configJson")}
          />
          {configError ? (
            <p className="mt-2 rounded-sm border border-danger/30 bg-danger/10 px-2 py-1.5 text-[11px] text-danger">
              {configError}
            </p>
          ) : null}
          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            <Button
              variant="primary"
              loading={createAgent.isPending}
              onClick={() => createAgent.mutate(newConfig)}
            >
              <Rocket className="h-3.5 w-3.5" />
              {t("agents.createVersion")}
            </Button>
            <Button variant="ghost" onClick={() => setNewOpen(false)}>
              {t("common.cancel")}
            </Button>
            <span className="ml-auto text-[10px] text-faint">{t("agents.schemaNote")}</span>
          </div>
        </Panel>
      ) : null}

      <div className="grid grid-cols-2 gap-2 min-[560px]:grid-cols-3 xl:grid-cols-5">
        <StatCard label={t("agents.statAgents")} value={agentList.length} />
        <StatCard label={t("agents.statPublished")} value={publishedCount} />
        <StatCard label={t("agents.statSessions")} value={allSessionList.length} />
        <StatCard label={t("agents.statActive")} value={activeCount} />
        <StatCard label={t("agents.statApprovals")} value={pendingApprovals.length} />
      </div>

      <div className="grid gap-3 xl:grid-cols-[250px_minmax(0,1fr)_320px]">
        <Panel className="p-3">
          <div className="flex items-center gap-2">
            <Bot className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("agents.registry")}</h2>
            <span className="ml-auto font-mono text-[10px] text-faint">{agentList.length}</span>
          </div>
          <input
            className={cn(inputClass, "mt-2")}
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder={t("agents.searchPlaceholder")}
            aria-label={t("common.search")}
          />
          <div className="mt-2 grid gap-1.5">
            {agents.isError ? (
              <p className="text-xs text-danger">{t("errors.loadFailed")}</p>
            ) : filtered.length === 0 ? (
              <EmptyState
                icon={<Bot className="h-4 w-4" />}
                title={t("agents.empty")}
                hint={t("agents.emptyHint")}
              />
            ) : (
              filtered.map((agent) => (
                <button
                  key={agent.id}
                  type="button"
                  onClick={() => selectAgent(agent.id)}
                  className={cn(
                    "rounded-sm border px-2.5 py-2 text-left transition-colors",
                    selectedId === agent.id
                      ? "border-accent/45 bg-accent/10"
                      : "border-line bg-panel hover:border-line-strong",
                  )}
                >
                  <div className="flex items-center gap-1.5">
                    <span className="min-w-0 flex-1 truncate text-[12px] font-semibold text-ink">
                      {agent.name}
                    </span>
                    <Badge tone={agent.status === "published" ? "success" : "warning"}>
                      {agent.status === "published" ? t("agents.published") : t("agents.draft")}
                    </Badge>
                  </div>
                  <div className="mt-0.5 flex items-center gap-1.5 font-mono text-[9.5px] text-faint">
                    <span className="min-w-0 truncate">{agent.id}</span>
                    <span>·</span>
                    <span>{agent.latestVersion}</span>
                  </div>
                </button>
              ))
            )}
          </div>
        </Panel>

        <div className="grid content-start gap-3">
          {!selected ? (
            <Panel className="p-4">
              <EmptyState
                icon={<Bot className="h-4 w-4" />}
                title={t("agents.selectAgent")}
                hint={t("agents.selectAgentHint")}
              />
            </Panel>
          ) : (
            <>
              <Panel className="p-4">
                <div className="flex flex-wrap items-center gap-2">
                  <div className="min-w-0">
                    <h2 className="truncate text-[15px] font-semibold text-ink-strong">
                      {selected.summary.name}
                    </h2>
                    <p className="mt-0.5 font-mono text-[10px] text-faint">
                      {selected.summary.id} · {selected.summary.latestVersion}
                    </p>
                  </div>
                  <Badge tone={selected.publishedVersion ? "success" : "warning"}>
                    {selected.publishedVersion
                      ? t("agents.publishedVersion")
                      : t("agents.notPublished")}
                  </Badge>
                  <span className="ml-auto font-mono text-[9.5px] text-faint">
                    {selectedVersion?.contentHash.slice(0, 12) ?? "—"}
                  </span>
                </div>
                <div className="mt-3 flex flex-wrap items-center gap-1.5">
                  <select
                    className={cn(inputClass, "h-7 w-auto min-w-[130px]")}
                    value={version ?? ""}
                    onChange={(event) => setVersion(event.target.value || null)}
                    aria-label={t("common.version")}
                  >
                    {selected.versions.map((entry) => (
                      <option key={entry.version} value={entry.version}>
                        {entry.version} ·{" "}
                        {entry.status === "published" ? t("agents.published") : t("agents.draft")}
                      </option>
                    ))}
                  </select>
                  <Button
                    size="sm"
                    variant="secondary"
                    loading={publishAgent.isPending}
                    disabled={!selectedVersion || selectedVersion.status === "published"}
                    onClick={() => publishAgent.mutate()}
                  >
                    <Rocket className="h-3.5 w-3.5" />
                    {t("agents.publish")}
                  </Button>
                  <Button
                    size="sm"
                    variant="primary"
                    loading={runAgent.isPending}
                    onClick={() => runAgent.mutate()}
                  >
                    <Play className="h-3.5 w-3.5" />
                    {t("agents.runAgent")}
                  </Button>
                  <span className="ml-auto text-[10px] text-faint">
                    {t("agents.approvalBoundary")}
                  </span>
                </div>
                <div className="mt-3 flex flex-wrap gap-1">
                  {(
                    [
                      ["overview", t("agents.tab.overview"), <FlaskConical key="o" className="h-3 w-3" />],
                      ["designer", t("agents.tab.designer"), <Braces key="d" className="h-3 w-3" />],
                      ["sessions", t("agents.tab.sessions"), <History key="s" className="h-3 w-3" />],
                    ] as [AgentTab, string, ReactNode][]
                  ).map(([key, label, icon]) => (
                    <button
                      key={key}
                      type="button"
                      onClick={() => setTab(key)}
                      className={cn(
                        "inline-flex h-6 items-center gap-1.5 rounded-sm border px-2 text-[11px] font-medium",
                        tab === key
                          ? "border-accent/40 bg-accent/10 text-accent"
                          : "border-line bg-panel text-muted hover:text-ink",
                      )}
                    >
                      {icon}
                      {label}
                    </button>
                  ))}
                </div>
              </Panel>

              {tab === "overview" ? (
                <>
                  <Panel className="p-4">
                    <div className="flex items-center gap-2">
                      <Play className="h-4 w-4 text-accent" />
                      <h2 className="text-sm font-semibold text-ink">{t("agents.testRun")}</h2>
                      <span className="text-[10.5px] text-faint">{t("agents.testRunHint")}</span>
                    </div>
                    <textarea
                      className={cn(textareaClass, "mt-2 min-h-[130px]")}
                      value={inputText}
                      onChange={(event) => setInputText(event.target.value)}
                      spellCheck={false}
                      aria-label={t("agents.inputJson")}
                    />
                    <div className="mt-2 flex flex-wrap items-center gap-1.5">
                      <Button
                        variant="primary"
                        loading={runAgent.isPending}
                        onClick={() => runAgent.mutate()}
                      >
                        <Play className="h-3.5 w-3.5" />
                        {t("agents.runAgent")}
                      </Button>
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => {
                          const config = selectedConfig;
                          const tools =
                            config && typeof config === "object" && Array.isArray((config as { tools?: unknown }).tools)
                              ? ((config as { tools: string[] }).tools ?? [])
                              : [];
                          setInputText(
                            JSON.stringify(
                              tools.includes("workflow.execute") ? workflowInput() : invoiceInput(),
                              null,
                              2,
                            ),
                          );
                        }}
                      >
                        <RotateCcw className="h-3 w-3" />
                        {t("agents.exampleInput")}
                      </Button>
                      <span className="ml-auto text-[10px] text-faint">
                        {t("agents.limitHint", {
                          steps: runtime.data?.defaultLimits.maxSteps ?? 12,
                          tools: runtime.data?.defaultLimits.maxToolCalls ?? 6,
                        })}
                      </span>
                    </div>
                  </Panel>

                  {session ? (
                    <SessionPanel
                      session={session}
                      replay={replay}
                      onReplay={() => replaySession.mutate(session.id)}
                      onCancel={() => cancelSession.mutate(session.id)}
                      onContinue={() => continueSession.mutate(session.id)}
                      replayBusy={replaySession.isPending}
                      cancelBusy={cancelSession.isPending}
                      continueBusy={continueSession.isPending}
                    />
                  ) : (
                    <Panel className="p-4">
                      <EmptyState
                        icon={<FlaskConical className="h-4 w-4" />}
                        title={t("agents.noSession")}
                        hint={t("agents.noSessionHint")}
                      />
                    </Panel>
                  )}
                </>
              ) : null}

              {tab === "designer" ? (
                <>
                  <Panel className="p-4">
                    <div className="flex flex-wrap items-center gap-2">
                      <FileJson className="h-4 w-4 text-accent" />
                      <h2 className="text-sm font-semibold text-ink">{t("agents.designerTitle")}</h2>
                      <span className="text-[10.5px] text-faint">{t("agents.designerHint")}</span>
                    </div>
                    <textarea
                      className={cn(textareaClass, "mt-2 min-h-[360px]")}
                      value={configText}
                      onChange={(event) => setConfigText(event.target.value)}
                      spellCheck={false}
                      aria-label={t("agents.configJson")}
                    />
                    {configError ? (
                      <p className="mt-2 whitespace-pre-wrap rounded-sm border border-danger/30 bg-danger/10 px-2 py-1.5 text-[11px] text-danger">
                        {configError}
                      </p>
                    ) : null}
                    <div className="mt-2 flex flex-wrap items-center gap-1.5">
                      <Button
                        variant="primary"
                        loading={createAgent.isPending}
                        onClick={() => createAgent.mutate(configText)}
                      >
                        <Plus className="h-3.5 w-3.5" />
                        {t("agents.createVersion")}
                      </Button>
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => {
                          if (versionDetail.data) {
                            setConfigText(JSON.stringify(versionDetail.data.config, null, 2));
                            setConfigError(null);
                          }
                        }}
                      >
                        <RotateCcw className="h-3 w-3" />
                        {t("agents.resetConfig")}
                      </Button>
                      <span className="ml-auto text-[10px] text-faint">{t("agents.immutableHint")}</span>
                    </div>
                  </Panel>

                  <Panel className="p-4">
                    <div className="flex items-center gap-2">
                      <History className="h-4 w-4 text-accent" />
                      <h2 className="text-sm font-semibold text-ink">{t("agents.versionList")}</h2>
                    </div>
                    <div className="mt-2 overflow-x-auto">
                      <table className="w-full min-w-[480px] text-left text-[11px]">
                        <thead className="text-[9.5px] uppercase tracking-[0.12em] text-faint">
                          <tr>
                            <th className="px-2 py-1 font-medium">{t("common.version")}</th>
                            <th className="px-2 py-1 font-medium">{t("common.status")}</th>
                            <th className="px-2 py-1 font-medium">{t("agents.hash")}</th>
                            <th className="px-2 py-1 font-medium">{t("common.createdAt")}</th>
                            <th className="px-2 py-1 font-medium" />
                          </tr>
                        </thead>
                        <tbody>
                          {selected.versions.map((entry) => (
                            <tr key={entry.version} className="border-t border-line">
                              <td className="px-2 py-1.5 font-mono text-ink">{entry.version}</td>
                              <td className="px-2 py-1.5">
                                <Badge tone={entry.status === "published" ? "success" : "warning"}>
                                  {entry.status === "published"
                                    ? t("agents.published")
                                    : t("agents.draft")}
                                </Badge>
                              </td>
                              <td className="px-2 py-1.5 font-mono text-[9.5px] text-faint">
                                {entry.contentHash.slice(0, 16)}
                              </td>
                              <td className="px-2 py-1.5 text-faint">
                                <Timestamp value={entry.createdAt} />
                              </td>
                              <td className="px-2 py-1.5 text-right">
                                <Button
                                  size="xs"
                                  variant="ghost"
                                  onClick={() => {
                                    setVersion(entry.version);
                                    setConfigError(null);
                                  }}
                                >
                                  {t("common.open")}
                                </Button>
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  </Panel>
                </>
              ) : null}

              {tab === "sessions" ? (
                <Panel className="p-4">
                  <div className="flex items-center gap-2">
                    <History className="h-4 w-4 text-accent" />
                    <h2 className="text-sm font-semibold text-ink">{t("agents.sessionsTitle")}</h2>
                    <span className="ml-auto font-mono text-[10px] text-faint">
                      {sessionList.length}
                    </span>
                  </div>
                  {sessionList.length === 0 ? (
                    <EmptyState
                      icon={<History className="h-4 w-4" />}
                      title={t("agents.noSessions")}
                      hint={t("agents.noSessionsHint")}
                    />
                  ) : (
                    <div className="mt-2 grid gap-1.5">
                      {sessionList.map((row) => (
                        <button
                          key={row.id}
                          type="button"
                          onClick={() => openSession.mutate(row.id)}
                          className="flex flex-wrap items-center gap-2 rounded-sm border border-line bg-panel px-2.5 py-2 text-left hover:border-accent/40"
                        >
                          <Badge tone={statusTone(row.status)} dot>
                            {t(`status.${row.status}`, { defaultValue: row.status })}
                          </Badge>
                          <span className="font-mono text-[10px] text-ink">{row.id}</span>
                          <span className="font-mono text-[9.5px] text-faint">
                            {row.agentVersion} · {row.steps} {t("agents.metric.steps")} ·{" "}
                            {row.toolCalls} {t("agents.metric.toolCalls")}
                          </span>
                          <span className="ml-auto text-[9.5px] text-faint">
                            <Timestamp value={row.createdAt} />
                          </span>
                          <ChevronRight className="h-3 w-3 text-faint" />
                        </button>
                      ))}
                    </div>
                  )}
                </Panel>
              ) : null}
            </>
          )}
        </div>

        <div className="grid content-start gap-3">
          <Panel className="p-4">
            <div className="flex items-center gap-2">
              <ShieldCheck className="h-4 w-4 text-accent" />
              <h2 className="text-sm font-semibold text-ink">{t("agents.approvalsTitle")}</h2>
              <Badge tone={pendingApprovals.length > 0 ? "warning" : "neutral"}>
                {pendingApprovals.length}
              </Badge>
            </div>
            {pendingApprovals.length === 0 ? (
              <p className="mt-2 text-xs text-muted">{t("agents.noApprovals")}</p>
            ) : (
              <div className="mt-2 grid gap-1.5">
                {pendingApprovals.map((approval) => (
                  <div key={approval.id} className="rounded-sm border border-line px-2.5 py-2">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span className="font-mono text-[10.5px] text-ink">
                        {approval.toolName}
                      </span>
                      <Badge tone={riskTone(approval.risk)}>
                        {t(`agents.risk.${approval.risk}`, { defaultValue: approval.risk })}
                      </Badge>
                      <span className="ml-auto text-[9px] text-faint">
                        <Timestamp value={approval.createdAt} />
                      </span>
                    </div>
                    <p className="mt-0.5 font-mono text-[9px] text-faint">
                      {t("agents.approvalFor")} {approval.sessionId}
                    </p>
                    <JsonView value={approval.arguments} maxHeight="90px" className="mt-1" />
                    <div className="mt-1.5 flex gap-1.5">
                      <Button
                        size="sm"
                        variant="primary"
                        loading={decideApproval.isPending && decideApproval.variables?.id === approval.id}
                        onClick={() =>
                          decideApproval.mutate({ id: approval.id, decision: "approved" })
                        }
                      >
                        {t("common.approve")}
                      </Button>
                      <Button
                        size="sm"
                        variant="secondary"
                        onClick={() =>
                          decideApproval.mutate({ id: approval.id, decision: "rejected" })
                        }
                      >
                        {t("common.reject")}
                      </Button>
                    </div>
                  </div>
                ))}
              </div>
            )}
            <p className="mt-2 border-t border-line pt-2 text-[10px] text-faint">
              {t("agents.workflowApprovalBoundary")}
            </p>
          </Panel>

          <Panel className="p-4">
            <div className="flex items-center gap-2">
              <Boxes className="h-4 w-4 text-accent" />
              <h2 className="text-sm font-semibold text-ink">{t("agents.runtimeTitle")}</h2>
            </div>
            <dl className="mt-2 grid gap-1.5 text-[10.5px]">
              <RuntimeRow label={t("agents.schema")} value={runtime.data?.schemaVersion ?? "—"} />
              <RuntimeRow
                label={t("agents.sessionSchema")}
                value={runtime.data?.sessionSchemaVersion ?? "—"}
              />
              <RuntimeRow
                label={t("agents.approvalTtl")}
                value={`${runtime.data?.approvalTtlSeconds ?? "—"}s`}
              />
              <RuntimeRow
                label={t("agents.limitSteps")}
                value={String(runtime.data?.defaultLimits.maxSteps ?? "—")}
              />
              <RuntimeRow
                label={t("agents.limitTools")}
                value={String(runtime.data?.defaultLimits.maxToolCalls ?? "—")}
              />
              <RuntimeRow
                label={t("agents.limitTokens")}
                value={String(runtime.data?.defaultLimits.maxTokens ?? "—")}
              />
              <RuntimeRow
                label={t("agents.limitCost")}
                value={`$${runtime.data?.defaultLimits.maxCostUsd ?? "—"}`}
              />
            </dl>
            <div className="mt-2 flex flex-wrap gap-1">
              {(runtime.data?.planners ?? []).map((planner) => (
                <Badge key={planner} tone="accent">
                  {planner}
                </Badge>
              ))}
            </div>
            <p className="mt-2 border-t border-line pt-2 text-[10px] text-faint">
              {t("agents.replayBoundary")}
            </p>
          </Panel>

          <Panel className="p-4">
            <div className="flex items-center gap-2">
              <Network className="h-4 w-4 text-accent" />
              <h2 className="text-sm font-semibold text-ink">{t("agents.toolsTitle")}</h2>
            </div>
            <div className="mt-2 grid gap-2">
              <ToolGroup
                icon={<Braces className="h-3.5 w-3.5" />}
                title={t("agents.toolGroups.builtIn")}
                entries={tools.data?.builtIn ?? []}
              />
              <ToolGroup
                icon={<Puzzle className="h-3.5 w-3.5" />}
                title={t("agents.toolGroups.skills")}
                entries={tools.data?.skills ?? []}
              />
              <ToolGroup
                icon={<Workflow className="h-3.5 w-3.5" />}
                title={t("agents.toolGroups.workflows")}
                entries={tools.data?.workflows ?? []}
              />
              <ToolGroup
                icon={<Network className="h-3.5 w-3.5" />}
                title={t("agents.toolGroups.mcp")}
                entries={tools.data?.mcpServers ?? []}
              />
              <ToolGroup
                icon={<Braces className="h-3.5 w-3.5" />}
                title={t("agents.toolGroups.connectors")}
                entries={tools.data?.connectors ?? []}
              />
            </div>
          </Panel>
        </div>
      </div>
    </div>
  );
}

function SessionPanel({
  session,
  replay,
  onReplay,
  onCancel,
  onContinue,
  replayBusy,
  cancelBusy,
  continueBusy,
}: {
  session: AgentSessionDetail;
  replay: JsonValue | null;
  onReplay: () => void;
  onCancel: () => void;
  onContinue: () => void;
  replayBusy: boolean;
  cancelBusy: boolean;
  continueBusy: boolean;
}) {
  const { t } = useTranslation();
  const replayResult = replay as {
    matches?: boolean;
    deterministic?: boolean;
    replayOutput?: JsonValue;
    reason?: string;
    actions?: number;
  } | null;
  const canCancel = ["running", "waiting_approval", "interrupted"].includes(session.status);
  return (
    <Panel className="p-4">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={statusTone(session.status)} dot>
          {t(`status.${session.status}`, { defaultValue: session.status })}
        </Badge>
        <span className="font-mono text-[10px] text-ink">{session.id}</span>
        <span className="font-mono text-[9.5px] text-faint">
          {session.agentId}@{session.agentVersion}
        </span>
        <span className="ml-auto text-[9.5px] text-faint">
          <Timestamp value={session.createdAt} />
        </span>
      </div>
      <div className="mt-2 grid grid-cols-2 gap-1.5 sm:grid-cols-4 xl:grid-cols-7">
        <MiniStat label={t("agents.metric.steps")} value={String(session.steps)} />
        <MiniStat label={t("agents.metric.toolCalls")} value={String(session.toolCalls)} />
        <MiniStat label={t("agents.metric.handoffs")} value={String(session.handoffs)} />
        <MiniStat label={t("agents.metric.tokens")} value={`${session.tokensIn}/${session.tokensOut}`} />
        <MiniStat label={t("agents.metric.cost")} value={`$${session.costUsd.toFixed(4)}`} />
        <MiniStat label={t("agents.metric.depth")} value={String(session.depth)} />
        <MiniStat
          label={t("agents.metric.replay")}
          value={replayResult ? (replayResult.matches ? "match" : "diff") : "—"}
        />
      </div>
      {session.pendingApproval ? (
        <p className="mt-2 rounded-sm border border-warning/30 bg-warning/10 px-2 py-1.5 text-[10.5px] text-warning">
          {t("agents.pendingApproval", { id: session.pendingApproval })}
        </p>
      ) : null}
      {session.error ? (
        <p className="mt-2 rounded-sm border border-danger/30 bg-danger/10 px-2 py-1.5 font-mono text-[10.5px] text-danger">
          {session.error}
        </p>
      ) : null}
      <div className="mt-2 grid gap-2 lg:grid-cols-2">
        <div>
          <p className="text-[10px] font-semibold uppercase tracking-[0.12em] text-faint">
            {t("agents.output")}
          </p>
          <JsonView value={session.output ?? session.lastOutput ?? null} maxHeight="220px" className="mt-1" />
        </div>
        <div>
          <p className="text-[10px] font-semibold uppercase tracking-[0.12em] text-faint">
            {t("agents.events")}
          </p>
          <div className="hpa-scroll mt-1 max-h-[220px] overflow-auto rounded-sm border border-line bg-panel-2">
            {session.events.length === 0 ? (
              <p className="px-2 py-2 text-[10.5px] text-faint">{t("agents.noEvents")}</p>
            ) : (
              session.events.map((event) => (
                <div key={`${event.sessionId}-${event.sequence}`} className="border-b border-line px-2 py-1.5 last:border-b-0">
                  <div className="flex items-center gap-1.5">
                    <span className="font-mono text-[9px] text-faint">
                      {String(event.sequence).padStart(2, "0")}
                    </span>
                    <span className="font-mono text-[10px] text-accent">{event.kind}</span>
                    <span className="ml-auto text-[9px] text-faint">
                      <Timestamp value={event.at} />
                    </span>
                  </div>
                  <JsonView value={event.data} maxHeight="80px" className="mt-1" />
                </div>
              ))
            )}
          </div>
        </div>
      </div>
      {replayResult ? (
        <div
          className={cn(
            "mt-2 rounded-sm border px-2 py-1.5 text-[10.5px]",
            replayResult.matches
              ? "border-success/30 bg-success/10 text-success"
              : "border-warning/30 bg-warning/10 text-warning",
          )}
        >
          <div className="flex flex-wrap items-center gap-1.5">
            <Badge tone={replayResult.matches ? "success" : "warning"}>
              {replayResult.matches ? t("agents.replayMatch") : t("agents.replayMismatch")}
            </Badge>
            <span>{replayResult.deterministic ? t("agents.replayTrace") : t("agents.replayModel")}</span>
            {replayResult.actions !== undefined ? (
              <span className="ml-auto font-mono text-[9px]">
                {t("agents.metric.steps")} {replayResult.actions}
              </span>
            ) : null}
          </div>
          {replayResult.reason ? <p className="mt-1">{replayResult.reason}</p> : null}
          {replayResult.replayOutput !== undefined ? (
            <JsonView value={replayResult.replayOutput} maxHeight="120px" className="mt-1" />
          ) : null}
        </div>
      ) : null}
      <div className="mt-2 flex flex-wrap gap-1.5">
        <Button
          size="sm"
          variant="secondary"
          loading={replayBusy}
          onClick={onReplay}
          disabled={session.status !== "succeeded"}
        >
          <RotateCcw className="h-3 w-3" />
          {t("agents.replay")}
        </Button>
        <Button
          size="sm"
          variant="secondary"
          loading={continueBusy}
          onClick={onContinue}
          disabled={session.status !== "interrupted"}
        >
          <Play className="h-3 w-3" />
          {t("agents.continue")}
        </Button>
        <Button
          size="sm"
          variant="danger"
          loading={cancelBusy}
          onClick={onCancel}
          disabled={!canCancel}
        >
          <Square className="h-3 w-3" />
          {t("agents.cancel")}
        </Button>
      </div>
    </Panel>
  );
}

function ToolGroup({
  icon,
  title,
  entries,
}: {
  icon: ReactNode;
  title: string;
  entries: { tool: string; risk: string; label?: string }[];
}) {
  const { t } = useTranslation();
  return (
    <div>
      <div className="flex items-center gap-1.5 text-[10px] font-semibold text-muted">
        {icon}
        {title}
        <span className="ml-auto font-mono text-[9px] text-faint">{entries.length}</span>
      </div>
      {entries.length === 0 ? (
        <p className="mt-1 text-[10px] text-faint">{t("agents.noTools")}</p>
      ) : (
        <div className="mt-1 flex flex-wrap gap-1">
          {entries.slice(0, 8).map((entry) => (
            <span
              key={entry.tool}
              className="inline-flex max-w-full items-center gap-1 rounded-xs border border-line bg-panel-3 px-1.5 py-0.5 font-mono text-[9px] text-ink"
              title={entry.tool}
            >
              <span className="truncate">{entry.label ?? entry.tool}</span>
              <span
                className={cn(
                  "shrink-0",
                  entry.risk === "high"
                    ? "text-danger"
                    : entry.risk === "medium"
                      ? "text-warning"
                      : "text-faint",
                )}
              >
                {entry.risk}
              </span>
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

function StatCard({ label, value }: { label: string; value: number | string }) {
  return (
    <div className="rounded-sm border border-line bg-panel px-3 py-2">
      <p className="text-[9px] uppercase tracking-[0.14em] text-faint">{label}</p>
      <p className="mt-0.5 font-mono text-sm text-ink">{value}</p>
    </div>
  );
}

function MiniStat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-sm border border-line bg-panel-2 px-2 py-1.5">
      <p className="text-[8.5px] uppercase tracking-[0.12em] text-faint">{label}</p>
      <p className="mt-0.5 truncate font-mono text-[11px] text-ink">{value}</p>
    </div>
  );
}

function RuntimeRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center gap-2 border-b border-line pb-1 last:border-b-0">
      <dt className="text-faint">{label}</dt>
      <dd className="ml-auto truncate font-mono text-[10px] text-ink">{value}</dd>
    </div>
  );
}

function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof ApiError) {
    return error.message || fallback;
  }
  return error instanceof Error ? error.message : fallback;
}
