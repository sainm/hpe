import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Boxes,
  Download,
  FileCode2,
  FlaskConical,
  PencilLine,
  Play,
  RefreshCw,
  Rocket,
  Save,
  ShieldCheck,
  Upload,
} from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Badge,
  Button,
  EmptyState,
  Field,
  JsonView,
  PageHeader,
  Panel,
  Timestamp,
  ValidationList,
  cn,
  inputClass,
  textareaClass,
} from "../components/ui";
import { api, ApiError } from "../lib/api";
import type {
  JsonValue,
  SkillRunResult,
  SkillTestResult,
} from "../lib/types";
import { useUiStore } from "../stores/ui";

function statusTone(status: string): "success" | "warning" | "neutral" {
  if (status === "published") return "success";
  if (status === "imported") return "warning";
  return "neutral";
}

export function SkillsPage() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [version, setVersion] = useState<string | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const [importPath, setImportPath] = useState("");
  const [exportPath, setExportPath] = useState("/tmp/hpa-skill-export");
  const [inputText, setInputText] = useState("{}");
  const [scriptId, setScriptId] = useState("");
  const [runResult, setRunResult] = useState<SkillRunResult | null>(null);
  const [testResults, setTestResults] = useState<SkillTestResult[] | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [editorVersion, setEditorVersion] = useState("");
  const [editorFiles, setEditorFiles] = useState<Record<string, string>>({});
  const [editorFile, setEditorFile] = useState("");

  const skills = useQuery({ queryKey: ["skills"], queryFn: api.listSkills, retry: 1 });
  const show = useQuery({
    queryKey: ["skill", selectedId],
    queryFn: () => api.getSkill(selectedId ?? ""),
    enabled: Boolean(selectedId),
    retry: 1,
  });
  const detail = useQuery({
    queryKey: ["skill-version", selectedId, version],
    queryFn: () => api.getSkillVersion(selectedId ?? "", version ?? ""),
    enabled: Boolean(selectedId && version),
    retry: 1,
  });
  const runs = useQuery({
    queryKey: ["skill-runs", selectedId],
    queryFn: () => api.listSkillRuns(selectedId ?? "", 20),
    enabled: Boolean(selectedId),
    retry: 1,
  });
  const source = useQuery({
    queryKey: ["skill-source", selectedId, version],
    queryFn: () => api.getSkillSource(selectedId ?? "", version ?? ""),
    enabled: Boolean(selectedId && version && editorOpen),
    retry: 1,
  });

  useEffect(() => {
    if (!selectedId && skills.data?.skills.length) {
      setSelectedId(skills.data.skills[0]!.id);
    }
  }, [skills.data, selectedId]);

  useEffect(() => {
    if (!show.data) return;
    setVersion((current) =>
      current && show.data?.versions.some((entry) => entry.version === current)
        ? current
        : show.data?.summary.latestVersion ?? null,
    );
  }, [show.data]);

  const data = detail.data;
  useEffect(() => {
    if (!data) return;
    setScriptId(data.scripts[0]?.id ?? "");
    setInputText(JSON.stringify(data.tests[0]?.input ?? {}, null, 2));
    setRunResult(null);
    setTestResults(null);
    setEditorOpen(false);
  }, [data?.skill.id, data?.skill.version]);

  useEffect(() => {
    if (!source.data || !editorOpen) return;
    setEditorFiles(source.data.files);
    setEditorFile(Object.keys(source.data.files)[0] ?? "");
    setEditorVersion(bumpPatch(source.data.version));
  }, [editorOpen, source.data]);

  const importSkill = useMutation({
    mutationFn: () => api.importSkill(importPath.trim()),
    onSuccess: (result) => {
      pushToast(t("skills.importDone"), "success");
      setImportOpen(false);
      setSelectedId(result.skill.skill.id);
      setVersion(result.skill.skill.version);
      void queryClient.invalidateQueries({ queryKey: ["skills"] });
      void queryClient.invalidateQueries({ queryKey: ["skill", result.skill.skill.id] });
      void queryClient.invalidateQueries({ queryKey: ["skill-version"] });
    },
    onError: (error) => pushToast(messageOf(error, t("skills.import"), t), "error"),
  });

  const publishSkill = useMutation({
    mutationFn: () => api.publishSkill(selectedId ?? "", version ?? ""),
    onSuccess: () => {
      pushToast(t("skills.published"), "success");
      void queryClient.invalidateQueries({ queryKey: ["skills"] });
      void queryClient.invalidateQueries({ queryKey: ["skill", selectedId] });
      void queryClient.invalidateQueries({ queryKey: ["skill-version", selectedId, version] });
    },
    onError: (error) => pushToast(messageOf(error, t("skills.publish"), t), "error"),
  });

  const saveDraft = useMutation({
    mutationFn: () =>
      api.saveSkillDraft(
        selectedId ?? "",
        version ?? "",
        editorVersion.trim(),
        editorFiles,
      ),
    onSuccess: (result) => {
      pushToast(t("skills.draftSaved"), "success");
      setEditorOpen(false);
      setVersion(result.skill.skill.version);
      void queryClient.invalidateQueries({ queryKey: ["skills"] });
      void queryClient.invalidateQueries({ queryKey: ["skill", result.skill.skill.id] });
      void queryClient.invalidateQueries({ queryKey: ["skill-version"] });
    },
    onError: (error) => pushToast(messageOf(error, t("skills.saveDraft"), t), "error"),
  });

  const signSkill = useMutation({
    mutationFn: () => api.signSkill(selectedId ?? "", version ?? ""),
    onSuccess: () => {
      pushToast(t("skills.signatureSaved"), "success");
      void queryClient.invalidateQueries({ queryKey: ["skill-version", selectedId, version] });
    },
    onError: (error) => pushToast(messageOf(error, t("skills.signSkill"), t), "error"),
  });

  const exportSkill = useMutation({
    mutationFn: () => api.exportSkill(selectedId ?? "", version ?? "", exportPath.trim()),
    onSuccess: (result) => pushToast(t("skills.exportedTo", { path: result.path }), "success"),
    onError: (error) => pushToast(messageOf(error, t("skills.export"), t), "error"),
  });

  const testSkill = useMutation({
    mutationFn: () => api.testSkill(selectedId ?? "", version ?? ""),
    onSuccess: (result) => {
      setTestResults(result.results);
      pushToast(
        result.passed ? t("skills.testPassed") : t("skills.testFailed"),
        result.passed ? "success" : "error",
      );
      void queryClient.invalidateQueries({ queryKey: ["skill-runs", selectedId] });
    },
    onError: (error) => pushToast(messageOf(error, t("skills.tests"), t), "error"),
  });

  const runSkill = useMutation({
    mutationFn: async () => {
      let input: JsonValue;
      try {
        input = JSON.parse(inputText) as JsonValue;
      } catch {
        throw new ApiError(0, "INPUT_JSON", t("skills.inputInvalid"));
      }
      return api.runSkill(selectedId ?? "", version ?? "", input, scriptId || undefined);
    },
    onSuccess: (result) => {
      setRunResult(result.run);
      pushToast(
        result.run.status === "succeeded" ? t("skills.runSucceeded") : t("skills.runFailed"),
        result.run.status === "succeeded" ? "success" : "error",
      );
      void queryClient.invalidateQueries({ queryKey: ["skill-runs", selectedId] });
    },
    onError: (error) => pushToast(messageOf(error, t("skills.run"), t), "error"),
  });

  function selectSkill(id: string) {
    if (id === selectedId) {
      return;
    }
    setSelectedId(id);
    setVersion(null);
    setRunResult(null);
    setTestResults(null);
  }

  const skillList = skills.data?.skills ?? [];

  return (
    <div className="mx-auto grid max-w-[1560px] gap-3">
      <PageHeader
        title={t("skills.title")}
        subtitle={t("skills.subtitle")}
        actions={
          <>
            <Button
              variant="secondary"
              onClick={() => void queryClient.invalidateQueries({ queryKey: ["skills"] })}
            >
              <RefreshCw className="h-3.5 w-3.5" />
              {t("common.refresh")}
            </Button>
            <Button variant="primary" onClick={() => setImportOpen((value) => !value)}>
              <Upload className="h-3.5 w-3.5" />
              {t("skills.import")}
            </Button>
          </>
        }
      />

      {importOpen ? (
        <Panel className="p-4">
          <div className="grid gap-3 md:grid-cols-[minmax(0,1fr)_auto] md:items-end">
            <Field label={t("skills.importPath")} hint={t("skills.importHint")}>
              <input
                className={inputClass}
                value={importPath}
                onChange={(event) => setImportPath(event.target.value)}
                placeholder="/path/to/skill"
              />
            </Field>
            <Button
              variant="primary"
              loading={importSkill.isPending}
              disabled={!importPath.trim()}
              onClick={() => importSkill.mutate()}
            >
              {t("skills.importAction")}
            </Button>
          </div>
        </Panel>
      ) : null}

      <div className="grid gap-3 lg:grid-cols-[280px_minmax(0,1fr)]">
        <Panel className="p-2">
          <div className="flex items-center gap-2 px-2 py-1.5">
            <Boxes className="h-3.5 w-3.5 text-accent" />
            <span className="text-xs font-semibold text-ink">{t("skills.registry")}</span>
            <span className="ml-auto font-mono text-[10px] text-faint">
              {skills.data?.sandbox ?? "—"}
            </span>
          </div>
          {skills.isLoading ? (
            <p className="p-3 text-xs text-muted">{t("common.loading")}</p>
          ) : skillList.length === 0 ? (
            <EmptyState
              icon={<Boxes className="h-5 w-5" />}
              title={t("skills.empty")}
              hint={t("skills.emptyHint")}
            />
          ) : (
            <ul className="grid gap-1">
              {skillList.map((skill) => (
                <li key={skill.id}>
                  <button
                    type="button"
                    onClick={() => selectSkill(skill.id)}
                    className={cn(
                      "w-full rounded-sm border px-2.5 py-2 text-left transition-colors",
                      selectedId === skill.id
                        ? "border-accent/45 bg-accent/10"
                        : "border-transparent hover:border-line hover:bg-panel-2",
                    )}
                  >
                    <span className="flex items-center gap-2">
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-xs font-medium text-ink">
                          {skill.name}
                        </span>
                        <span className="block truncate font-mono text-[9.5px] text-faint">
                          {skill.id} · v{skill.latestVersion}
                        </span>
                      </span>
                      <Badge tone={statusTone(skill.status)}>
                        {t(`skills.status.${skill.status}`, {
                          defaultValue: skill.status,
                        })}
                      </Badge>
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </Panel>

        {!selectedId ? (
          <Panel className="p-4">
            <EmptyState
              icon={<FileCode2 className="h-6 w-6" />}
              title={t("skills.selectSkill")}
              hint={t("skills.emptyHint")}
            />
          </Panel>
        ) : detail.isError ? (
          <Panel className="p-4 text-xs text-danger">{t("errors.loadFailed")}</Panel>
        ) : !data ? (
          <Panel className="p-4 text-xs text-muted">{t("common.loading")}</Panel>
        ) : (
          <div className="grid min-w-0 gap-3">
            <Panel className="p-4">
              <div className="flex flex-wrap items-start gap-3">
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <h2 className="text-base font-semibold text-ink">{data.skill.name}</h2>
                    <Badge tone={statusTone(data.skill.status)}>
                      {t(`skills.status.${data.skill.status}`, {
                        defaultValue: data.skill.status,
                      })}
                    </Badge>
                    <Badge tone={data.summary.licenseAllowed ? "success" : "danger"}>
                      {data.skill.license ?? t("skills.noLicense")}
                    </Badge>
                  </div>
                  <p className="mt-1 break-all font-mono text-[10px] text-faint">
                    {data.skill.id} · {data.skill.contentHash}
                  </p>
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <select
                    className={cn(inputClass, "h-7 w-auto font-mono text-xs")}
                    value={version ?? ""}
                    onChange={(event) => setVersion(event.target.value)}
                    aria-label={t("skills.versions")}
                  >
                    {(show.data?.versions ?? []).map((entry) => (
                      <option key={entry.version} value={entry.version}>
                        v{entry.version}
                      </option>
                    ))}
                  </select>
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => setEditorOpen((value) => !value)}
                  >
                    <PencilLine className="h-3.5 w-3.5" />
                    {t("skills.editDraft")}
                  </Button>
                  <Button
                    size="sm"
                    variant="secondary"
                    loading={signSkill.isPending}
                    onClick={() => signSkill.mutate()}
                  >
                    <ShieldCheck className="h-3.5 w-3.5" />
                    {data.signature?.verified ? t("skills.signed") : t("skills.signSkill")}
                  </Button>
                  <Button
                    size="sm"
                    variant="secondary"
                    loading={publishSkill.isPending}
                    disabled={data.skill.status === "published" || !data.summary.licenseAllowed}
                    onClick={() => publishSkill.mutate()}
                  >
                    <Rocket className="h-3.5 w-3.5" />
                    {t("skills.publish")}
                  </Button>
                  <Button
                    size="sm"
                    variant="secondary"
                    loading={testSkill.isPending}
                    disabled={data.tests.length === 0}
                    onClick={() => testSkill.mutate()}
                  >
                    <FlaskConical className="h-3.5 w-3.5" />
                    {t("skills.tests")}
                  </Button>
                </div>
              </div>

              <dl className="mt-4 grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
                <Info label={t("skills.entry")} value={t(`skills.entryKind.${data.entry}`)} />
                <Info
                  label={t("skills.license")}
                  value={data.skill.license ?? t("skills.noLicense")}
                />
                <Info label={t("skills.versions")} value={version ?? "—"} mono />
                <Info label={t("skills.sandbox")} value={show.data?.sandbox ?? "process"} mono />
                <Info
                  label={t("skills.signature")}
                  value={
                    data.signature?.verified
                      ? `${data.signature.keyId} · ${t("skills.signatureVerified")}`
                      : t("skills.unsigned")
                  }
                  mono
                />
              </dl>

              {data.report.issues.length > 0 ? (
                <div className="mt-4">
                  <p className="mb-2 text-xs font-semibold text-ink">
                    {t("skills.reportTitle")}
                  </p>
                  <ValidationList report={{ issues: data.report.issues }} />
                </div>
              ) : null}
            </Panel>

            {editorOpen ? (
              <Panel className="p-4">
                <div className="flex flex-wrap items-center gap-2">
                  <PencilLine className="h-3.5 w-3.5 text-accent" />
                  <h3 className="text-sm font-semibold text-ink">{t("skills.editDraft")}</h3>
                  <span className="text-[10.5px] text-muted">{t("skills.editDraftHint")}</span>
                  <input
                    className={cn(
                      inputClass,
                      "ml-auto h-7 w-[130px] font-mono text-xs",
                    )}
                    value={editorVersion}
                    onChange={(event) => setEditorVersion(event.target.value)}
                    aria-label={t("skills.newVersion")}
                  />
                  <Button
                    size="sm"
                    variant="primary"
                    loading={saveDraft.isPending}
                    disabled={
                      source.isLoading ||
                      !editorVersion.trim() ||
                      !editorFile ||
                      editorVersion.trim() === version
                    }
                    onClick={() => saveDraft.mutate()}
                  >
                    <Save className="h-3.5 w-3.5" />
                    {t("skills.saveDraft")}
                  </Button>
                </div>
                {source.isError ? (
                  <p className="mt-3 text-xs text-danger">{t("errors.loadFailed")}</p>
                ) : source.isLoading ? (
                  <p className="mt-3 text-xs text-muted">{t("common.loading")}</p>
                ) : (
                  <div className="mt-3 grid gap-3 md:grid-cols-[240px_minmax(0,1fr)]">
                    <div className="grid content-start gap-1">
                      {Object.keys(editorFiles).map((file) => (
                        <button
                          key={file}
                          type="button"
                          onClick={() => setEditorFile(file)}
                          className={cn(
                            "truncate rounded-sm border px-2.5 py-2 text-left font-mono text-[10.5px]",
                            editorFile === file
                              ? "border-accent/45 bg-accent/10 text-accent"
                              : "border-line bg-panel-2 text-muted hover:border-line-strong",
                          )}
                        >
                          {file}
                        </button>
                      ))}
                      {(source.data?.binaryFiles ?? []).map((file) => (
                        <span
                          key={file}
                          className="truncate rounded-sm border border-line bg-panel-2 px-2.5 py-2 font-mono text-[10px] text-faint"
                          title={t("skills.binaryReadOnly")}
                        >
                          {file}
                        </span>
                      ))}
                    </div>
                    <textarea
                      className={cn(
                        textareaClass,
                        "min-h-[320px] font-mono text-[11px]",
                      )}
                      value={editorFile ? editorFiles[editorFile] ?? "" : ""}
                      onChange={(event) =>
                        setEditorFiles((current) => ({
                          ...current,
                          [editorFile]: event.target.value,
                        }))
                      }
                      aria-label={t("skills.editFile")}
                      spellCheck={false}
                    />
                  </div>
                )}
              </Panel>
            ) : null}

            <div className="grid gap-3 xl:grid-cols-2">
              <Panel className="p-4">
                <h3 className="text-sm font-semibold text-ink">{t("skills.scripts")}</h3>
                {data.scripts.length === 0 ? (
                  <p className="mt-2 text-xs text-muted">{t("skills.noScripts")}</p>
                ) : (
                  <div className="mt-3 divide-y divide-line rounded-sm border border-line">
                    {data.scripts.map((script) => (
                      <div key={script.id} className="grid gap-1 px-3 py-2">
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="font-mono text-xs text-ink">{script.id}</span>
                          <Badge tone="neutral">{script.runtime ?? "python3"}</Badge>
                          <span className="ml-auto font-mono text-[9.5px] text-faint">
                            {script.timeoutMs ?? "—"} ms · {script.network ?? "none"}
                          </span>
                        </div>
                        <p className="break-all font-mono text-[10px] text-muted">
                          {script.path}
                        </p>
                        <p className="break-all font-mono text-[9.5px] text-faint">
                          {script.args.join(" ")}
                        </p>
                      </div>
                    ))}
                  </div>
                )}
              </Panel>

              <Panel className="p-4">
                <h3 className="text-sm font-semibold text-ink">{t("skills.tests")}</h3>
                {data.tests.length === 0 ? (
                  <p className="mt-2 text-xs text-muted">{t("skills.noTests")}</p>
                ) : (
                  <div className="mt-3 divide-y divide-line rounded-sm border border-line">
                    {data.tests.map((test) => (
                      <div key={test.name} className="flex flex-wrap items-center gap-2 px-3 py-2">
                        <span className="font-mono text-xs text-ink">{test.name}</span>
                        <span className="font-mono text-[10px] text-muted">{test.script}</span>
                        <span className="ml-auto font-mono text-[10px] text-faint">
                          exit {test.expect.exitCode ?? 0}
                        </span>
                      </div>
                    ))}
                  </div>
                )}
                {testResults ? (
                  <div className="mt-3 grid gap-1.5">
                    {testResults.map((result) => (
                      <div
                        key={result.name}
                        className={cn(
                          "rounded-sm border px-2.5 py-2 text-xs",
                          result.passed
                            ? "border-success/30 bg-success/5"
                            : "border-danger/30 bg-danger/5",
                        )}
                      >
                        <div className="flex items-center gap-2">
                          <span className="font-mono text-ink">{result.name}</span>
                          <Badge tone={result.passed ? "success" : "danger"}>
                            {result.passed ? t("skills.passed") : t("skills.failed")}
                          </Badge>
                          <span className="ml-auto font-mono text-[9.5px] text-faint">
                            {result.run?.durationMs ?? 0} ms
                          </span>
                        </div>
                        {result.failures.map((failure) => (
                          <p key={failure} className="mt-1 text-[10.5px] text-danger">
                            {failure}
                          </p>
                        ))}
                        {result.error ? (
                          <p className="mt-1 text-[10.5px] text-danger">{result.error}</p>
                        ) : null}
                      </div>
                    ))}
                  </div>
                ) : null}
              </Panel>
            </div>

            <Panel className="p-4">
              <div className="flex flex-wrap items-center gap-2">
                <Play className="h-3.5 w-3.5 text-accent" />
                <h3 className="text-sm font-semibold text-ink">{t("skills.run")}</h3>
                <select
                  className={cn(inputClass, "ml-auto h-7 w-auto font-mono text-xs")}
                  value={scriptId}
                  onChange={(event) => setScriptId(event.target.value)}
                  aria-label={t("skills.runScript")}
                >
                  {data.scripts.map((script) => (
                    <option key={script.id} value={script.id}>
                      {script.id}
                    </option>
                  ))}
                </select>
                <Button
                  size="sm"
                  variant="primary"
                  loading={runSkill.isPending}
                  disabled={!data.entry || data.entry === "instructions" || data.scripts.length === 0}
                  onClick={() => runSkill.mutate()}
                >
                  {t("skills.runButton")}
                </Button>
              </div>
              <div className="mt-3 grid gap-3 xl:grid-cols-2">
                <Field label={t("skills.runInput")}>
                  <textarea
                    className={cn(textareaClass, "min-h-[180px] font-mono text-[11px]")}
                    value={inputText}
                    onChange={(event) => setInputText(event.target.value)}
                    spellCheck={false}
                  />
                </Field>
                <div className="grid content-start gap-2">
                  {runResult ? (
                    <>
                      <div className="flex flex-wrap items-center gap-2">
                        <Badge
                          tone={runResult.status === "succeeded" ? "success" : "danger"}
                        >
                          {runResult.status}
                        </Badge>
                        <span className="font-mono text-[10px] text-muted">
                          {t("skills.duration")} {runResult.durationMs} ms
                        </span>
                        <span className="font-mono text-[10px] text-faint">
                          {t("skills.sandbox")} {runResult.sandbox}
                        </span>
                      </div>
                      <JsonView
                        value={runResult.output ?? runResult.error ?? {}}
                        maxHeight="200px"
                      />
                      {runResult.stderr ? (
                        <pre className="hpa-scroll max-h-[100px] overflow-auto rounded-sm border border-danger/20 bg-danger/5 p-2 font-mono text-[10px] text-danger">
                          {runResult.stderr}
                        </pre>
                      ) : null}
                      {runResult.artifacts.length > 0 ? (
                        <div className="flex flex-wrap gap-1.5">
                          {runResult.artifacts.map((artifact) => (
                            <a
                              key={artifact.id}
                              href={`/api/skill-runs/${runResult.id}/artifacts/${artifact.id}`}
                              className="inline-flex h-6 items-center gap-1 rounded-sm border border-line bg-bg-elevated px-2 font-mono text-[10px] text-accent hover:border-accent/40"
                            >
                              <Download className="h-3 w-3" />
                              {artifact.name}
                            </a>
                          ))}
                        </div>
                      ) : null}
                    </>
                  ) : (
                    <p className="text-xs text-muted">{t("skills.runHint")}</p>
                  )}
                </div>
              </div>
            </Panel>

            <Panel className="p-4">
              <div className="flex flex-wrap items-center gap-2">
                <h3 className="text-sm font-semibold text-ink">{t("skills.export")}</h3>
                <input
                  className={cn(inputClass, "ml-auto h-7 max-w-[320px] flex-1 font-mono text-xs")}
                  value={exportPath}
                  onChange={(event) => setExportPath(event.target.value)}
                  aria-label={t("skills.exportPath")}
                />
                <Button
                  size="sm"
                  variant="secondary"
                  loading={exportSkill.isPending}
                  disabled={!data.summary.licenseAllowed || !exportPath.trim()}
                  onClick={() => exportSkill.mutate()}
                >
                  {t("skills.exportAction")}
                </Button>
              </div>
              <p className="mt-2 text-[10.5px] text-faint">{t("skills.exportHint")}</p>
            </Panel>

            <Panel className="p-4">
              <h3 className="text-sm font-semibold text-ink">{t("skills.runs")}</h3>
              {runs.isLoading ? (
                <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
              ) : (runs.data?.runs ?? []).length === 0 ? (
                <p className="mt-2 text-xs text-muted">{t("skills.noRuns")}</p>
              ) : (
                <div className="mt-3 overflow-x-auto rounded-sm border border-line">
                  <table className="w-full min-w-[520px] border-collapse text-left text-[11px]">
                    <thead className="bg-panel-3 text-[9.5px] uppercase tracking-[0.12em] text-faint">
                      <tr>
                        <th className="px-3 py-1.5 font-medium">{t("skills.script")}</th>
                        <th className="px-3 py-1.5 font-medium">{t("common.status")}</th>
                        <th className="px-3 py-1.5 font-medium">{t("skills.duration")}</th>
                        <th className="px-3 py-1.5 font-medium">{t("skills.sandbox")}</th>
                        <th className="px-3 py-1.5 font-medium">{t("common.startedAt")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {(runs.data?.runs ?? []).map((run) => (
                        <tr key={run.id} className="border-t border-line">
                          <td className="px-3 py-1.5 font-mono text-ink">{run.scriptId}</td>
                          <td className="px-3 py-1.5">
                            <Badge tone={run.status === "succeeded" ? "success" : "danger"}>
                              {run.status}
                            </Badge>
                          </td>
                          <td className="px-3 py-1.5 font-mono text-muted">{run.durationMs} ms</td>
                          <td className="px-3 py-1.5 font-mono text-muted">{run.sandbox}</td>
                          <td className="px-3 py-1.5 font-mono text-faint">
                            <Timestamp value={run.startedAt} />
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </Panel>
          </div>
        )}
      </div>
    </div>
  );
}

function Info({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="rounded-sm border border-line bg-bg-elevated px-3 py-2">
      <dt className="text-[0.6rem] uppercase tracking-[0.14em] text-faint">{label}</dt>
      <dd className={cn("mt-1 break-all text-xs text-ink", mono && "font-mono")}>{value}</dd>
    </div>
  );
}

function bumpPatch(version: string): string {
  const match = /^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/.exec(version);
  if (!match) return version;
  return `${match[1]}.${match[2]}.${Number(match[3]) + 1}`;
}

function messageOf(
  error: unknown,
  fallback: string,
  t: (key: string) => string,
): string {
  if (error instanceof ApiError) {
    if (error.code === "SKILL_PERMISSION") return t("skills.permissionDenied");
    return error.message || fallback;
  }
  return error instanceof Error ? error.message : fallback;
}
