import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "@tanstack/react-router";
import {
  ArrowLeft,
  ArrowUpRight,
  BadgeCheck,
  Clock3,
  RefreshCw,
  RotateCcw,
  Timer,
  XCircle,
} from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Badge,
  Button,
  EmptyState,
  JsonView,
  PageHeader,
  Panel,
  StatusBadge,
  Timestamp,
  cn,
  inputClass,
} from "../components/ui";
import { api } from "../lib/api";
import { formatDate } from "../lib/i18n";
import type { ExecutionEvent, JsonValue } from "../lib/types";
import { useUiStore } from "../stores/ui";

export function ExecutionDetailPage() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const params = useParams({ strict: false }) as { executionId?: string };
  const executionId = params.executionId ?? "";
  const pushToast = useUiStore((state) => state.pushToast);
  const [comment, setComment] = useState("");

  const execution = useQuery({
    queryKey: ["execution", executionId],
    queryFn: () => api.getExecution(executionId),
    enabled: Boolean(executionId),
    retry: false,
    refetchInterval: (query) => {
      const status = query.state.data?.summary.status;
      return status === "running" || status === "waiting_approval" ? 1_000 : false;
    },
  });

  const events = useQuery({
    queryKey: ["events", executionId],
    queryFn: () => api.listEvents(executionId),
    enabled: Boolean(executionId),
    refetchInterval: execution.data?.summary.status === "running" ? 1_000 : false,
  });

  const decide = useMutation({
    mutationFn: ({
      decision,
      nodeId,
    }: {
      decision: "approved" | "rejected";
      nodeId: string;
    }) => {
      const form: JsonValue = comment.trim() ? { comment: comment.trim() } : {};
      return api.decideApproval(executionId, nodeId, decision, form);
    },
    onSuccess: () => {
      setComment("");
      pushToast(t("approvals.approveSuccess"), "success");
      void queryClient.invalidateQueries({ queryKey: ["execution", executionId] });
      void queryClient.invalidateQueries({ queryKey: ["events", executionId] });
      void queryClient.invalidateQueries({ queryKey: ["approvals"] });
      void queryClient.invalidateQueries({ queryKey: ["executions"] });
    },
    onError: (error) =>
      pushToast(error instanceof Error ? error.message : t("approvals.error"), "error"),
  });

  const retry = useMutation({
    mutationFn: () => api.retryExecution(executionId),
    onSuccess: (detail) => {
      pushToast(t("executions.retryStarted"), "success");
      void queryClient.invalidateQueries({ queryKey: ["executions"] });
      navigate({ to: "/executions/$executionId", params: { executionId: detail.summary.id } });
    },
    onError: (error) =>
      pushToast(error instanceof Error ? error.message : t("errors.unknown"), "error"),
  });

  const cancel = useMutation({
    mutationFn: () => api.cancelExecution(executionId),
    onSuccess: (detail) => {
      pushToast(
        t(
          detail.summary.status === "running"
            ? "executions.cancelRequested"
            : "status.cancelled",
        ),
        "info",
      );
      void queryClient.invalidateQueries({ queryKey: ["execution", executionId] });
      void queryClient.invalidateQueries({ queryKey: ["executions"] });
    },
    onError: (error) =>
      pushToast(error instanceof Error ? error.message : t("errors.unknown"), "error"),
  });

  if (execution.isLoading) {
    return <p className="p-6 text-sm text-muted">{t("common.loading")}</p>;
  }

  if (execution.isError || !execution.data) {
    return (
      <EmptyState
        title={t("executions.notFound")}
        hint={execution.error instanceof Error ? execution.error.message : undefined}
        action={
          <Link to="/executions">
            <Button variant="secondary">{t("common.back")}</Button>
          </Link>
        }
      />
    );
  }

  const detail = execution.data;
  const summary = detail.summary;
  const state = detail.state;
  const waiting = summary.waiting;

  return (
    <div className="mx-auto grid max-w-[1560px] gap-3">
      <PageHeader
        title={t("executions.detailTitle")}
        subtitle={summary.id}
        actions={
          <>
            <Link to="/executions">
              <Button variant="ghost" size="sm">
                <ArrowLeft className="h-3.5 w-3.5" />
                {t("common.back")}
              </Button>
            </Link>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => retry.mutate()}
              loading={retry.isPending}
              disabled={!["succeeded", "failed", "cancelled"].includes(summary.status)}
            >
              <RotateCcw className="h-3.5 w-3.5" />
              {t("executions.retry")}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => {
                void execution.refetch();
                void events.refetch();
              }}
            >
              <RefreshCw className="h-3.5 w-3.5" />
              {t("common.refresh")}
            </Button>
            {["running", "waiting_approval"].includes(summary.status) ? (
              <Button
                variant="danger"
                size="sm"
                onClick={() => cancel.mutate()}
                loading={cancel.isPending}
              >
                <XCircle className="h-3.5 w-3.5" />
                {t("executions.cancel")}
              </Button>
            ) : null}
            <Link to="/designer" search={{ id: summary.workflowId }}>
              <Button variant="primary" size="sm">
                {t("workflows.openDesigner")}
                <ArrowUpRight className="h-3.5 w-3.5" />
              </Button>
            </Link>
          </>
        }
      />

      <Panel className="p-3">
        <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
          <StatusBadge status={summary.status} />
          <div>
            <p className="text-[0.6rem] uppercase tracking-[0.14em] text-faint">{t("common.workflow")}</p>
            <p className="mt-0.5 font-mono text-xs text-ink">{summary.workflowId}</p>
          </div>
          <div>
            <p className="text-[0.6rem] uppercase tracking-[0.14em] text-faint">{t("common.version")}</p>
            <p className="mt-0.5 font-mono text-xs text-ink">v{summary.workflowVersion}</p>
          </div>
          <div>
            <p className="text-[0.6rem] uppercase tracking-[0.14em] text-faint">{t("common.startedAt")}</p>
            <p className="mt-0.5 text-xs text-ink">{formatDate(summary.startedAt)}</p>
          </div>
          <div>
            <p className="text-[0.6rem] uppercase tracking-[0.14em] text-faint">{t("common.steps")}</p>
            <p className="mt-0.5 font-mono text-xs text-ink">{state.stepCount}</p>
          </div>
          <div className="ml-auto flex items-center gap-1.5 text-[0.68rem] text-faint">
            <Clock3 className="h-3.5 w-3.5" />
            <Timestamp value={state.updatedAt} />
          </div>
        </div>
        {summary.error ? (
          <div className="mt-4 rounded-sm border border-danger/30 bg-danger/10 px-3 py-2">
            <p className="font-mono text-[0.68rem] text-danger">{summary.error.code}</p>
            <p className="mt-1 text-xs text-danger">{summary.error.message}</p>
          </div>
        ) : null}
      </Panel>

      <StepTimeline events={events.data ?? []} />

      {waiting ? (
        <Panel className="border-warning/40 bg-warning/5 p-3">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <div className="flex items-center gap-2">
                <Badge tone="warning" dot>
                  {t("executions.waiting")}
                </Badge>
                <span className="font-mono text-[0.64rem] text-muted">{waiting.nodeId}</span>
              </div>
              <h2 className="mt-2 text-lg font-semibold text-ink-strong">{waiting.title}</h2>
              <p className="mt-1 text-xs text-muted">
                {t("approvals.requested")}: {formatDate(waiting.requestedAt)}
              </p>
            </div>
            <div className="grid min-w-[260px] gap-2">
              <input
                className={inputClass}
                placeholder={t("approvals.commentPlaceholder")}
                value={comment}
                onChange={(event) => setComment(event.target.value)}
              />
              <div className="flex justify-end gap-2">
                <Button
                  variant="primary"
                  size="sm"
                  onClick={() =>
                    decide.mutate({ decision: "approved", nodeId: waiting.nodeId })
                  }
                  loading={decide.isPending}
                >
                  <BadgeCheck className="h-3.5 w-3.5" />
                  {t("common.approve")}
                </Button>
                <Button
                  variant="danger"
                  size="sm"
                  onClick={() =>
                    decide.mutate({ decision: "rejected", nodeId: waiting.nodeId })
                  }
                  loading={decide.isPending}
                >
                  {t("common.reject")}
                </Button>
              </div>
            </div>
          </div>
        </Panel>
      ) : null}

      <div className="grid gap-3 xl:grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)]">
        <Panel>
          <div className="flex items-center justify-between border-b border-line px-3 py-2">
            <h2 className="text-sm font-semibold text-ink">{t("executions.events")}</h2>
            <span className="font-mono text-[0.62rem] text-faint">{events.data?.length ?? 0}</span>
          </div>
          <ol className="hpa-scroll max-h-[560px] overflow-y-auto p-3">
            {(events.data ?? []).map((event) => (
              <li key={event.id} className="relative grid grid-cols-[18px_minmax(0,1fr)] gap-2 pb-3">
                <span className="relative mt-1 grid place-items-center">
                  <span
                    className={cn(
                      "h-2 w-2 rounded-full",
                      event.kind.includes("failed")
                        ? "bg-danger"
                        : event.kind.includes("succeeded")
                          ? "bg-success"
                          : event.kind.includes("waiting") || event.kind.includes("approval")
                            ? "bg-warning"
                            : "bg-info",
                    )}
                  />
                  <span className="absolute top-2 h-full w-px bg-line" />
                </span>
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-xs text-ink">
                      {t(`event.${event.kind}`, { defaultValue: event.kind })}
                    </span>
                    {event.nodeId ? (
                      <span className="font-mono text-[0.6rem] text-muted">{event.nodeId}</span>
                    ) : null}
                    <span className="ml-auto font-mono text-[0.58rem] text-faint">
                      {formatDate(event.at)}
                    </span>
                  </div>
                  <details className="mt-1">
                    <summary className="cursor-pointer font-mono text-[0.58rem] text-faint hover:text-muted">
                      #{event.sequence}
                    </summary>
                    <div className="mt-1">
                      <JsonView value={event.data} maxHeight="140px" />
                    </div>
                  </details>
                </div>
              </li>
            ))}
          </ol>
          {events.data?.length === 0 ? (
            <p className="px-4 pb-4 text-xs text-faint">{t("common.noData")}</p>
          ) : null}
        </Panel>

        <div className="grid content-start gap-4">
          <Panel className="p-3">
            <h2 className="text-[0.66rem] font-semibold uppercase tracking-[0.14em] text-faint">
              {t("common.input")}
            </h2>
            <div className="mt-2">
              <JsonView value={state.input} maxHeight="180px" />
            </div>
          </Panel>
          <Panel className="p-3">
            <h2 className="text-[0.66rem] font-semibold uppercase tracking-[0.14em] text-faint">
              {t("executions.nodeOutputs")}
            </h2>
            <div className="mt-2">
              <JsonView value={state.nodeOutputs} maxHeight="220px" />
            </div>
          </Panel>
          <Panel className="p-3">
            <h2 className="text-[0.66rem] font-semibold uppercase tracking-[0.14em] text-faint">
              {t("common.output")}
            </h2>
            <div className="mt-2">
              <JsonView value={state.output ?? null} maxHeight="180px" />
            </div>
          </Panel>
        </div>
      </div>
    </div>
  );
}

interface StepSpan {
  key: string;
  nodeId: string;
  graphId?: string | null;
  attempt?: number | null;
  startedAt: number;
  endedAt: number;
  status: "succeeded" | "failed" | "waiting" | "skipped" | "running";
}

function buildSpans(events: ExecutionEvent[]): StepSpan[] {
  const open = new Map<string, { startedAt: number; attempt?: number | null; graphId?: string | null }>();
  const spans: StepSpan[] = [];
  const ordered = [...events].sort((left, right) => left.sequence - right.sequence);
  for (const event of ordered) {
    const nodeId = event.nodeId;
    if (!nodeId) continue;
    const at = Date.parse(event.at);
    if (Number.isNaN(at)) continue;
    if (event.kind === "node.started") {
      open.set(nodeId, { startedAt: at, attempt: event.attempt, graphId: event.graphId });
      continue;
    }
    const status =
      event.kind === "node.succeeded"
        ? "succeeded"
        : event.kind === "node.failed"
          ? "failed"
          : event.kind === "node.waiting"
            ? "waiting"
            : event.kind === "node.skipped"
              ? "skipped"
              : null;
    if (!status) continue;
    const started = open.get(nodeId);
    open.delete(nodeId);
    spans.push({
      key: `${nodeId}-${event.sequence}`,
      nodeId,
      graphId: event.graphId ?? started?.graphId ?? null,
      attempt: event.attempt ?? started?.attempt ?? null,
      startedAt: started?.startedAt ?? at,
      endedAt: at,
      status,
    });
  }
  for (const [nodeId, started] of open) {
    spans.push({
      key: `${nodeId}-open`,
      nodeId,
      graphId: started.graphId ?? null,
      attempt: started.attempt ?? null,
      startedAt: started.startedAt,
      endedAt: Date.now(),
      status: "running",
    });
  }
  return spans.sort((left, right) => left.startedAt - right.startedAt);
}

function formatDuration(ms: number): string {
  if (ms < 1) return "<1ms";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)}s`;
  const minutes = Math.floor(ms / 60_000);
  return `${minutes}m ${Math.round((ms % 60_000) / 1000)}s`;
}

const spanTone: Record<StepSpan["status"], string> = {
  succeeded: "bg-accent",
  failed: "bg-danger",
  waiting: "bg-warning",
  skipped: "bg-line-strong",
  running: "bg-accent/60",
};

function StepTimeline({ events }: { events: ExecutionEvent[] }) {
  const { t } = useTranslation();
  const spans = buildSpans(events);
  if (spans.length === 0) {
    return null;
  }
  const start = Math.min(...spans.map((span) => span.startedAt));
  const end = Math.max(...spans.map((span) => span.endedAt));
  const total = Math.max(end - start, 1);
  const slowest = spans.reduce((worst, span) =>
    span.endedAt - span.startedAt > worst.endedAt - worst.startedAt ? span : worst,
  );

  return (
    <Panel className="p-3">
      <div className="flex flex-wrap items-center gap-2">
        <Timer className="h-4 w-4 text-accent" />
        <h2 className="text-sm font-semibold text-ink">{t("executions.timeline.title")}</h2>
        <Badge tone="neutral">{t("executions.timeline.spans", { total: spans.length })}</Badge>
        <Badge tone="neutral">
          {t("executions.timeline.total")}: {formatDuration(total)}
        </Badge>
        <Badge tone="warning">
          {t("executions.timeline.slowest")}: {slowest.nodeId} ·{" "}
          {formatDuration(slowest.endedAt - slowest.startedAt)}
        </Badge>
      </div>
      <p className="mt-1 text-[10.5px] text-faint">{t("executions.timeline.hint")}</p>
      <ol className="mt-2 grid gap-1">
        {spans.map((span) => {
          const offset = ((span.startedAt - start) / total) * 100;
          const width = Math.max(((span.endedAt - span.startedAt) / total) * 100, 0.8);
          const isSlowest = span.key === slowest.key;
          return (
            <li
              key={span.key}
              className="grid min-w-0 grid-cols-[minmax(0,148px)_minmax(0,1fr)_58px] items-center gap-2"
            >
              <div className="min-w-0">
                <p
                  className={cn(
                    "truncate font-mono text-[11px]",
                    isSlowest ? "text-warning" : "text-ink",
                  )}
                  title={span.graphId ? `${span.nodeId} (${span.graphId})` : span.nodeId}
                >
                  {span.nodeId}
                </p>
                {span.attempt && span.attempt > 1 ? (
                  <p className="text-[9.5px] text-faint">#{span.attempt}</p>
                ) : null}
              </div>
              <div className="relative h-2.5 min-w-0 rounded-sm bg-panel-3">
                <div
                  className={cn("absolute inset-y-0 rounded-sm", spanTone[span.status])}
                  style={{ left: `${offset}%`, width: `${width}%` }}
                  title={`${span.status} · ${formatDuration(span.endedAt - span.startedAt)}`}
                />
              </div>
              <span className="text-right font-mono text-[10px] tabular-nums text-faint">
                {formatDuration(span.endedAt - span.startedAt)}
              </span>
            </li>
          );
        })}
      </ol>
    </Panel>
  );
}
