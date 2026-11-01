import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowUpRight, Plus, Search, Workflow } from "lucide-react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Badge,
  Button,
  EmptyState,
  PageHeader,
  Panel,
  Timestamp,
  inputClass,
} from "../components/ui";
import { useWorkflowActions } from "../lib/actions";
import { api } from "../lib/api";

export function WorkflowsPage() {
  const { t } = useTranslation();
  const { createWorkflow, loadSamples, busy } = useWorkflowActions();
  const [query, setQuery] = useState("");
  const workflows = useQuery({ queryKey: ["workflows"], queryFn: api.listWorkflows });

  const filtered = useMemo(() => {
    const text = query.trim().toLowerCase();
    if (!text) {
      return workflows.data ?? [];
    }
    return (workflows.data ?? []).filter(
      (workflow) =>
        workflow.name.toLowerCase().includes(text) ||
        workflow.id.toLowerCase().includes(text),
    );
  }, [query, workflows.data]);

  return (
    <div className="mx-auto grid max-w-[1560px] gap-3">
      <PageHeader
        title={t("workflows.title")}
        subtitle={t("workflows.subtitle")}
        actions={
          <>
            <Button variant="secondary" onClick={() => void loadSamples()} loading={busy === "samples"}>
              {t("workflows.loadSamples")}
            </Button>
            <Button variant="primary" onClick={() => void createWorkflow()} loading={busy === "create"}>
              <Plus className="h-3.5 w-3.5" />
              {t("workflows.create")}
            </Button>
          </>
        }
      />

      <div className="relative max-w-[320px]">
        <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-faint" />
        <input
          className={`${inputClass} pl-8`}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder={t("workflows.searchPlaceholder")}
          aria-label={t("common.search")}
        />
      </div>

      <Panel>
        {workflows.isLoading ? (
          <p className="p-4 text-xs text-muted">{t("common.loading")}</p>
        ) : filtered.length === 0 ? (
          <div className="p-3">
            <EmptyState
              icon={<Workflow className="h-4 w-4" />}
              title={query ? t("workflows.noMatch") : t("workflows.empty")}
              hint={query ? undefined : t("workflows.emptyHint")}
              action={
                query ? (
                  <Button size="sm" variant="ghost" onClick={() => setQuery("")}>
                    {t("common.all")}
                  </Button>
                ) : (
                  <Button size="sm" variant="secondary" onClick={() => void loadSamples()} loading={busy === "samples"}>
                    {t("workflows.loadSamples")}
                  </Button>
                )
              }
            />
          </div>
        ) : (
          <>
            <div className="hidden grid-cols-[minmax(0,1.4fr)_100px_140px_90px] gap-3 border-b border-line bg-panel-3 px-3 py-1.5 text-[10px] font-semibold text-faint sm:grid">
              <span>{t("common.name")}</span>
              <span>{t("common.status")}</span>
              <span>{t("common.updatedAt")}</span>
              <span className="text-right">{t("common.actions")}</span>
            </div>
            <ul className="divide-y divide-line">
              {filtered.map((workflow) => (
                <li key={workflow.id}>
                  <Link
                    to="/designer"
                    search={{ id: workflow.id }}
                    className="group grid grid-cols-[minmax(0,1fr)_auto] items-center gap-3 px-3 py-2 transition-colors hover:bg-panel-2 sm:grid-cols-[minmax(0,1.4fr)_100px_140px_90px]"
                  >
                    <span className="flex min-w-0 items-center gap-2.5">
                      <span className="grid h-7 w-7 shrink-0 place-items-center rounded-sm border border-line bg-panel-3 text-accent">
                        <Workflow className="h-3.5 w-3.5" />
                      </span>
                      <span className="min-w-0">
                        <span className="block truncate text-[11.5px] font-medium text-ink">
                          {workflow.name}
                        </span>
                        <span className="block truncate font-mono text-[9.5px] text-faint">
                          {workflow.id} · v{workflow.version}
                        </span>
                      </span>
                    </span>
                    <span className="hidden sm:block">
                      <Badge tone={workflow.published ? "success" : "neutral"}>
                        {workflow.published ? t("workflows.published") : t("workflows.draft")}
                      </Badge>
                    </span>
                    <span className="hidden font-mono text-[10px] text-faint sm:block">
                      <Timestamp value={workflow.updatedAt} />
                    </span>
                    <span className="flex items-center justify-end gap-1 text-[11px] text-muted group-hover:text-accent">
                      <span className="hidden sm:inline">{t("common.open")}</span>
                      <ArrowUpRight className="h-3.5 w-3.5" />
                    </span>
                  </Link>
                </li>
              ))}
            </ul>
          </>
        )}
      </Panel>
    </div>
  );
}
