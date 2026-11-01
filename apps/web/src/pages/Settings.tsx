import { useQuery } from "@tanstack/react-query";
import { Boxes, Check, Globe2, Moon, Server, Sun, Waves } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Badge, PageHeader, Panel, cn } from "../components/ui";
import { api } from "../lib/api";
import { currentLocale, locales, setLocale, type Locale } from "../lib/i18n";
import { useUiStore, type Theme } from "../stores/ui";

const themes: { value: Theme; icon: typeof Moon; labelKey: string }[] = [
  { value: "light", icon: Sun, labelKey: "settings.themeLight" },
  { value: "dark", icon: Moon, labelKey: "settings.themeDark" },
  { value: "ocean", icon: Waves, labelKey: "settings.themeOcean" },
];

const localeLabels: Record<Locale, string> = {
  ja: "日本語",
  en: "English",
  zh: "中文",
};

export function SettingsPage() {
  const { t } = useTranslation();
  const theme = useUiStore((state) => state.theme);
  const setTheme = useUiStore((state) => state.setTheme);
  const health = useQuery({ queryKey: ["health"], queryFn: api.health, retry: 1 });
  const nodes = useQuery({ queryKey: ["nodes"], queryFn: api.nodes, retry: 1 });
  const activeLocale = currentLocale();

  return (
    <div className="mx-auto grid max-w-[1100px] gap-3">
      <PageHeader title={t("settings.title")} subtitle={t("settings.subtitle")} />

      <Panel className="p-4">
        <h2 className="text-sm font-semibold text-ink">{t("settings.appearance")}</h2>
        <p className="mt-1 text-xs text-muted">{t("settings.theme")}</p>
        <div className="mt-4 grid gap-3 sm:grid-cols-3">
          {themes.map((entry) => {
            const Icon = entry.icon;
            const active = theme === entry.value;
            return (
              <button
                key={entry.value}
                type="button"
                onClick={() => setTheme(entry.value)}
                className={cn(
                  "flex items-center gap-3 rounded-md border px-4 py-3 text-left transition-colors",
                  active
                    ? "border-accent/50 bg-accent/10 text-ink"
                    : "border-line bg-bg-elevated text-muted hover:border-line-strong hover:text-ink",
                )}
              >
                <span
                  className={cn(
                    "grid h-9 w-9 place-items-center rounded-sm border",
                    active ? "border-accent/40 text-accent" : "border-line text-muted",
                  )}
                >
                  <Icon className="h-4 w-4" />
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block text-xs font-medium">{t(entry.labelKey)}</span>
                  <span className="mt-1 flex gap-1">
                    <span className="h-2 w-4 rounded-full bg-bg" />
                    <span className="h-2 w-4 rounded-full bg-panel" />
                    <span className="h-2 w-4 rounded-full bg-accent" />
                  </span>
                </span>
                {active ? <Check className="h-4 w-4 text-accent" /> : null}
              </button>
            );
          })}
        </div>
      </Panel>

      <Panel className="p-4">
        <div className="flex items-center gap-2">
          <Globe2 className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("settings.language")}</h2>
          <span className="text-xs text-faint">{t("settings.threeLocales")}</span>
        </div>
        <div className="mt-4 flex flex-wrap gap-2">
          {locales.map((locale) => (
            <button
              key={locale}
              type="button"
              onClick={() => setLocale(locale)}
              className={cn(
                "inline-flex h-9 items-center gap-2 rounded-sm border px-4 text-xs transition-colors",
                activeLocale === locale
                  ? "border-accent/50 bg-accent/10 text-accent"
                  : "border-line bg-bg-elevated text-muted hover:border-line-strong hover:text-ink",
              )}
            >
              {localeLabels[locale]}
              {activeLocale === locale ? <Check className="h-3.5 w-3.5" /> : null}
            </button>
          ))}
        </div>
      </Panel>

      <Panel className="p-4">
        <div className="flex items-center gap-2">
          <Server className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("settings.runtime")}</h2>
          <Badge tone={health.isError ? "danger" : "success"} dot>
            {health.isError ? t("shell.offline") : t("shell.online")}
          </Badge>
        </div>
        <dl className="mt-4 grid gap-3 sm:grid-cols-2">
          <Info label={t("settings.product")} value={health.data?.brand?.productName ?? t("settings.unknown")} />
          <Info label={t("settings.internalId")} value={health.data?.internalId ?? "hpa"} mono />
          <Info label={t("settings.version")} value={health.data?.version ?? t("settings.unknown")} mono />
          <Info label={t("settings.offline")} value={health.isError ? t("common.no") : t("common.yes")} />
        </dl>
      </Panel>

      <Panel className="p-4">
        <div className="flex flex-wrap items-center gap-2">
          <Boxes className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("settings.registry")}</h2>
          {nodes.data ? (
            <span className="font-mono text-xs text-faint">
              {t("settings.registryVersion")} {nodes.data.registryVersion} ·{" "}
              {nodes.data.nodes.length} {t("settings.nodeTypes")}
            </span>
          ) : null}
        </div>
        <p className="mt-1 text-xs text-muted">{t("settings.registryHint")}</p>
        {nodes.isError ? (
          <p className="mt-3 text-xs text-danger">{t("errors.loadFailed")}</p>
        ) : (
          <div className="mt-3 overflow-x-auto rounded-sm border border-line">
            <table className="w-full min-w-[520px] border-collapse text-left text-xs">
              <thead className="bg-bg-elevated text-[0.6rem] uppercase tracking-[0.14em] text-faint">
                <tr>
                  <th className="px-3 py-2 font-medium">{t("settings.nodeType")}</th>
                  <th className="px-3 py-2 font-medium">{t("settings.nodeVersions")}</th>
                  <th className="px-3 py-2 font-medium">{t("settings.permissions")}</th>
                </tr>
              </thead>
              <tbody>
                {(nodes.data?.nodes ?? []).map((node) => (
                  <tr key={node.type} className="border-t border-line">
                    <td className="px-3 py-2 font-mono text-ink">{node.type}</td>
                    <td className="px-3 py-2 font-mono text-muted">
                      {node.versions
                        .map((version) =>
                          version.version === node.defaultVersion
                            ? `${version.version} *`
                            : version.version,
                        )
                        .join(" / ")}
                    </td>
                    <td className="px-3 py-2 text-muted">
                      {node.permissionKinds.length > 0
                        ? node.permissionKinds.join(", ")
                        : t("common.none")}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Panel>
    </div>
  );
}

function Info({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="rounded-sm border border-line bg-bg-elevated px-3 py-2">
      <dt className="text-[0.6rem] uppercase tracking-[0.14em] text-faint">{label}</dt>
      <dd className={cn("mt-1 text-xs text-ink", mono && "font-mono")}>{value}</dd>
    </div>
  );
}
