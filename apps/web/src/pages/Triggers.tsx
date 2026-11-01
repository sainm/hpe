import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  CalendarClock,
  Copy,
  ExternalLink,
  Play,
  RefreshCw,
  Timer,
  Webhook,
  Zap,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { Badge, Button, EmptyState, PageHeader, Panel, Timestamp } from "../components/ui";
import { api } from "../lib/api";
import type { TriggerFire, TriggerSummary } from "../lib/types";
import { useUiStore } from "../stores/ui";

const kindTone: Record<string, "accent" | "neutral" | "success" | "warning"> = {
  schedule: "accent",
  webhook: "success",
  event: "warning",
  mcp: "neutral",
};

export function TriggersPage() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);

  const triggersQuery = useQuery({
    queryKey: ["triggers"],
    queryFn: () => api.triggers(),
    refetchInterval: 15_000,
  });
  const firesQuery = useQuery({
    queryKey: ["trigger-fires"],
    queryFn: () => api.triggerFires(30),
    refetchInterval: 15_000,
  });

  const toggle = useMutation({
    mutationFn: (input: { workflowId: string; triggerId: string; enabled: boolean }) =>
      input.enabled
        ? api.enableTrigger(input.workflowId, input.triggerId)
        : api.disableTrigger(input.workflowId, input.triggerId),
    onSuccess: (_result, input) => {
      pushToast(
        input.enabled ? t("triggers.enabledToast") : t("triggers.disabledToast"),
        input.enabled ? "success" : "warning",
      );
      refresh();
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  const fire = useMutation({
    mutationFn: (input: { workflowId: string; triggerId: string }) =>
      api.fireTrigger(input.workflowId, input.triggerId),
    onSuccess: (result) => {
      pushToast(t("triggers.firedToast", { execution: result.executionId }), "success");
      refresh();
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  function refresh() {
    void queryClient.invalidateQueries({ queryKey: ["triggers"] });
    void queryClient.invalidateQueries({ queryKey: ["trigger-fires"] });
  }

  const triggers = triggersQuery.data?.triggers ?? [];
  const fires = firesQuery.data?.fires ?? [];
  const schedules = triggers.filter((trigger) => trigger.kind === "schedule").length;
  const hooks = triggers.filter((trigger) => trigger.kind === "webhook").length;

  return (
    <div className="grid min-w-0 gap-3">
      <PageHeader
        title={t("triggers.title")}
        subtitle={t("triggers.subtitle")}
        actions={
          <div className="flex flex-wrap items-center gap-1.5">
            <Badge tone="accent">{t("triggers.countSchedule", { total: schedules })}</Badge>
            <Badge tone="success">{t("triggers.countWebhook", { total: hooks })}</Badge>
            <Button size="sm" variant="ghost" onClick={refresh}>
              <RefreshCw className="h-3 w-3" />
              {t("common.refresh")}
            </Button>
          </div>
        }
      />

      <div className="grid min-w-0 gap-3 xl:grid-cols-[minmax(0,1fr)_340px]">
        <div className="grid min-w-0 gap-2">
          {triggersQuery.isLoading ? (
            <Panel className="p-4">
              <p className="text-xs text-muted">{t("common.loading")}</p>
            </Panel>
          ) : triggersQuery.isError ? (
            <Panel className="p-4">
              <p className="text-xs text-danger">{messageOf(triggersQuery.error)}</p>
            </Panel>
          ) : triggers.length === 0 ? (
            <Panel className="p-6">
              <EmptyState
                icon={<CalendarClock className="h-4 w-4" />}
                title={t("triggers.empty")}
                hint={t("triggers.emptyHint")}
              />
            </Panel>
          ) : (
            triggers.map((trigger) => (
              <TriggerCard
                key={`${trigger.workflowId}/${trigger.triggerId}`}
                trigger={trigger}
                busy={toggle.isPending || fire.isPending}
                onToggle={() =>
                  toggle.mutate({
                    workflowId: trigger.workflowId,
                    triggerId: trigger.triggerId,
                    enabled: !trigger.enabled,
                  })
                }
                onFire={() =>
                  fire.mutate({ workflowId: trigger.workflowId, triggerId: trigger.triggerId })
                }
              />
            ))
          )}
        </div>

        <Panel className="min-w-0 p-4">
          <div className="flex flex-wrap items-center gap-2">
            <Zap className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("triggers.firesTitle")}</h2>
            <Badge tone="neutral" className="ml-auto">
              {fires.length}
            </Badge>
          </div>
          <p className="mt-1 text-[10.5px] text-faint">{t("triggers.firesHint")}</p>
          {fires.length === 0 ? (
            <p className="mt-3 text-[11.5px] text-muted">{t("triggers.firesEmpty")}</p>
          ) : (
            <ul className="mt-2 grid gap-1.5">
              {fires.map((entry) => (
                <FireRow key={entry.id} entry={entry} />
              ))}
            </ul>
          )}
        </Panel>
      </div>
    </div>
  );
}

function TriggerCard({
  trigger,
  busy,
  onToggle,
  onFire,
}: {
  trigger: TriggerSummary;
  busy: boolean;
  onToggle: () => void;
  onFire: () => void;
}) {
  const { t } = useTranslation();
  const notWired = trigger.kind === "event" || trigger.kind === "mcp";
  return (
    <Panel className="min-w-0 p-3.5">
      <div className="flex flex-wrap items-center gap-1.5">
        {trigger.kind === "schedule" ? (
          <Timer className="h-3.5 w-3.5 text-accent" />
        ) : (
          <Webhook className="h-3.5 w-3.5 text-success" />
        )}
        <span className="min-w-0 truncate text-[12.5px] font-semibold text-ink">
          {trigger.triggerId}
        </span>
        <span className="min-w-0 truncate font-mono text-[10.5px] text-faint">
          {trigger.workflowId}
        </span>
        <Badge tone={kindTone[trigger.kind] ?? "neutral"}>{t(`triggers.kind.${trigger.kind}`)}</Badge>
        <Badge tone={trigger.enabled ? "success" : "neutral"} dot>
          {trigger.enabled ? t("triggers.enabled") : t("triggers.disabled")}
        </Badge>
        <div className="ml-auto flex items-center gap-1">
          {!notWired ? (
            <>
              <Button size="xs" variant="ghost" disabled={busy} onClick={onFire}>
                <Play className="h-3 w-3" />
                {t("triggers.fireNow")}
              </Button>
              <Button size="xs" variant="ghost" disabled={busy} onClick={onToggle}>
                {trigger.enabled ? t("triggers.disable") : t("triggers.enable")}
              </Button>
            </>
          ) : (
            <Badge tone="warning">{t("triggers.notWired")}</Badge>
          )}
        </div>
      </div>

      {trigger.schedule ? (
        <div className="mt-2 grid gap-1">
          <div className="flex min-w-0 flex-wrap items-center gap-1.5">
            <code className="rounded-sm border border-line bg-panel-2 px-1.5 py-0.5 font-mono text-[11px] text-ink">
              {trigger.schedule.cron}
            </code>
            <Badge tone="neutral">{trigger.schedule.timezone}</Badge>
            <Badge tone="neutral">{t(`triggers.misfire.${trigger.schedule.misfire}`)}</Badge>
            {trigger.schedule.misfire === "catch_up" ? (
              <span className="text-[10px] text-faint">
                {t("triggers.maxCatchUp", { limit: trigger.schedule.maxCatchUp })}
              </span>
            ) : null}
          </div>
        </div>
      ) : null}

      {trigger.webhook ? (
        <div className="mt-2 grid gap-1">
          <div className="flex min-w-0 items-center gap-1.5">
            <code className="min-w-0 flex-1 truncate rounded-sm border border-line bg-panel-2 px-1.5 py-1 font-mono text-[10.5px] text-muted">
              {trigger.webhook.url}
            </code>
            <CopyButton text={trigger.webhook.url} />
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            <Badge tone={trigger.webhook.hasSecret ? "success" : "danger"}>
              {trigger.webhook.hasSecret
                ? t("triggers.secretSet")
                : t("triggers.secretMissing")}
            </Badge>
            <span className="font-mono text-[10px] text-faint">
              {trigger.webhook.signatureHeader}
            </span>
          </div>
        </div>
      ) : null}

      <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[10.5px] text-faint">
        {trigger.kind === "schedule" ? (
          <span>
            {t("triggers.nextFire")}:{" "}
            {trigger.nextFireAt ? relativeTime(trigger.nextFireAt, t) : t("triggers.never")}
          </span>
        ) : null}
        <span>
          {t("triggers.lastFire")}:{" "}
          {trigger.lastFireAt ? (
            <Timestamp value={new Date(trigger.lastFireAt).toISOString()} />
          ) : (
            t("triggers.never")
          )}
        </span>
        {trigger.lastExecutionId ? (
          <Link
            to="/executions/$executionId"
            params={{ executionId: trigger.lastExecutionId }}
            className="inline-flex items-center gap-0.5 text-accent hover:underline"
          >
            {t("triggers.openExecution")}
            <ExternalLink className="h-2.5 w-2.5" />
          </Link>
        ) : null}
        {trigger.workflowVersion ? (
          <span className="ml-auto font-mono">v{trigger.workflowVersion}</span>
        ) : null}
      </div>
    </Panel>
  );
}

function FireRow({ entry }: { entry: TriggerFire }) {
  return (
    <li className="min-w-0 rounded-sm border border-line bg-panel-2 px-2 py-1.5">
      <div className="flex min-w-0 items-center gap-1.5">
        <Badge tone={kindTone[entry.kind] ?? "neutral"}>{entry.kind}</Badge>
        <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted">
          {entry.workflowId} › {entry.triggerId}
        </span>
        <Timestamp value={entry.firedAt} />
      </div>
      <div className="mt-0.5 flex min-w-0 items-center gap-1.5 text-[9.5px] text-faint">
        <span className="min-w-0 flex-1 truncate font-mono">{entry.executionId ?? "—"}</span>
        {entry.executionId ? (
          <Link
            to="/executions/$executionId"
            params={{ executionId: entry.executionId }}
            className="shrink-0 text-accent hover:underline"
          >
            <ExternalLink className="h-2.5 w-2.5" />
          </Link>
        ) : null}
      </div>
    </li>
  );
}

function CopyButton({ text }: { text: string }) {
  const { t } = useTranslation();
  return (
    <Button
      size="xs"
      variant="ghost"
      title={t("common.copy")}
      aria-label={t("common.copy")}
      onClick={() => void navigator.clipboard.writeText(text)}
    >
      <Copy className="h-3 w-3" />
    </Button>
  );
}

function relativeTime(target: number, t: (key: string, options?: Record<string, unknown>) => string) {
  const delta = target - Date.now();
  if (delta <= 0) return t("triggers.due");
  const minutes = Math.round(delta / 60_000);
  if (minutes < 60) return t("triggers.inMinutes", { minutes });
  const hours = Math.round(minutes / 60);
  if (hours < 48) return t("triggers.inHours", { hours });
  return t("triggers.inDays", { days: Math.round(hours / 24) });
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
