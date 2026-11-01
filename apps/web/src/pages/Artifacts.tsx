import { useQuery } from "@tanstack/react-query";
import { Boxes, FileJson, HardDrive } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  EmptyState,
  JsonView,
  PageHeader,
  Panel,
  Timestamp,
  cn,
} from "../components/ui";
import { api } from "../lib/api";

export function ArtifactsPage() {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<string | null>(null);
  const artifacts = useQuery({ queryKey: ["artifacts"], queryFn: () => api.listArtifacts() });
  const activeId = selected ?? artifacts.data?.[0]?.id ?? null;
  const artifact = useQuery({
    queryKey: ["artifact", activeId],
    queryFn: () => api.getArtifact(activeId ?? ""),
    enabled: Boolean(activeId),
  });

  return (
    <div className="mx-auto grid max-w-[1360px] gap-3">
      <PageHeader title={t("artifacts.title")} subtitle={t("artifacts.subtitle")} />

      {artifacts.isLoading ? (
        <p className="text-sm text-muted">{t("common.loading")}</p>
      ) : artifacts.data?.length === 0 ? (
        <EmptyState
          icon={<Boxes className="h-8 w-8" />}
          title={t("artifacts.empty")}
          hint={t("artifacts.emptyHint")}
        />
      ) : (
        <div className="grid gap-3 lg:grid-cols-[minmax(0,340px)_minmax(0,1fr)]">
          <Panel className="overflow-hidden">
            <ul className="divide-y divide-line">
              {(artifacts.data ?? []).map((entry) => (
                <li key={entry.id}>
                  <button
                    type="button"
                    onClick={() => setSelected(entry.id)}
                    className={cn(
                      "flex w-full items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-panel-2",
                      activeId === entry.id && "bg-panel-2",
                    )}
                  >
                    <span className="grid h-8 w-8 shrink-0 place-items-center rounded-sm border border-line bg-bg-elevated text-success">
                      <FileJson className="h-4 w-4" />
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-xs font-medium text-ink">
                        {entry.name}
                      </span>
                      <span className="block truncate font-mono text-[0.58rem] text-faint">
                        {entry.executionId}
                      </span>
                    </span>
                    <span className="inline-flex items-center gap-1 font-mono text-[0.58rem] text-faint">
                      <HardDrive className="h-3 w-3" />
                      {formatBytes(entry.size)}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </Panel>

          <Panel className="min-w-0 p-3">
            {artifact.data ? (
              <div className="grid gap-4">
                <div className="flex flex-wrap items-center gap-3 border-b border-line pb-3">
                  <div className="min-w-0">
                    <h2 className="truncate text-sm font-semibold text-ink">
                      {artifact.data.artifact.name}
                    </h2>
                    <p className="mt-0.5 truncate font-mono text-[0.62rem] text-faint">
                      {artifact.data.artifact.id}
                    </p>
                  </div>
                  <div className="ml-auto flex flex-wrap items-center gap-3 text-[0.66rem] text-muted">
                    <span>{artifact.data.artifact.mimeType}</span>
                    <span>{formatBytes(artifact.data.artifact.size)}</span>
                    <Timestamp value={artifact.data.artifact.createdAt} />
                  </div>
                </div>
                <div>
                  <h3 className="mb-2 text-[0.64rem] font-semibold uppercase tracking-[0.14em] text-faint">
                    {t("artifacts.content")}
                  </h3>
                  <JsonView value={artifact.data.content} maxHeight="560px" />
                </div>
              </div>
            ) : (
              <p className="text-xs text-faint">{t("common.noData")}</p>
            )}
          </Panel>
        </div>
      )}
    </div>
  );
}

function formatBytes(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}
