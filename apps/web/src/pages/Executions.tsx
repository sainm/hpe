import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Activity, ArrowUpRight, ChevronLeft, ChevronRight, RefreshCw } from "lucide-react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Button,
  EmptyState,
  PageHeader,
  Panel,
  StatusBadge,
  Timestamp,
  cn,
} from "../components/ui";
import { api } from "../lib/api";
import type { ExecutionStatus } from "../lib/types";

const filters: Array<ExecutionStatus | "all"> = [
  "all",
  "running",
  "waiting_approval",
  "succeeded",
  "failed",
];

const PAGE_SIZE = 50;

export function ExecutionsPage() {
  const { t } = useTranslation();
  const [filter, setFilter] = useState<ExecutionStatus | "all">("all");
  const [page, setPage] = useState(0);
  const executions = useQuery({
    queryKey: ["executions", filter, page],
    // Paging and the status filter happen on the server so the console can show
    // more than one page of history (the old client-side filter only saw 100).
    queryFn: () =>
      api.listExecutionsPage({ limit: PAGE_SIZE, offset: page * PAGE_SIZE, status: filter }),
    refetchInterval: 4_000,
  });

  const rows = useMemo(() => executions.data?.data ?? [], [executions.data]);
  const total = executions.data?.total ?? 0;
  const pageCount = Math.max(1, Math.ceil(total / PAGE_SIZE));

  const selectFilter = (next: ExecutionStatus | "all") => {
    setFilter(next);
    setPage(0);
  };

  return (
    <div className="mx-auto grid max-w-[1560px] gap-3">
      <PageHeader
        title={t("executions.title")}
        subtitle={t("executions.subtitle")}
        actions={
          <Button variant="secondary" onClick={() => void executions.refetch()} loading={executions.isFetching}>
            <RefreshCw className="h-3.5 w-3.5" />
            {t("common.refresh")}
          </Button>
        }
      />

      <div className="flex flex-wrap items-center gap-1">
        {filters.map((entry) => (
          <button
            key={entry}
            type="button"
            onClick={() => selectFilter(entry)}
            className={cn(
              "inline-flex h-6 items-center gap-1.5 rounded-sm border px-2 text-[11px] transition-colors",
              filter === entry
                ? "border-accent/40 bg-accent-soft text-accent"
                : "border-transparent bg-transparent text-muted hover:bg-panel-3 hover:text-ink",
            )}
          >
            {entry === "all" ? t("common.all") : t(`status.${entry}`)}
          </button>
        ))}
      </div>

      <Panel>
        {executions.isLoading ? (
          <p className="p-4 text-xs text-muted">{t("common.loading")}</p>
        ) : rows.length === 0 ? (
          <div className="p-3">
            <EmptyState
              icon={<Activity className="h-4 w-4" />}
              title={total > 0 ? t("executions.noMatch") : t("executions.empty")}
              hint={total > 0 ? undefined : t("executions.emptyHint")}
            />
          </div>
        ) : (
          <>
            <div className="hidden grid-cols-[92px_minmax(0,1fr)_150px_130px_28px] gap-3 border-b border-line bg-panel-3 px-3 py-1.5 text-[10px] font-semibold text-faint sm:grid">
              <span>{t("common.status")}</span>
              <span>{t("common.execution")}</span>
              <span>{t("common.workflow")}</span>
              <span>{t("common.startedAt")}</span>
              <span />
            </div>
            <ul className="divide-y divide-line">
              {rows.map((execution) => (
                <li key={execution.id}>
                  <Link
                    to="/executions/$executionId"
                    params={{ executionId: execution.id }}
                    className="group grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 px-3 py-2 transition-colors hover:bg-panel-2 sm:grid-cols-[92px_minmax(0,1fr)_150px_130px_28px]"
                  >
                    <StatusBadge status={execution.status} />
                    <span className="min-w-0">
                      <span className="block truncate font-mono text-[10.5px] text-muted">
                        {execution.id}
                      </span>
                      <span className="block truncate text-[10px] text-faint sm:hidden">
                        {execution.workflowId}
                      </span>
                    </span>
                    <span className="hidden truncate text-[11.5px] text-ink sm:block">
                      {execution.workflowId} · v{execution.workflowVersion}
                    </span>
                    <span className="hidden sm:block">
                      <Timestamp value={execution.startedAt} />
                    </span>
                    <ArrowUpRight className="h-3.5 w-3.5 text-faint group-hover:text-accent" />
                  </Link>
                </li>
              ))}
            </ul>
          </>
        )}
        <div className="flex flex-wrap items-center justify-between gap-2 border-t border-line px-3 py-2">
          <span className="text-[10.5px] text-faint">
            {t("executions.pageSummary", {
              total,
              current: Math.min(page + 1, pageCount),
              pages: pageCount,
            })}
          </span>
          <div className="flex items-center gap-1.5">
            <Button
              variant="ghost"
              size="sm"
              disabled={page === 0}
              onClick={() => setPage((current) => Math.max(0, current - 1))}
            >
              <ChevronLeft className="h-3.5 w-3.5" />
              {t("common.prev")}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              disabled={page + 1 >= pageCount}
              onClick={() => setPage((current) => current + 1)}
            >
              {t("common.next")}
              <ChevronRight className="h-3.5 w-3.5" />
            </Button>
          </div>
        </div>
      </Panel>
    </div>
  );
}
