import { Trash2 } from "lucide-react";
import { useEffect, useState, type InputHTMLAttributes, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { JsonValue, WorkflowIR, WorkflowNode } from "../../lib/types";
import { useEditorStore } from "../../stores/editor";
import { useUiStore } from "../../stores/ui";
import {
  Badge,
  Button,
  EmptyState,
  Field,
  cn,
  inputClass,
  textareaClass,
} from "../ui";

interface Props {
  ir: WorkflowIR;
  node: WorkflowNode | null;
}

export function NodeInspector({ ir, node }: Props) {
  const { t } = useTranslation();
  const patchNode = useEditorStore((state) => state.patchNode);
  const setConfig = useEditorStore((state) => state.setConfig);
  const removeNode = useEditorStore((state) => state.removeNode);
  const pushToast = useUiStore((state) => state.pushToast);

  if (!node) {
    return (
      <EmptyState
        icon={<span className="font-mono text-2xl">⌁</span>}
        title={t("inspector.noNode")}
        hint={t("designer.noSelection")}
      />
    );
  }

  const config = node.config as Record<string, JsonValue>;
  const setKey = (key: string, value: JsonValue) => {
    setConfig(node.id, { ...config, [key]: value });
  };

  return (
    <div className="hpa-scroll h-full overflow-y-auto px-4 py-4">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <Badge tone="accent">{t(nodeTypeKey(node.type))}</Badge>
            <span className="truncate font-mono text-[0.66rem] text-faint">{node.id}</span>
          </div>
          <p className="mt-2 text-xs leading-relaxed text-muted">{t(nodeDescKey(node.type))}</p>
        </div>
        <Button
          variant="danger"
          size="icon"
          onClick={() => {
            removeNode(node.id);
            pushToast(t("designer.deleteNode"), "info");
          }}
          title={t("designer.deleteNode")}
          aria-label={t("designer.deleteNode")}
        >
          <Trash2 className="h-3.5 w-3.5" />
        </Button>
      </div>

      <div className="mt-5 grid gap-3">
        <Section title={t("inspector.general")}>
          <Field label={t("inspector.name")}>
            <TextInput
              value={node.name ?? ""}
              onChange={(event) => patchNode(node.id, { name: event.target.value })}
              placeholder={node.id}
            />
          </Field>
          <Field label={t("inspector.nodeId")}>
            <input className={cn(inputClass, "font-mono text-xs opacity-70")} value={node.id} readOnly />
          </Field>
          <div className="grid grid-cols-2 gap-3">
            <Field label={t("inspector.typeVersion")}>
              <TextInput
                value={node.typeVersion}
                onChange={(event) => patchNode(node.id, { typeVersion: event.target.value })}
              />
            </Field>
            <Field label={t("inspector.timeoutMs")}>
              <TextInput
                type="number"
                min={1}
                value={node.timeoutMs ?? ""}
                onChange={(event) =>
                  patchNode(node.id, {
                    timeoutMs: event.target.value ? Number(event.target.value) : undefined,
                  })
                }
              />
            </Field>
          </div>
          <Field label={t("inspector.description")}>
            <textarea
              className={cn(textareaClass, "min-h-[64px] font-sans text-xs")}
              value={node.description ?? ""}
              onChange={(event) => patchNode(node.id, { description: event.target.value })}
              rows={2}
            />
          </Field>
          {node.type === "http.request" ? (
            <Field
              label={t("inspector.networkPermissions")}
              hint={t("inspector.networkPermissionsHint")}
            >
              <TextInput
                value={networkPermissions(node)}
                onChange={(event) => {
                  const entries = event.target.value
                    .split(",")
                    .map((entry) => entry.trim())
                    .filter(Boolean);
                  patchNode(node.id, {
                    permissions: entries.length
                      ? { ...(node.permissions ?? {}), network: entries }
                      : undefined,
                  });
                }}
                placeholder="api.example.com:443, *.trusted.example"
              />
            </Field>
          ) : null}
        </Section>

        <Section title={t("inspector.config")}>
          {node.type === "transform" ? (
            <JsonField
              label={t("inspector.mapping")}
              value={config.mapping}
              onApply={(value) => setKey("mapping", value as JsonValue)}
            />
          ) : null}
          {node.type === "condition" ? (
            <ExpressionField
              label={t("inspector.condition")}
              value={exprSource(config.condition)}
              onChange={(source) => setKey("condition", { $expr: source })}
            />
          ) : null}
          {node.type === "switch" ? (
            <>
              <JsonField
                label={t("inspector.cases")}
                value={config.cases ?? []}
                onApply={(value) => setKey("cases", value as JsonValue)}
              />
              <Field label={t("inspector.defaultPort")}>
                <TextInput
                  value={typeof config.defaultPort === "string" ? config.defaultPort : "default"}
                  onChange={(event) => setKey("defaultPort", event.target.value)}
                />
              </Field>
            </>
          ) : null}
          {node.type === "loop" ? (
            <>
              <div className="grid grid-cols-2 gap-3">
                <Field label={t("inspector.mode")}>
                  <select
                    className={inputClass}
                    value={typeof config.mode === "string" ? config.mode : "forEach"}
                    onChange={(event) => setKey("mode", event.target.value)}
                  >
                    <option value="forEach">forEach</option>
                    <option value="while">while</option>
                  </select>
                </Field>
                <Field label={t("inspector.maxIterations")}>
                  <TextInput
                    type="number"
                    min={1}
                    value={Number(config.maxIterations ?? 100)}
                    onChange={(event) => setKey("maxIterations", Number(event.target.value))}
                  />
                </Field>
              </div>
              <JsonField
                label={t("inspector.items")}
                value={config.items ?? { $expr: "[]" }}
                onApply={(value) => setKey("items", value as JsonValue)}
              />
              <div className="grid grid-cols-2 gap-3">
                <Field label="item">
                  <TextInput
                    value={typeof config.itemVariable === "string" ? config.itemVariable : "item"}
                    onChange={(event) => setKey("itemVariable", event.target.value)}
                  />
                </Field>
                <Field label="index">
                  <TextInput
                    value={typeof config.indexVariable === "string" ? config.indexVariable : "index"}
                    onChange={(event) => setKey("indexVariable", event.target.value)}
                  />
                </Field>
              </div>
              <p className="text-[0.68rem] leading-relaxed text-faint">{t("designer.containerHint")}</p>
            </>
          ) : null}
          {node.type === "parallel" ? (
            <>
              <div className="grid grid-cols-2 gap-3">
                <Field label={t("inspector.join")}>
                  <select
                    className={inputClass}
                    value={typeof config.join === "string" ? config.join : "all"}
                    onChange={(event) => setKey("join", event.target.value)}
                  >
                    <option value="all">all</option>
                    <option value="any">any</option>
                    <option value="n_of">n_of</option>
                  </select>
                </Field>
                <Field label={t("inspector.maxConcurrency")}>
                  <TextInput
                    type="number"
                    min={1}
                    value={Number(config.maxConcurrency ?? 2)}
                    onChange={(event) => setKey("maxConcurrency", Number(event.target.value))}
                  />
                </Field>
              </div>
              <label className="flex items-center gap-2 text-xs text-ink">
                <input
                  type="checkbox"
                  className="accent-accent"
                  checked={Boolean(config.failFast)}
                  onChange={(event) => setKey("failFast", event.target.checked)}
                />
                {t("inspector.failFast")}
              </label>
              <p className="text-[0.68rem] leading-relaxed text-faint">{t("designer.containerHint")}</p>
            </>
          ) : null}
          {node.type === "human.approval" ? (
            <>
              <Field label={t("inspector.title")}>
                <TextInput
                  value={typeof config.title === "string" ? config.title : ""}
                  onChange={(event) => setKey("title", event.target.value)}
                />
              </Field>
              <div className="grid grid-cols-2 gap-3">
                <Field label={t("inspector.onTimeout")}>
                  <select
                    className={inputClass}
                    value={typeof config.onTimeout === "string" ? config.onTimeout : "reject"}
                    onChange={(event) => setKey("onTimeout", event.target.value)}
                  >
                    <option value="reject">reject</option>
                    <option value="approve">approve</option>
                  </select>
                </Field>
                <Field label={t("inspector.slaMs")}>
                  <TextInput
                    type="number"
                    min={1}
                    value={typeof config.slaMs === "number" ? config.slaMs : ""}
                    onChange={(event) =>
                      setKey("slaMs", event.target.value ? Number(event.target.value) : null)
                    }
                  />
                </Field>
              </div>
              <JsonField
                label="formSchema"
                value={config.formSchema ?? {}}
                onApply={(value) => setKey("formSchema", value as JsonValue)}
              />
            </>
          ) : null}
          {node.type === "http.request" ? (
            <>
              <div className="grid grid-cols-[100px_minmax(0,1fr)] gap-3">
                <Field label={t("inspector.method")}>
                  <select
                    className={inputClass}
                    value={typeof config.method === "string" ? config.method : "GET"}
                    onChange={(event) => setKey("method", event.target.value)}
                  >
                    {["GET", "POST", "PUT", "PATCH", "DELETE"].map((method) => (
                      <option key={method} value={method}>
                        {method}
                      </option>
                    ))}
                  </select>
                </Field>
                <Field
                  label={t("inspector.url")}
                  hint={t("inspector.urlHint")}
                >
                  <TextInput
                    className="font-mono text-xs"
                    value={urlSource(config.url)}
                    onChange={(event) => {
                      const value = event.target.value;
                      setKey(
                        "url",
                        /^https?:\/\/\S+$/i.test(value.trim())
                          ? value.trim()
                          : { $expr: value },
                      );
                    }}
                  />
                </Field>
              </div>
              <JsonField
                label="body"
                value={config.body ?? {}}
                onApply={(value) => setKey("body", value as JsonValue)}
              />
            </>
          ) : null}
          {node.type === "script" ? (
            <>
              <Field label={t("inspector.scriptLanguage")}>
                <input className={inputClass} value="jq" readOnly />
              </Field>
              <Field label={t("inspector.scriptSource")}>
                <textarea
                  className={cn(textareaClass, "min-h-[120px]")}
                  value={typeof config.source === "string" ? config.source : "."}
                  onChange={(event) => setKey("source", event.target.value)}
                  spellCheck={false}
                />
              </Field>
            </>
          ) : null}
          {node.type === "setVariable" ? (
            <>
              <Field label={t("inspector.variableName")}>
                <TextInput
                  value={typeof config.name === "string" ? config.name : ""}
                  onChange={(event) => setKey("name", event.target.value)}
                />
              </Field>
              <JsonField
                label={t("inspector.value")}
                value={config.value ?? {}}
                onApply={(value) => setKey("value", value as JsonValue)}
              />
            </>
          ) : null}
          {node.type === "subflow" ? (
            <Field label={t("inspector.subflowId")}>
              <select
                className={inputClass}
                value={typeof config.subflowId === "string" ? config.subflowId : ""}
                onChange={(event) => setKey("subflowId", event.target.value)}
              >
                <option value="">—</option>
                {(ir.subflows ?? []).map((subflow) => (
                  <option key={subflow.id} value={subflow.id}>
                    {subflow.name} ({subflow.id})
                  </option>
                ))}
              </select>
            </Field>
          ) : null}
          {node.type === "end" ? (
            <JsonField
              label={t("common.output")}
              value={config.output ?? {}}
              onApply={(value) => setKey("output", value as JsonValue)}
            />
          ) : null}
        </Section>

        <Section title={t("inspector.advanced")}>
          <div className="grid grid-cols-2 gap-3">
            <Field label={t("inspector.retryAttempts")}>
              <TextInput
                type="number"
                min={1}
                value={node.retry?.maxAttempts ?? ""}
                onChange={(event) =>
                  patchNode(node.id, {
                    retry: event.target.value
                      ? { ...(node.retry ?? {}), maxAttempts: Number(event.target.value) }
                      : undefined,
                  })
                }
              />
            </Field>
            <Field label={t("inspector.errorMode")}>
              <select
                className={inputClass}
                value={node.errorPolicy?.mode ?? "fail"}
                onChange={(event) =>
                  patchNode(node.id, {
                    errorPolicy: {
                      ...(node.errorPolicy ?? {}),
                      mode: event.target.value as "fail" | "skip" | "continue" | "fallback" | "compensate",
                    },
                  })
                }
              >
                {["fail", "skip", "continue", "fallback", "compensate"].map((mode) => (
                  <option key={mode} value={mode}>
                    {mode}
                  </option>
                ))}
              </select>
            </Field>
          </div>
          {node.errorPolicy?.mode === "fallback" ? (
            <Field label={t("inspector.fallbackPort")}>
              <TextInput
                value={node.errorPolicy.fallbackPort ?? "error"}
                onChange={(event) =>
                  patchNode(node.id, {
                    errorPolicy: {
                      ...(node.errorPolicy ?? { mode: "fallback" }),
                      fallbackPort: event.target.value,
                    },
                  })
                }
              />
            </Field>
          ) : null}
          <JsonField
            label={t("inspector.configJson")}
            value={config}
            onApply={(value) => {
              if (value && typeof value === "object" && !Array.isArray(value)) {
                setConfig(node.id, value as Record<string, JsonValue>);
              } else {
                pushToast(t("inspector.configInvalid"), "error");
              }
            }}
          />
        </Section>
      </div>
    </div>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="rounded-md border border-line bg-panel-2/50 p-3">
      <h3 className="mb-3 text-[0.62rem] font-semibold uppercase tracking-[0.16em] text-faint">
        {title}
      </h3>
      <div className="grid gap-3">{children}</div>
    </section>
  );
}

function TextInput(props: InputHTMLAttributes<HTMLInputElement>) {
  return <input {...props} className={cn(inputClass, props.className)} />;
}

function ExpressionField({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (source: string) => void;
}) {
  return (
    <Field label={label}>
      <textarea
        className={cn(textareaClass, "min-h-[84px]")}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        spellCheck={false}
      />
    </Field>
  );
}

function JsonField({
  label,
  value,
  onApply,
}: {
  label: string;
  value: unknown;
  onApply: (value: unknown) => void;
}) {
  const { t } = useTranslation();
  const [text, setText] = useState(() => JSON.stringify(value ?? {}, null, 2));
  const [error, setError] = useState(false);

  useEffect(() => {
    setText(JSON.stringify(value ?? {}, null, 2));
    setError(false);
  }, [value]);

  return (
    <div className="grid gap-1.5">
      <span className="text-[0.7rem] font-medium uppercase tracking-[0.12em] text-muted">
        {label}
      </span>
      <textarea
        className={cn(textareaClass, "min-h-[96px]", error && "border-danger/70")}
        value={text}
        onChange={(event) => {
          setText(event.target.value);
          setError(false);
        }}
        spellCheck={false}
      />
      {error ? <span className="text-[0.68rem] text-danger">{t("inspector.configInvalid")}</span> : null}
      <div className="flex justify-end">
        <Button
          size="xs"
          variant="secondary"
          onClick={() => {
            try {
              onApply(JSON.parse(text) as unknown);
              setError(false);
            } catch {
              setError(true);
            }
          }}
        >
          {t("inspector.applyConfig")}
        </Button>
      </div>
    </div>
  );
}

function exprSource(value: unknown): string {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    const source = (value as Record<string, unknown>).$expr;
    if (typeof source === "string") {
      return source;
    }
  }
  return typeof value === "string" ? JSON.stringify(value) : "";
}

function urlSource(value: unknown): string {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    const source = (value as Record<string, unknown>).$expr;
    if (typeof source === "string") {
      return source;
    }
  }
  return typeof value === "string" ? value : "";
}

function nodeTypeKey(type: string): string {
  const map: Record<string, string> = {
    start: "node.start",
    end: "node.end",
    transform: "node.transform",
    condition: "node.condition",
    switch: "node.switch",
    loop: "node.loop",
    parallel: "node.parallel",
    join: "node.join",
    "http.request": "node.http",
    script: "node.script",
    setVariable: "node.setVariable",
    "human.approval": "node.approval",
    subflow: "node.subflow",
  };
  return map[type] ?? type;
}

function nodeDescKey(type: string): string {
  const map: Record<string, string> = {
    start: "node.desc.start",
    end: "node.desc.end",
    transform: "node.desc.transform",
    condition: "node.desc.condition",
    switch: "node.desc.switch",
    loop: "node.desc.loop",
    parallel: "node.desc.parallel",
    join: "node.desc.join",
    "http.request": "node.desc.http",
    script: "node.desc.script",
    setVariable: "node.desc.setVariable",
    "human.approval": "node.desc.approval",
    subflow: "node.desc.subflow",
  };
  return map[type] ?? "node.desc.transform";
}

function networkPermissions(node: WorkflowNode): string {
  const network = node.permissions?.network;
  if (!Array.isArray(network)) {
    return "";
  }
  return network.filter((entry): entry is string => typeof entry === "string").join(", ");
}
