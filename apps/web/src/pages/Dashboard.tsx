import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Activity, ArrowRight, BadgeCheck, Clock3, Plus, RefreshCw, Workflow } from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  Badge,
  Button,
  EmptyState,
  PageHeader,
  Panel,
  StatusBadge,
  Timestamp,
  cn,
} from "../components/ui";
import { useWorkflowActions } from "../lib/actions";
import { api } from "../lib/api";

export function DashboardPage() {
  const { t } = useTranslation();
  const { createWorkflow, loadSamples, busy } = useWorkflowActions();
  const health = useQuery({ queryKey: ["health"], queryFn: api.health, retry: 1 });
  const stats = useQuery({ queryKey: ["stats"], queryFn: api.stats, refetchInterval: 5_000 });
  const workflows = useQuery({ queryKey: ["workflows"], queryFn: api.listWorkflows });
  const executions = useQuery({
    queryKey: ["executions", 8],
    queryFn: () => api.listExecutions(8),
    refetchInterval: 5_000,
  });

  const metrics = [
    { key: "statsWorkflows", value: stats.data?.workflows ?? 0, icon: Workflow },
    { key: "statsExecutions", value: stats.data?.executions ?? 0, icon: Activity },
    { key: "statsRunning", value: stats.data?.running ?? 0, icon: RefreshCw },
    { key: "statsWaiting", value: stats.data?.waitingApproval ?? 0, icon: BadgeCheck },
  ];

  return (
    <div className="mx-auto grid max-w-[1560px] gap-3">
      <PageHeader
        title={t("dashboard.title")}
        subtitle={t("dashboard.subtitle")}
        actions={
          <>
            <Button variant="secondary" onClick={() => void loadSamples()} loading={busy === "samples"}>
              {t("dashboard.seedSamples")}
            </Button>
            <Button variant="primary" onClick={() => void createWorkflow()} loading={busy === "create"}>
              <Plus className="h-3.5 w-3.5" />
              {t("dashboard.newWorkflow")}
            </Button>
          </>
        }
      />

      <Panel className="grid grid-cols-2 lg:grid-cols-4">
        {metrics.map((metric, index) => {
          const Icon = metric.icon;
          return (
            <div
              key={metric.key}
              className={cn(
                "min-w-0 border-line p-3",
                index % 2 === 1 && "border-l",
                index < 2 && "border-b lg:border-b-0",
                index > 1 && "lg:border-l",
                index === 1 && "lg:border-l",
              )}
            >
              <div className="flex items-center gap-1.5 text-[10.5px] font-medium text-faint">
                <Icon className="h-3 w-3" />
                {t(`dashboard.${metric.key}`)}
              </div>
              <div className="mt-1.5 flex items-baseline gap-1.5">
                <span className="text-[17px] font-semibold leading-none tabular-nums text-ink-strong">
                  {metric.value}
                </span>
              </div>
            </div>
          );
        })}
      </Panel>

      <div className="grid gap-3 xl:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
        <Panel>
          <div className="flex items-center justify-between gap-3 border-b border-line bg-panel-2 px-3 py-2">
            <h2 className="text-[12px] font-semibold text-ink">{t("dashboard.recentExecutions")}</h2>
            <Link
              to="/executions"
              className="inline-flex items-center gap-1 text-[11px] text-muted hover:text-accent"
            >
              {t("common.view")}
              <ArrowRight className="h-3 w-3" />
            </Link>
          </div>
          {executions.data && executions.data.length > 0 ? (
            <div className="hpa-scroll overflow-x-auto">
              <div className="min-w-[600px]">
                <div className="grid grid-cols-[80px_minmax(0,1.3fr)_minmax(0,1fr)_130px] gap-3 border-b border-line bg-panel-3 px-3 py-1.5 text-[10px] font-semibold text-faint">
                  <span>{t("common.status")}</span>
                  <span>{t("common.execution")}</span>
                  <span>{t("common.workflow")}</span>
                  <span>{t("common.startedAt")}</span>
                </div>
                <ul className="divide-y divide-line">
                  {executions.data.slice(0, 8).map((execution) => (
                    <li key={execution.id}>
                      <Link
                        to="/executions/$executionId"
                        params={{ executionId: execution.id }}
                        className="grid grid-cols-[80px_minmax(0,1.3fr)_minmax(0,1fr)_130px] items-center gap-3 px-3 py-2 transition-colors hover:bg-panel-2"
                      >
                        <StatusBadge status={execution.status} />
                        <span className="truncate font-mono text-[10.5px] text-muted">
                          {execution.id}
                        </span>
                        <span className="truncate text-[11.5px] text-ink">{execution.workflowId}</span>
                        <Timestamp value={execution.startedAt} />
                      </Link>
                    </li>
                  ))}
                </ul>
              </div>
            </div>
          ) : (
            <div className="p-3">
              <EmptyState
                icon={<Activity className="h-4 w-4" />}
                title={t("dashboard.emptyExecutions")}
                hint={t("executions.emptyHint")}
              />
            </div>
          )}
        </Panel>

        <Panel>
          <div className="flex items-center justify-between gap-3 border-b border-line bg-panel-2 px-3 py-2">
            <h2 className="text-[12px] font-semibold text-ink">{t("dashboard.recentWorkflows")}</h2>
            <Link
              to="/workflows"
              className="inline-flex items-center gap-1 text-[11px] text-muted hover:text-accent"
            >
              {t("common.view")}
              <ArrowRight className="h-3 w-3" />
            </Link>
          </div>
          {workflows.data && workflows.data.length > 0 ? (
            <ul className="divide-y divide-line">
              {workflows.data.slice(0, 6).map((workflow) => (
                <li key={workflow.id}>
                  <Link
                    to="/designer"
                    search={{ id: workflow.id }}
                    className="flex items-center gap-2.5 px-3 py-2.5 transition-colors hover:bg-panel-2"
                  >
                    <span className="grid h-7 w-7 shrink-0 place-items-center rounded-sm border border-line bg-panel-3 text-accent">
                      <Workflow className="h-3.5 w-3.5" />
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[11.5px] font-medium text-ink">
                        {workflow.name}
                      </span>
                      <span className="block truncate font-mono text-[9.5px] text-faint">
                        {workflow.id} · v{workflow.version}
                      </span>
                    </span>
                    <Badge tone={workflow.published ? "success" : "neutral"}>
                      {workflow.published ? t("common.published") : t("common.draft")}
                    </Badge>
                  </Link>
                </li>
              ))}
            </ul>
          ) : (
            <div className="p-3">
              <EmptyState
                icon={<Workflow className="h-4 w-4" />}
                title={t("dashboard.emptyWorkflows")}
                action={
                  <Button size="sm" variant="secondary" onClick={() => void loadSamples()} loading={busy === "samples"}>
                    {t("dashboard.seedSamples")}
                  </Button>
                }
              />
            </div>
          )}
        </Panel>
      </div>

      <Panel className="flex flex-wrap items-center gap-x-5 gap-y-2 px-3 py-2 text-[10.5px] text-faint">
        <span className="inline-flex items-center gap-1.5">
          <span
            className={cn(
              "h-1.5 w-1.5 rounded-full",
              health.isError ? "bg-danger" : "bg-success",
            )}
          />
          {health.isError ? t("shell.offline") : t("shell.online")}
        </span>
        <span className="font-mono">{health.data?.internalId ?? "hpa"} · {health.data?.version ?? "0.1.0"}</span>
        <span className="inline-flex items-center gap-1">
          <Clock3 className="h-3 w-3" />
          {t("dashboard.offlineReady")}
        </span>
        <span className="ml-auto">{t("dashboard.paletteHint")}</span>
      </Panel>
    </div>
  );
}
