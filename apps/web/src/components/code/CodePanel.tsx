import Editor from "@monaco-editor/react";
import { useQueryClient } from "@tanstack/react-query";
import { Bug, Code2, FileJson, FileText, ShieldCheck } from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { api, ApiError } from "../../lib/api";
import { workflowToText, parseWorkflowText } from "../../lib/ir";
import type { ValidationReport } from "../../lib/types";
import "../../lib/monaco";
import { useEditorStore } from "../../stores/editor";
import { useUiStore } from "../../stores/ui";
import { RunPanel } from "../debug/RunPanel";
import { Badge, Button, ValidationList, cn } from "../ui";

export function CodePanel() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const ir = useEditorStore((state) => state.ir);
  const panel = useEditorStore((state) => state.panel);
  const setPanel = useEditorStore((state) => state.setPanel);
  const replaceIr = useEditorStore((state) => state.replaceIr);
  const issues = useEditorStore((state) => state.issues);
  const setIssues = useEditorStore((state) => state.setIssues);
  const theme = useUiStore((state) => state.theme);
  const pushToast = useUiStore((state) => state.pushToast);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);

  const format = panel === "yaml" ? "yaml" : "json";

  useEffect(() => {
    if (ir && panel !== "debug") {
      setText(workflowToText(format, ir));
    }
  }, [ir, panel, format]);

  const errorCount = useMemo(
    () => issues.filter((issue) => issue.severity === "error").length,
    [issues],
  );

  async function validateText(content: string): Promise<ValidationReport | null> {
    try {
      const result = await api.validate(format, content);
      setIssues(result.report.issues);
      return result.report;
    } catch (error) {
      if (error instanceof ApiError && error.status === 422 && isReport(error.details)) {
        setIssues(error.details.issues);
        return error.details;
      }
      pushToast(error instanceof Error ? error.message : t("errors.unknown"), "error");
      return null;
    }
  }

  async function handleApply() {
    if (!ir) {
      return;
    }
    setBusy(true);
    try {
      const parsed = parseWorkflowText(format, text);
      replaceIr(parsed);
      const report = await validateText(workflowToText(format, parsed));
      if (report && report.issues.every((issue) => issue.severity !== "error")) {
        pushToast(t("designer.applySuccess"), "success");
      } else {
        pushToast(t("designer.validationFailed"), "error");
      }
      await queryClient.invalidateQueries({ queryKey: ["workflows"] });
    } catch (error) {
      pushToast(
        error instanceof Error ? `${t("designer.applyError")}: ${error.message}` : t("designer.applyError"),
        "error",
      );
    } finally {
      setBusy(false);
    }
  }

  async function handleValidate() {
    setBusy(true);
    try {
      const content = panel === "debug" ? (ir ? workflowToText(format, ir) : "") : text;
      if (!content) {
        return;
      }
      const report = await validateText(content);
      if (report) {
        pushToast(
          report.issues.some((issue) => issue.severity === "error")
            ? t("designer.validationFailed")
            : t("designer.validationPassed"),
          report.issues.some((issue) => issue.severity === "error") ? "error" : "success",
        );
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="flex min-h-0 flex-col border-t border-line bg-panel">
      <div className="flex flex-wrap items-center gap-1 border-b border-line px-2 py-0">
        <TabButton
          active={panel === "yaml"}
          onClick={() => setPanel("yaml")}
          icon={<FileText className="h-3.5 w-3.5" />}
          label={t("common.yaml")}
        />
        <TabButton
          active={panel === "json"}
          onClick={() => setPanel("json")}
          icon={<FileJson className="h-3.5 w-3.5" />}
          label={t("common.json")}
        />
        <TabButton
          active={panel === "debug"}
          onClick={() => setPanel("debug")}
          icon={<Bug className="h-3.5 w-3.5" />}
          label={t("designer.debugger")}
        />
        <div className="ml-auto flex items-center gap-1.5 py-1">
          {issues.length > 0 ? (
            <Badge tone={errorCount > 0 ? "danger" : "warning"}>
              {errorCount > 0
                ? t("common.errorsCount", { count: errorCount })
                : t("common.warningsCount", { count: issues.length })}
            </Badge>
          ) : null}
          {panel !== "debug" ? (
            <>
              <Button size="xs" variant="ghost" onClick={() => void handleValidate()} loading={busy}>
                <ShieldCheck className="h-3 w-3" />
                {t("common.validate")}
              </Button>
              <Button size="xs" variant="secondary" onClick={() => void handleApply()} loading={busy}>
                <Code2 className="h-3 w-3" />
                {t("designer.apply")}
              </Button>
            </>
          ) : null}
        </div>
      </div>

      <div className="min-h-0 flex-1">
        {panel === "debug" ? (
          <div className="hpa-scroll h-full overflow-y-auto" style={{ maxHeight: "var(--hpa-code-height)" }}>
            <RunPanel />
          </div>
        ) : (
          <div className="grid" style={{ gridTemplateRows: "minmax(180px, var(--hpa-code-height)) auto" }}>
            <Editor
              height="100%"
              language={panel === "yaml" ? "hpa-yaml" : "json"}
              theme={theme === "light" ? "hpa-light" : "hpa-dark"}
              value={text}
              onChange={(value) => setText(value ?? "")}
              options={{
                minimap: { enabled: false },
                fontSize: 12,
                fontFamily: "var(--hpa-font-mono)",
                lineNumbersMinChars: 3,
                scrollBeyondLastLine: false,
                wordWrap: "on",
                automaticLayout: true,
                tabSize: 2,
                padding: { top: 10, bottom: 10 },
                renderLineHighlight: "all",
                smoothScrolling: true,
                accessibilitySupport: "auto",
              }}
              loading={<span className="p-3 text-xs text-faint">{t("common.loading")}</span>}
            />
            {issues.length > 0 ? (
              <div className="hpa-scroll max-h-[150px] overflow-y-auto border-t border-line p-2">
                <ValidationList report={{ issues }} />
              </div>
            ) : null}
          </div>
        )}
      </div>
    </section>
  );
}

function TabButton({
  active,
  onClick,
  icon,
  label,
}: {
  active: boolean;
  onClick: () => void;
  icon: ReactNode;
  label: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "-mb-px inline-flex h-[31px] items-center gap-1.5 border-b-2 px-2.5 text-[12px] transition-colors",
        active
          ? "border-accent font-semibold text-ink"
          : "border-transparent text-muted hover:text-ink",
      )}
    >
      {icon}
      {label}
    </button>
  );
}

function isReport(value: unknown): value is ValidationReport {
  return Boolean(
    value &&
      typeof value === "object" &&
      Array.isArray((value as ValidationReport).issues),
  );
}
