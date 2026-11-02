import { cva, type VariantProps } from "class-variance-authority";
import { clsx, type ClassValue } from "clsx";
import { Check, Copy, Loader2, X } from "lucide-react";
import type { ButtonHTMLAttributes, ReactNode } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { twMerge } from "tailwind-merge";
import type { ExecutionStatus, Issue, ValidationReport } from "../lib/types";
import { formatDate } from "../lib/i18n";
import { useUiStore } from "../stores/ui";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

const buttonVariants = cva(
  "inline-flex select-none items-center justify-center gap-1.5 rounded-sm border font-medium transition-[background,border-color,color] duration-100 active:translate-y-px disabled:pointer-events-none disabled:opacity-45",
  {
    variants: {
      variant: {
        primary:
          "border-accent bg-accent text-accent-ink hover:border-accent-strong hover:bg-accent-strong",
        secondary:
          "border-line-strong bg-panel text-ink hover:bg-panel-3",
        ghost:
          "border-transparent bg-transparent text-muted hover:bg-accent-soft hover:text-accent-text",
        danger:
          "border-danger/30 bg-danger/10 text-danger hover:border-danger/50",
        subtle:
          "border-line bg-transparent text-muted hover:border-line-strong hover:text-ink",
      },
      size: {
        xs: "h-5 px-1.5 text-[10.5px]",
        sm: "h-6 px-2 text-[11.5px]",
        md: "h-7 px-2.5 text-[12px]",
        icon: "h-7 w-7",
      },
    },
    defaultVariants: { variant: "secondary", size: "md" },
  },
);

export type ButtonVariantProps = VariantProps<typeof buttonVariants>;

interface ButtonProps
  extends ButtonHTMLAttributes<HTMLButtonElement>,
    ButtonVariantProps {
  loading?: boolean;
}

export function Button({
  className,
  variant,
  size,
  loading,
  children,
  disabled,
  ...props
}: ButtonProps) {
  return (
    <button
      className={cn(buttonVariants({ variant, size }), className)}
      disabled={disabled || loading}
      {...props}
    >
      {loading ? <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" /> : null}
      {children}
    </button>
  );
}

const badgeTones = {
  neutral: "border-line bg-panel-3 text-muted",
  accent: "border-accent/30 bg-accent/10 text-accent",
  success: "border-success/30 bg-success/10 text-success",
  danger: "border-danger/30 bg-danger/10 text-danger",
  warning: "border-warning/30 bg-warning/10 text-warning",
  info: "border-info/30 bg-info/10 text-info",
  graph: "border-graph/30 bg-graph/10 text-graph",
} as const;

export type BadgeTone = keyof typeof badgeTones;

export function Badge({
  children,
  tone = "neutral",
  dot,
  className,
}: {
  children: ReactNode;
  tone?: BadgeTone;
  dot?: boolean;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "inline-flex h-[17px] items-center gap-1 rounded-xs border px-[5px] text-[10px] font-semibold whitespace-nowrap",
        badgeTones[tone],
        className,
      )}
    >
      {dot ? <span className="h-[5px] w-[5px] rounded-full bg-current" aria-hidden="true" /> : null}
      {children}
    </span>
  );
}

const statusTones: Record<ExecutionStatus, BadgeTone> = {
  pending: "neutral",
  running: "info",
  waiting_approval: "warning",
  succeeded: "success",
  failed: "danger",
  cancelled: "neutral",
};

export function StatusBadge({ status }: { status: ExecutionStatus }) {
  const { t } = useTranslation();
  return (
    <Badge tone={statusTones[status] ?? "neutral"} dot>
      {t(`status.${status}`, { defaultValue: status })}
    </Badge>
  );
}

export function PageHeader({
  title,
  subtitle,
  actions,
}: {
  title: string;
  subtitle?: string;
  actions?: ReactNode;
}) {
  return (
    <header className="mb-3.5 flex flex-wrap items-start justify-between gap-3">
      <div className="min-w-0">
        <h1 className="text-[17px] font-semibold leading-tight tracking-tight text-ink-strong">
          {title}
        </h1>
        {subtitle ? <p className="mt-1 max-w-[78ch] text-xs text-muted">{subtitle}</p> : null}
      </div>
      {actions ? <div className="flex flex-wrap items-center gap-1.5">{actions}</div> : null}
    </header>
  );
}

export function Panel({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <section
      className={cn(
        "overflow-hidden rounded-md border border-line bg-panel",
        className,
      )}
    >
      {children}
    </section>
  );
}

export function EmptyState({
  icon,
  title,
  hint,
  action,
}: {
  icon?: ReactNode;
  title: string;
  hint?: string;
  action?: ReactNode;
}) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 px-5 py-9 text-center">
      {icon ? (
        <div className="grid h-9 w-9 place-items-center rounded-full bg-panel-3 text-faint">
          {icon}
        </div>
      ) : null}
      <div>
        <p className="text-[13px] font-semibold text-ink">{title}</p>
        {hint ? <p className="mt-1 max-w-[40ch] text-xs text-faint">{hint}</p> : null}
      </div>
      {action}
    </div>
  );
}

export function Field({
  label,
  hint,
  children,
  className,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <label className={cn("grid gap-1", className)}>
      <span className="text-[11px] font-semibold text-muted">{label}</span>
      {children}
      {hint ? <span className="text-[10.5px] text-faint">{hint}</span> : null}
    </label>
  );
}

export const inputClass =
  "h-7 w-full rounded-sm border border-line-strong bg-panel px-2 text-[12px] text-ink outline-none transition-[border-color,box-shadow] placeholder:text-faint focus:border-accent focus:ring-2 focus:ring-accent/15";

export const textareaClass =
  "w-full rounded-sm border border-line-strong bg-panel px-2 py-1.5 font-mono text-[11px] leading-relaxed text-ink outline-none transition-[border-color,box-shadow] placeholder:text-faint focus:border-accent focus:ring-2 focus:ring-accent/15";

export function JsonView({
  value,
  className,
  maxHeight = "none",
}: {
  value: unknown;
  className?: string;
  maxHeight?: string;
}) {
  const text = typeof value === "string" ? value : JSON.stringify(value, null, 2);
  return (
    <pre
      tabIndex={0}
      className={cn(
        "hpa-scroll overflow-auto rounded-sm border border-line bg-panel-2 p-2.5 font-mono text-[10.5px] leading-relaxed text-ink",
        className,
      )}
      style={{ maxHeight }}
    >
      {text}
    </pre>
  );
}

export function CopyButton({ text, label }: { text: string; label?: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  return (
    <Button
      size="xs"
      variant="ghost"
      onClick={() => {
        void navigator.clipboard.writeText(text).then(() => {
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1500);
        });
      }}
      title={label ?? t("common.copy")}
    >
      {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
      {copied ? t("common.copied") : t("common.copy")}
    </Button>
  );
}

export function Timestamp({ value }: { value?: string }) {
  return (
    <time className="font-mono text-[10.5px] tabular-nums text-faint" dateTime={value}>
      {formatDate(value)}
    </time>
  );
}

export function ValidationList({ report }: { report: ValidationReport }) {
  const { t } = useTranslation();
  if (report.issues.length === 0) {
    return <p className="text-[11px] text-success">{t("validation.noIssues")}</p>;
  }
  return (
    <ul className="grid gap-1.5">
      {report.issues.map((issue: Issue, index) => (
        <li
          key={`${issue.code}-${issue.path}-${index}`}
          className="rounded-sm border border-line bg-panel-2 px-2 py-1.5"
        >
          <div className="flex items-center gap-2">
            <Badge tone={issue.severity === "error" ? "danger" : "warning"}>
              {issue.severity === "error" ? t("common.error") : t("common.warnings")}
            </Badge>
            <span className="font-mono text-[10px] text-muted">{issue.code}</span>
            <span className="ml-auto truncate font-mono text-[10px] text-faint">
              {issue.path}
            </span>
          </div>
          <p className="mt-1 text-[11.5px] text-ink">
            {t(`validationCodes.${issue.code}`, { defaultValue: issue.message })}
          </p>
        </li>
      ))}
    </ul>
  );
}

export function ToastViewport() {
  const { t } = useTranslation();
  const toasts = useUiStore((state) => state.toasts);
  const dismiss = useUiStore((state) => state.dismissToast);
  if (toasts.length === 0) {
    return null;
  }
  return (
    <div
      className="fixed bottom-[18px] left-[18px] z-[80] grid w-[min(360px,calc(100vw-2rem))] gap-2"
      role="status"
      aria-live="polite"
    >
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className="flex items-start gap-2 rounded-sm border border-line bg-panel px-2.5 py-2 shadow-float"
        >
          <span
            className={cn(
              "mt-1 h-2 w-2 shrink-0 rounded-full",
              toast.tone === "success"
                ? "bg-success"
                : toast.tone === "error"
                  ? "bg-danger"
                  : toast.tone === "warning"
                    ? "bg-warning"
                    : "bg-info",
            )}
          />
          <p className="min-w-0 flex-1 text-[11.5px] text-ink">{toast.message}</p>
          <button
            type="button"
            className="rounded-xs p-1 text-faint hover:bg-panel-2 hover:text-ink"
            onClick={() => dismiss(toast.id)}
            aria-label={t("common.dismiss")}
          >
            <X className="h-3 w-3" />
          </button>
        </div>
      ))}
    </div>
  );
}
