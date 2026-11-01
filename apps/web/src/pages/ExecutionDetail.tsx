import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import {
  ArrowLeft,
  ArrowUpRight,
  BadgeCheck,
  Clock3,
  RefreshCw,
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
import type { JsonValue } from "../lib/types";
import { useUiStore } from "../stores/ui";

export function ExecutionDetailPage() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
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

  const cancel = useMutation({
    mutationFn: () => api.cancelExecution(executionId),
    onSuccess: () => {
      pushToast(t("status.cancelled"), "info");
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
              onClick={() => {
                void execution.refetch();
                void events.refetch();
              }}
            >
              <RefreshCw className="h-3.5 w-3.5" />
              {t("common.refresh")}
            </Button>
            {summary.status === "waiting_approval" ? (
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
