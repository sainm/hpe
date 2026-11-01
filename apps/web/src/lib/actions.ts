import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "./api";
import { emptyWorkflow, sampleWorkflows } from "./ir";
import { useUiStore } from "../stores/ui";

export function useWorkflowActions() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [busy, setBusy] = useState<"create" | "samples" | null>(null);

  async function createWorkflow() {
    setBusy("create");
    try {
      const summary = await api.saveWorkflow(emptyWorkflow(t("workflows.untitled")));
      await queryClient.invalidateQueries({ queryKey: ["workflows"] });
      await navigate({ to: "/designer", search: { id: summary.id } });
    } catch (error) {
      pushToast(error instanceof Error ? error.message : t("errors.saveFailed"), "error");
    } finally {
      setBusy(null);
    }
  }

  async function loadSamples() {
    setBusy("samples");
    try {
      const samples = sampleWorkflows({
        sequenceName: t("samples.sequenceName"),
        sequenceDescription: t("samples.sequenceDescription"),
        approvalName: t("samples.approvalName"),
        approvalDescription: t("samples.approvalDescription"),
        parallelName: t("samples.parallelName"),
        parallelDescription: t("samples.parallelDescription"),
        approvalTitle: t("node.approval"),
      });
      for (const sample of samples) {
        await api.saveWorkflow(sample);
      }
      await queryClient.invalidateQueries({ queryKey: ["workflows"] });
      pushToast(t("dashboard.samplesAdded"), "success");
    } catch (error) {
      pushToast(error instanceof Error ? error.message : t("errors.saveFailed"), "error");
    } finally {
      setBusy(null);
    }
  }

  return { createWorkflow, loadSamples, busy };
}
