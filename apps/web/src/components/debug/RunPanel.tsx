import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Play, RefreshCw } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, ApiError } from "../../lib/api";
import { defaultInputFor } from "../../lib/ir";
import { formatDate } from "../../lib/i18n";
import type { JsonValue } from "../../lib/types";
import { useEditorStore } from "../../stores/editor";
import { useUiStore } from "../../stores/ui";
import {
  Badge,
  Button,
  EmptyState,
  Field,
  JsonView,
  StatusBadge,
  ValidationList,
  cn,
  inputClass,
  textareaClass,
} from "../ui";

export function RunPanel() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const ir = useEditorStore((state) => state.ir);
  const workflowId = useEditorStore((state) => state.workflowId);
  const dirty = useEditorStore((state) => state.dirty);
  const issues = useEditorStore((state) => state.issues);
  const executionId = useEditorStore((state) => state.runningExecutionId);
  const setRunningExecution = useEditorStore((state) => state.setRunningExecution);
  const markSaved = useEditorStore((state) => state.markSaved);
  const pushToast = useUiStore((state) => state.pushToast);
  const [inputText, setInputText] = useState("{}");
  const [comment, setComment] = useState("");
  const [starting, setStarting] = useState(false);

  useEffect(() => {
    if (ir) {
      setInputText(JSON.stringify(defaultInputFor(ir), null, 2));
    }
  }, [ir?.id]);

  const execution = useQuery({
    queryKey: ["execution", executionId],
    queryFn: () => api.getExecution(executionId ?? ""),
    enabled: Boolean(executionId),
    refetchInterval: (query) => {
      const status = query.state.data?.summary.status;
      return status === "running" || status === "waiting_approval" ? 900 : false;
    },
  });

  const events = useQuery({
    queryKey: ["events", executionId],
    queryFn: () => api.listEvents(executionId ?? ""),
    enabled: Boolean(executionId),
    refetchInterval: () => {
      const status = execution.data?.summary.status;
      return status === "running" || status === "waiting_approval" ? 900 : false;
    },
  });

  const decide = useMutation({
    mutationFn: async ({
      decision,
      nodeId,
    }: {
      decision: "approved" | "rejected";
      nodeId: string;
    }) => {
      if (!executionId) {
        throw new Error("missing execution");
      }
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
    onError: (error) => {
      pushToast(error instanceof Error ? error.message : t("errors.unknown"), "error");
    },
  });

  const status = execution.data?.summary.status;
  const waiting = execution.data?.summary.waiting;
  const running = starting || status === "running";
  const eventList = useMemo(() => (events.data ?? []).slice(-16).reverse(), [events.data]);

  async function handleRun() {
    if (!ir) {
      return;
    }
    let input: JsonValue;
    try {
      input = JSON.parse(inputText) as JsonValue;
    } catch {
      pushToast(t("designer.invalidInput"), "error");
      return;
    }
    setStarting(true);
    try {
      let id = workflowId;
      if (!id || dirty) {
        const saved = await api.saveWorkflow(ir);
        markSaved(saved.id);
        id = saved.id;
        await queryClient.invalidateQueries({ queryKey: ["workflows"] });
      }
      const started = await api.startExecution(id, input);
      setRunningExecution(started.summary.id);
      pushToast(t("designer.runStarted"), "success");
      await queryClient.invalidateQueries({ queryKey: ["executions"] });
    } catch (error) {
      const message =
        error instanceof ApiError && error.status === 422
          ? t("designer.validationFailed")
          : error instanceof Error
            ? error.message
            : t("errors.unknown");
      pushToast(message, "error");
    } finally {
      setStarting(false);
    }
  }

  return (
    <div className="grid min-h-0 gap-4 p-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.15fr)]">
      {issues.length > 0 ? (
        <div className="rounded-md border border-line bg-panel p-3 lg:col-span-2">
          <div className="mb-2 flex items-center gap-2">
            <span className="text-xs font-semibold text-ink">
              {t("validation.title")}
            </span>
            <Badge
              tone={issues.some((issue) => issue.severity === "error") ? "danger" : "warning"}
            >
              {issues.filter((issue) => issue.severity === "error").length > 0
                ? `${issues.filter((issue) => issue.severity === "error").length} ${t("validation.errors")}`
                : `${issues.length} ${t("validation.warnings")}`}
            </Badge>
          </div>
          <ValidationList report={{ issues }} />
        </div>
      ) : null}
      <div className="grid content-start gap-3">
        <Field label={t("designer.inputLabel")}>
          <textarea
            className={cn(textareaClass, "min-h-[150px]")}
            value={inputText}
            onChange={(event) => setInputText(event.target.value)}
            spellCheck={false}
          />
        </Field>
        <div className="flex flex-wrap items-center gap-2">
          <Button variant="primary" onClick={() => void handleRun()} loading={starting || running}>
            <Play className="h-3.5 w-3.5" />
            {t("designer.runButton")}
          </Button>
          {executionId ? (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                void execution.refetch();
                void events.refetch();
              }}
            >
              <RefreshCw className="h-3 w-3" />
              {t("common.refresh")}
            </Button>
          ) : null}
          {executionId && status ? <StatusBadge status={status} /> : null}
          {executionId ? (
            <Link
              to="/executions/$executionId"
              params={{ executionId }}
              className="ml-auto text-xs text-accent underline-offset-4 hover:underline"
            >
              {t("designer.openExecution")}
            </Link>
          ) : null}
        </div>
        {execution.data ? (
          <div className="grid gap-2 rounded-md border border-line bg-panel p-3 text-xs">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-mono text-[0.66rem] text-muted">{execution.data.summary.id}</span>
              <span className="ml-auto font-mono text-[0.64rem] text-faint">
                {t("executions.stepsUsed", { count: execution.data.state.stepCount })}
              </span>
            </div>
            {execution.data.summary.error ? (
              <div className="rounded-sm border border-danger/30 bg-danger/10 p-2 text-danger">
                <p className="font-mono text-[0.66rem]">{execution.data.summary.error.code}</p>
                <p className="mt-1 text-[0.7rem]">{execution.data.summary.error.message}</p>
              </div>
            ) : null}
            {waiting ? (
              <div className="rounded-sm border border-warning/40 bg-warning/10 p-3">
                <div className="flex items-center justify-between gap-2">
                  <p className="text-[0.72rem] font-medium text-warning">{waiting.title}</p>
                  <Badge tone="warning">{t("executions.waiting")}</Badge>
                </div>
                <p className="mt-1 font-mono text-[0.62rem] text-muted">{waiting.nodeId}</p>
                <input
                  className={cn(inputClass, "mt-2 h-8 bg-panel text-xs")}
                  placeholder={t("approvals.commentPlaceholder")}
                  value={comment}
                  onChange={(event) => setComment(event.target.value)}
                />
                <div className="mt-2 flex gap-2">
                  <Button
                    size="sm"
                    variant="primary"
                    onClick={() => decide.mutate({ decision: "approved", nodeId: waiting.nodeId })}
                    loading={decide.isPending}
                  >
                    {t("common.approve")}
                  </Button>
                  <Button
                    size="sm"
                    variant="danger"
                    onClick={() => decide.mutate({ decision: "rejected", nodeId: waiting.nodeId })}
                    loading={decide.isPending}
                  >
                    {t("common.reject")}
                  </Button>
                </div>
              </div>
            ) : null}
          </div>
        ) : (
          <EmptyState title={t("designer.noSelection")} hint={t("designer.runHint")} />
        )}
      </div>

      <div className="grid min-h-0 content-start gap-3">
        <div className="rounded-md border border-line bg-panel">
          <div className="flex items-center justify-between border-b border-line px-3 py-2">
            <h3 className="text-[0.66rem] font-semibold uppercase tracking-[0.14em] text-faint">
              {t("designer.events")}
            </h3>
            <span className="font-mono text-[0.62rem] text-faint">{events.data?.length ?? 0}</span>
          </div>
          <div className="hpa-scroll max-h-[220px] overflow-y-auto p-2">
            {eventList.length === 0 ? (
              <p className="px-2 py-4 text-xs text-faint">{t("common.noData")}</p>
            ) : (
              <ul className="grid gap-1">
                {eventList.map((event) => (
                  <li
                    key={event.id}
                    className="grid grid-cols-[54px_minmax(0,1fr)_auto] items-center gap-2 rounded-sm px-2 py-1 hover:bg-panel-2"
                  >
                    <span className="font-mono text-[0.6rem] text-faint">{event.sequence}</span>
                    <span className="min-w-0 truncate text-[0.7rem] text-ink">
                      {t(`event.${event.kind}`, { defaultValue: event.kind })}
                      {event.nodeId ? (
                        <span className="ml-2 font-mono text-[0.6rem] text-muted">
                          {event.nodeId}
                        </span>
                      ) : null}
                    </span>
                    <span className="font-mono text-[0.58rem] text-faint">
                      {formatDate(event.at)}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>

        <div className="grid gap-2">
          <div className="flex items-center justify-between">
            <h3 className="text-[0.66rem] font-semibold uppercase tracking-[0.14em] text-faint">
              {t("designer.nodeOutputs")}
            </h3>
          </div>
          <JsonView
            value={execution.data?.state.nodeOutputs ?? {}}
            maxHeight="220px"
          />
        </div>
        {execution.data?.summary.output !== undefined ? (
          <div className="grid gap-2">
            <h3 className="text-[0.66rem] font-semibold uppercase tracking-[0.14em] text-faint">
              {t("common.output")}
            </h3>
            <JsonView value={execution.data.summary.output} maxHeight="180px" />
          </div>
        ) : null}
      </div>
    </div>
  );
}
