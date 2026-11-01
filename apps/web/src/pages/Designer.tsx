import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useRouterState } from "@tanstack/react-router";
import {
  ArrowLeft,
  BadgeCheck,
  Braces,
  CirclePlay,
  CircleStop,
  FileJson,
  GitBranch,
  Globe,
  Layers,
  Merge,
  Play,
  Repeat,
  Save,
  ShieldCheck,
  Split,
  Terminal,
  UploadCloud,
  Variable,
  Workflow,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { WorkflowCanvas } from "../components/canvas/WorkflowCanvas";
import { CodePanel } from "../components/code/CodePanel";
import { NodeInspector } from "../components/inspector/NodeInspector";
import { Badge, Button, EmptyState, cn } from "../components/ui";
import { api, ApiError } from "../lib/api";
import {
  createNode,
  defaultInputFor,
  emptyWorkflow,
  nextNodePosition,
  nodeKey,
  nodePorts,
  normalizedWorkflow,
  type NodeType,
} from "../lib/ir";
import type { JsonValue, ValidationReport, WorkflowIR } from "../lib/types";
import { useEditorStore } from "../stores/editor";
import { useUiStore } from "../stores/ui";
import styles from "./Designer.module.css";

const icons: Record<string, LucideIcon> = {
  start: CirclePlay,
  end: CircleStop,
  transform: Braces,
  condition: GitBranch,
  switch: Split,
  loop: Repeat,
  parallel: Layers,
  join: Merge,
  "http.request": Globe,
  script: Terminal,
  setVariable: Variable,
  "human.approval": BadgeCheck,
  subflow: Workflow,
};

const paletteGroups: { key: string; types: NodeType[] }[] = [
  { key: "flow", types: ["start", "end"] },
  { key: "control", types: ["condition", "switch", "loop", "parallel", "join"] },
  { key: "data", types: ["transform", "setVariable", "script"] },
  { key: "integration", types: ["http.request"] },
  { key: "human", types: ["human.approval"] },
  { key: "composition", types: ["subflow"] },
];

export function DesignerPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const searchStr = useRouterState({ select: (state) => state.location.searchStr });
  const workflowIdParam = useMemo(() => {
    const params = new URLSearchParams(searchStr);
    return params.get("id") ?? undefined;
  }, [searchStr]);

  const ir = useEditorStore((state) => state.ir);
  const workflowId = useEditorStore((state) => state.workflowId);
  const selectedNodeId = useEditorStore((state) => state.selectedNodeId);
  const dirty = useEditorStore((state) => state.dirty);
  const issues = useEditorStore((state) => state.issues);
  const load = useEditorStore((state) => state.load);
  const replaceIr = useEditorStore((state) => state.replaceIr);
  const patchWorkflow = useEditorStore((state) => state.patchWorkflow);
  const selectNode = useEditorStore((state) => state.selectNode);
  const addNode = useEditorStore((state) => state.addNode);
  const removeNode = useEditorStore((state) => state.removeNode);
  const moveNode = useEditorStore((state) => state.moveNode);
  const addEdge = useEditorStore((state) => state.addEdge);
  const removeEdge = useEditorStore((state) => state.removeEdge);
  const markSaved = useEditorStore((state) => state.markSaved);
  const setIssues = useEditorStore((state) => state.setIssues);
  const setPanel = useEditorStore((state) => state.setPanel);
  const setRunningExecution = useEditorStore((state) => state.setRunningExecution);
  const pushToast = useUiStore((state) => state.pushToast);
  const [busy, setBusy] = useState<"save" | "publish" | "validate" | "run" | null>(null);

  const workflowQuery = useQuery({
    queryKey: ["workflow", workflowIdParam],
    queryFn: () => api.getWorkflow(workflowIdParam ?? ""),
    enabled: Boolean(workflowIdParam),
    retry: false,
  });

  const workflows = useQuery({
    queryKey: ["workflows"],
    queryFn: api.listWorkflows,
    retry: 1,
  });

  useEffect(() => {
    if (workflowIdParam && workflowQuery.data && workflowId !== workflowIdParam) {
      load(workflowQuery.data.ir);
    }
  }, [workflowIdParam, workflowQuery.data, workflowId, load]);

  useEffect(() => {
    if (!workflowIdParam && !useEditorStore.getState().ir) {
      replaceIr(emptyWorkflow(t("workflows.untitled")));
    }
  }, [workflowIdParam, replaceIr, t]);

  const selectedNode = ir?.nodes.find((node) => node.id === selectedNodeId) ?? null;
  const published =
    workflows.data?.find((workflow) => workflow.id === ir?.id)?.published ??
    workflowQuery.data?.summary.published ??
    false;

  async function saveWorkflow(): Promise<WorkflowIR | null> {
    if (!ir) {
      return null;
    }
    setBusy("save");
    try {
      const saved = await api.saveWorkflow(normalizedWorkflow(ir));
      markSaved(saved.id);
      pushToast(t("designer.savedDraft"), "success");
      await queryClient.invalidateQueries({ queryKey: ["workflows"] });
      if (!workflowIdParam) {
        void navigate({
          to: "/designer",
          search: { id: saved.id },
          replace: true,
        });
      }
      return { ...ir, id: saved.id };
    } catch (error) {
      handleApiFailure(error, t("errors.saveFailed"));
      return null;
    } finally {
      setBusy(null);
    }
  }

  async function validateWorkflow() {
    if (!ir) {
      return;
    }
    setBusy("validate");
    try {
      const result = await api.validate(
        "json",
        JSON.stringify(normalizedWorkflow(ir), null, 2),
      );
      setIssues(result.report.issues);
      const hasErrors = result.report.issues.some(
        (issue) => issue.severity === "error",
      );
      if (hasErrors) {
        setPanel("debug");
      }
      pushToast(
        hasErrors
          ? t("designer.validationFailed")
          : t("designer.validationPassed"),
        hasErrors ? "error" : "success",
      );
    } catch (error) {
      if (error instanceof ApiError && error.status === 422 && isReport(error.details)) {
        setIssues(error.details.issues);
        setPanel("debug");
        pushToast(t("designer.validationFailed"), "error");
      } else {
        handleApiFailure(error, t("errors.validateFailed"));
      }
    } finally {
      setBusy(null);
    }
  }

  async function publishWorkflow() {
    if (!ir) {
      return;
    }
    setBusy("publish");
    try {
      const current = dirty || !workflowIdParam ? await saveWorkflow() : ir;
      if (!current) {
        return;
      }
      await api.publishWorkflow(current.id);
      pushToast(t("designer.publishedDone"), "success");
      await queryClient.invalidateQueries({ queryKey: ["workflows"] });
      await queryClient.invalidateQueries({ queryKey: ["workflow", current.id] });
    } catch (error) {
      handleApiFailure(error, t("errors.saveFailed"));
    } finally {
      setBusy(null);
    }
  }

  async function runWorkflow() {
    if (!ir) {
      return;
    }
    setBusy("run");
    try {
      const current = dirty || !workflowIdParam ? await saveWorkflow() : ir;
      if (!current) {
        return;
      }
      const started = await api.startExecution(current.id, defaultInputFor(current) as JsonValue);
      setRunningExecution(started.summary.id);
      setPanel("debug");
      pushToast(t("designer.runStarted"), "success");
      await queryClient.invalidateQueries({ queryKey: ["executions"] });
    } catch (error) {
      handleApiFailure(error, t("errors.runFailed"));
    } finally {
      setBusy(null);
    }
  }

  function handleAddNode(type: NodeType) {
    if (!ir) {
      return;
    }
    const position = nextNodePosition(ir);
    const seed = createNode(type, ir, position, {
      name: t(`node.${nodeKey(type)}`),
      approvalTitle: t("node.approval"),
    });
    addNode(seed.node, seed.subflow);
    if (type !== "start") {
      const source =
        selectedNode && selectedNode.type !== "end"
          ? selectedNode
          : [...ir.nodes].reverse().find((node) => node.type !== "end");
      if (source && source.id !== seed.node.id) {
        const outgoing = ir.edges
          .filter((edge) => edge.source === source.id)
          .map((edge) => edge.sourcePort ?? "out");
        const ports = nodePorts(source, outgoing);
        const port = ports.find((entry) => entry === "out") ?? ports[0];
        if (port) {
          addEdge({
            id: `e_${source.id}_${seed.node.id}_${Date.now().toString(36)}`,
            source: source.id,
            sourcePort: port,
            target: seed.node.id,
            targetPort: "in",
          });
        }
      }
    }
  }

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey)) {
        return;
      }
      if (event.key.toLowerCase() === "s") {
        event.preventDefault();
        void saveWorkflow();
      }
      if (event.key === "Enter") {
        event.preventDefault();
        void runWorkflow();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  });

  if (workflowIdParam && workflowQuery.isLoading) {
    return <div className={styles.loading}>{t("common.loading")}</div>;
  }

  if (workflowIdParam && workflowQuery.isError) {
    return (
      <div className={styles.emptyWrap}>
        <EmptyState
          title={t("designer.loadFailed")}
          hint={workflowQuery.error instanceof Error ? workflowQuery.error.message : undefined}
          action={
            <Button variant="secondary" onClick={() => void navigate({ to: "/workflows" })}>
              {t("nav.workflows")}
            </Button>
          }
        />
      </div>
    );
  }

  if (!ir) {
    return <div className={styles.loading}>{t("common.loading")}</div>;
  }

  const existingTypes = new Set(ir.nodes.map((node) => node.type));
  const errorCount = issues.filter((issue) => issue.severity === "error").length;

  return (
    <div className={styles.designer}>
      <div className={styles.toolbar}>
        <div className={styles.toolbarLeft}>
          <Link to="/workflows" aria-label={t("common.back")} title={t("common.back")}>
            <Button variant="ghost" size="icon">
              <ArrowLeft className="h-4 w-4" />
            </Button>
          </Link>
          <input
            className={styles.name}
            value={ir.name}
            aria-label={t("common.name")}
            onChange={(event) => patchWorkflow({ name: event.target.value })}
          />
          <span className={styles.workflowMeta}>
            {ir.id} · v{ir.workflowVersion}
          </span>
          <Badge tone={published ? "success" : dirty ? "warning" : "neutral"}>
            {published
              ? t("common.published")
              : dirty
                ? t("designer.unsaved")
                : t("common.draft")}
          </Badge>
          {errorCount > 0 ? <Badge tone="danger">{errorCount}</Badge> : null}
        </div>
        <div className={styles.toolbarActions}>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void validateWorkflow()}
            loading={busy === "validate"}
          >
            <ShieldCheck className="h-3.5 w-3.5" />
            {t("designer.validate")}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => void saveWorkflow()}
            loading={busy === "save"}
          >
            <Save className="h-3.5 w-3.5" />
            {t("designer.save")}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => void publishWorkflow()}
            loading={busy === "publish"}
          >
            <UploadCloud className="h-3.5 w-3.5" />
            {t("designer.publish")}
          </Button>
          <Button
            variant="primary"
            size="sm"
            onClick={() => void runWorkflow()}
            loading={busy === "run"}
          >
            <Play className="h-3.5 w-3.5" />
            {t("designer.saveAndRun")}
          </Button>
        </div>
      </div>

      <div className={styles.workspace}>
        <aside className={styles.palette}>
          <div className={styles.sideHeader}>
            <span>{t("designer.palette")}</span>
          </div>
          <div className={styles.paletteScroll}>
            {paletteGroups.map((group) => (
              <div key={group.key} className={styles.paletteGroup}>
                <div className={styles.paletteGroupLabel}>
                  {t(`designer.groups.${group.key}`)}
                </div>
                <div className={styles.paletteGrid}>
                  {group.types.map((type) => {
                    const Icon = icons[type] ?? FileJson;
                    const disabled =
                      (type === "start" || type === "end") && existingTypes.has(type);
                    return (
                      <button
                        key={type}
                        type="button"
                        className={cn(styles.paletteItem, disabled && "cursor-not-allowed opacity-45")}
                        onClick={() => handleAddNode(type)}
                        disabled={disabled}
                        title={t(`node.desc.${nodeKey(type)}`)}
                      >
                        <span className={styles.paletteIcon}>
                          <Icon className="h-3.5 w-3.5" />
                        </span>
                        <span className={styles.paletteText}>
                          <span className={styles.paletteLabel}>{t(`node.${nodeKey(type)}`)}</span>
                          <span className={styles.paletteDesc}>
                            {t(`node.desc.${nodeKey(type)}`)}
                          </span>
                        </span>
                      </button>
                    );
                  })}
                </div>
              </div>
            ))}
          </div>
        </aside>

        <section className={styles.canvasPane}>
          <div className={styles.canvasHeader}>
            <span>{t("designer.canvas")}</span>
            <span className={styles.canvasHeaderHint}>{t("designer.moveHint")}</span>
          </div>
          <div className={styles.canvasBody}>
            <WorkflowCanvas
              key={ir.id}
              ir={ir}
              selectedId={selectedNodeId}
              onSelect={selectNode}
              onMove={moveNode}
              onConnect={addEdge}
              onDeleteNode={removeNode}
              onDeleteEdge={removeEdge}
            />
          </div>
        </section>

        <aside className={cn(styles.inspector, !selectedNode && styles.inspectorEmpty)}>
          <div className={styles.sideHeader}>
            <span>{t("designer.inspector")}</span>
            {selectedNode ? (
              <span className="font-mono text-[0.6rem] normal-case tracking-normal text-muted">
                {selectedNode.id}
              </span>
            ) : null}
          </div>
          <div className={styles.inspectorBody}>
            <NodeInspector ir={ir} node={selectedNode} />
          </div>
        </aside>
      </div>

      <div className={styles.codePane}>
        <CodePanel />
      </div>
    </div>
  );

  function handleApiFailure(error: unknown, fallback: string) {
    if (error instanceof ApiError && error.status === 422 && isReport(error.details)) {
      setIssues(error.details.issues);
      pushToast(t("designer.validationFailed"), "error");
      setPanel("yaml");
      return;
    }
    pushToast(error instanceof Error ? error.message : fallback, "error");
  }
}

function isReport(value: unknown): value is ValidationReport {
  return Boolean(value && typeof value === "object" && Array.isArray((value as ValidationReport).issues));
}
