import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { BadgeCheck, Clock3, ExternalLink, ShieldCheck } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Badge,
  Button,
  EmptyState,
  PageHeader,
  Panel,
  cn,
  inputClass,
} from "../components/ui";
import { api } from "../lib/api";
import { formatDate } from "../lib/i18n";
import { useUiStore } from "../stores/ui";

export function ApprovalsPage() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [comments, setComments] = useState<Record<string, string>>({});

  const approvals = useQuery({
    queryKey: ["approvals"],
    queryFn: api.listApprovals,
    refetchInterval: 5_000,
  });

  const decide = useMutation({
    mutationFn: ({
      executionId,
      nodeId,
      decision,
      comment,
    }: {
      executionId: string;
      nodeId: string;
      decision: "approved" | "rejected";
      comment: string;
    }) =>
      api.decideApproval(
        executionId,
        nodeId,
        decision,
        comment.trim() ? { comment: comment.trim() } : {},
      ),
    onSuccess: (_data, variables) => {
      setComments((current) => ({ ...current, [variables.executionId]: "" }));
      pushToast(
        variables.decision === "approved" ? t("approvals.approveSuccess") : t("approvals.rejectSuccess"),
        "success",
      );
      void queryClient.invalidateQueries({ queryKey: ["approvals"] });
      void queryClient.invalidateQueries({ queryKey: ["executions"] });
      void queryClient.invalidateQueries({ queryKey: ["execution", variables.executionId] });
    },
    onError: (error) =>
      pushToast(error instanceof Error ? error.message : t("approvals.error"), "error"),
  });

  return (
    <div className="mx-auto grid max-w-[1100px] gap-3">
      <PageHeader title={t("approvals.title")} subtitle={t("approvals.subtitle")} />

      {approvals.isLoading ? (
        <p className="text-sm text-muted">{t("common.loading")}</p>
      ) : approvals.data?.length === 0 ? (
        <EmptyState
          icon={<ShieldCheck className="h-8 w-8" />}
          title={t("approvals.empty")}
          hint={t("approvals.emptyHint")}
        />
      ) : (
        <div className="grid gap-3">
          {(approvals.data ?? []).map((approval) => {
            const comment = comments[approval.executionId] ?? "";
            const pending =
              decide.isPending && decide.variables?.executionId === approval.executionId;
            return (
              <Panel key={approval.request.id} className="overflow-hidden">
                <div className="flex flex-wrap items-start gap-4 border-b border-line p-3">
                  <span className="grid h-10 w-10 place-items-center rounded-md border border-warning/30 bg-warning/10 text-warning">
                    <BadgeCheck className="h-5 w-5" />
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <h2 className="text-sm font-semibold text-ink-strong">
                        {approval.request.title}
                      </h2>
                      <Badge tone="warning" dot>
                        {t("status.waiting_approval")}
                      </Badge>
                    </div>
                    <p className="mt-1 truncate font-mono text-[0.64rem] text-muted">
                      {approval.workflowId} · v{approval.workflowVersion}
                    </p>
                    <div className="mt-2 flex flex-wrap items-center gap-4 text-[0.68rem] text-faint">
                      <span>
                        {t("approvals.node")}:{" "}
                        <span className="font-mono text-muted">{approval.request.nodeId}</span>
                      </span>
                      <span className="inline-flex items-center gap-1">
                        <Clock3 className="h-3 w-3" />
                        {formatDate(approval.request.requestedAt)}
                      </span>
                      <span>
                        {t("approvals.sla")}:{" "}
                        {approval.request.slaMs
                          ? t("approvals.minutes", {
                              count: Math.round(approval.request.slaMs / 60_000),
                            })
                          : t("approvals.noSla")}
                      </span>
                    </div>
                  </div>
                  <Link
                    to="/executions/$executionId"
                    params={{ executionId: approval.executionId }}
                    className="inline-flex items-center gap-1 text-xs text-muted hover:text-accent"
                  >
                    {t("approvals.openExecution")}
                    <ExternalLink className="h-3 w-3" />
                  </Link>
                </div>
                <div className="grid gap-3 p-3 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-end">
                  <input
                    className={cn(inputClass, "sm:max-w-md")}
                    value={comment}
                    onChange={(event) =>
                      setComments((current) => ({
                        ...current,
                        [approval.executionId]: event.target.value,
                      }))
                    }
                    placeholder={t("approvals.commentPlaceholder")}
                  />
                  <div className="flex items-center gap-2 sm:justify-end">
                    <Button
                      variant="danger"
                      onClick={() =>
                        decide.mutate({
                          executionId: approval.executionId,
                          nodeId: approval.request.nodeId,
                          decision: "rejected",
                          comment,
                        })
                      }
                      loading={pending}
                    >
                      {t("common.reject")}
                    </Button>
                    <Button
                      variant="primary"
                      onClick={() =>
                        decide.mutate({
                          executionId: approval.executionId,
                          nodeId: approval.request.nodeId,
                          decision: "approved",
                          comment,
                        })
                      }
                      loading={pending}
                    >
                      <BadgeCheck className="h-3.5 w-3.5" />
                      {t("common.approve")}
                    </Button>
                  </div>
                </div>
              </Panel>
            );
          })}
        </div>
      )}
    </div>
  );
}
